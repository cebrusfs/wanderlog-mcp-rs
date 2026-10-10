//! The stdio server as an agent drives it: a short edit flow on a trip it creates itself, and a
//! read-only pass over every trip in the account. Both check that no trip key or the session
//! cookie reaches the protocol stream. The edit-op matrix itself is covered by `client`.

use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use wanderlog_mcp::auth;

use crate::{Account, END, START};

/// A `wanderlog-mcp serve` child process; the cookie goes in by environment, not the Keychain.
struct Server {
    child: Child,
    stdin: ChildStdin,
    stdout: Lines<BufReader<ChildStdout>>,
    next_id: u64,
    /// Every line the server wrote, for the secret check.
    transcript: String,
}

struct Reply {
    tool: String,
    text: String,
    is_error: bool,
    structured: Value,
}

impl Reply {
    fn ok(self) -> Result<String> {
        ensure!(!self.is_error, "{} failed: {}", self.tool, self.text);
        Ok(self.text)
    }
}

impl Server {
    async fn start(cookie: &str) -> Result<Self> {
        let mut child = Command::new(env!("CARGO_BIN_EXE_wanderlog-mcp"))
            .arg("serve")
            .env(auth::ENV_VAR, cookie)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .kill_on_drop(true)
            .spawn()?;
        let mut server = Self {
            stdin: child.stdin.take().context("no stdin")?,
            stdout: BufReader::new(child.stdout.take().context("no stdout")?).lines(),
            child,
            next_id: 0,
            transcript: String::new(),
        };
        server
            .request(
                "initialize",
                json!({"protocolVersion": "2025-11-25", "capabilities": {}, "clientInfo": {"name": "e2e", "version": "1"}}),
            )
            .await?;
        server
            .send(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))
            .await?;
        Ok(server)
    }

    async fn send(&mut self, message: &Value) -> Result<()> {
        self.stdin
            .write_all(format!("{message}\n").as_bytes())
            .await?;
        Ok(())
    }

    async fn request(&mut self, method: &str, params: Value) -> Result<Value> {
        self.next_id += 1;
        let id = self.next_id;
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))
            .await?;
        loop {
            let line = tokio::time::timeout(Duration::from_secs(120), self.stdout.next_line())
                .await
                .with_context(|| format!("{method}: no answer within 120 s"))??
                .with_context(|| format!("{method}: server exited"))?;
            self.transcript.push_str(&line);
            self.transcript.push('\n');
            let message: Value = serde_json::from_str(&line)?;
            if message["id"] != id {
                continue;
            }
            if let Some(error) = message.get("error") {
                bail!("{method}: {error}");
            }
            return Ok(message["result"].clone());
        }
    }

    async fn call(&mut self, tool: &str, arguments: Value) -> Result<Reply> {
        let result = self
            .request("tools/call", json!({"name": tool, "arguments": arguments}))
            .await?;
        let text = result["content"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|c| c["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n");
        Ok(Reply {
            tool: tool.to_owned(),
            text,
            is_error: result["isError"] == true,
            structured: result["structuredContent"].clone(),
        })
    }

    /// Fails without echoing the secret it found.
    fn ensure_no_secrets<'a>(&self, secrets: impl IntoIterator<Item = &'a str>) -> Result<()> {
        for secret in secrets {
            ensure!(
                !self.transcript.contains(secret),
                "a trip key or the session cookie reached the MCP output"
            );
        }
        Ok(())
    }

    async fn stop(mut self) -> Result<()> {
        drop(self.stdin);
        let status = tokio::time::timeout(Duration::from_secs(10), self.child.wait())
            .await
            .context("server did not exit after stdin closed")??;
        ensure!(status.success(), "server exited with {status}");
        Ok(())
    }
}

/// The number after `prefix`, e.g. the id in "Created trip 123 for …".
fn number_after(text: &str, prefix: &str) -> Option<u64> {
    text.split(prefix).skip(1).find_map(|rest| {
        let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
        digits.parse().ok()
    })
}

#[tokio::test]
#[ignore = "talks to wanderlog.com with the stored session"]
async fn tools_edit_a_new_trip() -> Result<()> {
    let account = Account::load().await?;
    let mut server = Server::start(&account.cookie).await?;
    let title = crate::title();
    let created = server
        .call(
            "create_trip",
            json!({"destination": "Tokyo", "start_date": START, "end_date": END, "title": title}),
        )
        .await?;
    // A late failure still names the created trip, so it can be cleaned up.
    let trip_id = number_after(&created.text, "trip ")
        .with_context(|| format!("create_trip: {}", created.text))?;
    let key = account.own_key(trip_id).await?;
    account
        .delete_after(&key, async {
            ensure!(!created.is_error, "create_trip failed: {}", created.text);
            edit_flow(&mut server, trip_id, &title).await
        })
        .await?;
    server.ensure_no_secrets([key.as_str(), account.cookie.as_str()])?;
    server.stop().await
}

/// get_trip → search_places → apply_edits on the read revision; a second apply on that now stale
/// revision must be refused before writing, and get_trip must show the first apply's revision.
async fn edit_flow(server: &mut Server, trip_id: u64, title: &str) -> Result<()> {
    let listed = server.call("list_trips", json!({})).await?.ok()?;
    ensure!(
        listed.contains(&format!("trip {trip_id} ")),
        "list_trips does not show the new trip"
    );
    let view = server
        .call("get_trip", json!({"trip_id": trip_id}))
        .await?
        .ok()?;
    ensure!(view.contains(title), "get_trip does not show the title");
    let base = number_after(&view, "revision ").context("get_trip shows no revision")?;
    let day = view
        .lines()
        .find(|l| l.starts_with("## Day 1 "))
        .and_then(|l| l.rsplit_once('[')?.1.strip_suffix(']'))
        .context("get_trip shows no day 1")?
        .to_owned();
    let found = server
        .call(
            "search_places",
            json!({"query": "Tokyo Tower", "trip_id": trip_id}),
        )
        .await?
        .ok()?;
    let place_id = found
        .lines()
        .find_map(|l| l.strip_prefix("place_id ")?.split(' ').next())
        .context("search_places found nothing")?
        .to_owned();

    let note = format!("{title} note");
    let applied = server
        .call(
            "apply_edits",
            json!({"trip_id": trip_id, "base_revision": base, "edits": [
                {"op": "add_place", "section": day, "place_id": place_id, "start_time": "10:00"},
                {"op": "add_note", "section": day, "text": note},
            ]}),
        )
        .await?
        .ok()?;
    let revision =
        number_after(&applied, "as revision ").context("apply_edits names no revision")?;

    let stale_note = format!("{title} stale");
    let stale = server
        .call(
            "apply_edits",
            json!({"trip_id": trip_id, "base_revision": base, "edits": [
                {"op": "add_note", "section": day, "text": stale_note},
            ]}),
        )
        .await?;
    ensure!(
        stale.is_error
            && stale.structured["code"] == "REVISION_CONFLICT"
            && stale.structured["write_state"] == "not_started",
        "an apply on a stale revision was not refused: {}",
        stale.text
    );

    let view = server
        .call("get_trip", json!({"trip_id": trip_id, "detail": "full"}))
        .await?
        .ok()?;
    ensure!(
        number_after(&view, "revision ") == Some(revision),
        "get_trip does not show the applied revision {revision}"
    );
    ensure!(
        view.contains(&place_id) && view.contains(&note),
        "get_trip does not show the applied edits"
    );
    ensure!(
        !view.contains(&stale_note),
        "the refused apply changed the trip"
    );
    Ok(())
}

/// Real trips hold data the fixtures do not: every one must render, compact and full.
#[tokio::test]
#[ignore = "talks to wanderlog.com with the stored session"]
async fn get_trip_reads_every_trip() -> Result<()> {
    let account = Account::load().await?;
    let keys: Vec<String> = account
        .rest
        .trips()
        .await?
        .into_iter()
        .map(|t| t.key)
        .collect();
    let mut server = Server::start(&account.cookie).await?;
    let listed = server.call("list_trips", json!({})).await?.ok()?;
    let ids: Vec<u64> = listed
        .lines()
        .filter(|l| l.starts_with("trip "))
        .filter_map(|l| number_after(l, "trip "))
        .collect();
    ensure!(
        ids.len() == keys.len(),
        "list_trips shows {} trips, the account has {}",
        ids.len(),
        keys.len()
    );
    for &id in &ids {
        for detail in ["compact", "full"] {
            server
                .call("get_trip", json!({"trip_id": id, "detail": detail}))
                .await?
                .ok()
                .with_context(|| format!("trip {id}, detail {detail}"))?;
        }
    }
    server.ensure_no_secrets(
        keys.iter()
            .map(String::as_str)
            .chain([account.cookie.as_str()]),
    )?;
    eprintln!("rendered {} trips", ids.len());
    server.stop().await
}
