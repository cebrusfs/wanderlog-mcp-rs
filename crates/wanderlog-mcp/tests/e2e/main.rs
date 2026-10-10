//! Live tests against wanderlog.com with the stored session (or `WANDERLOG_COOKIE`). Ignored by
//! default; run with `mise run e2e`, which runs them one at a time. Each writing test creates its
//! own trip, titled `wanderlog-mcp e2e <pid>`, then moves it to the trash and deletes it
//! permanently, also when the test fails; only a panic or a killed run leaves one behind.

mod client;
mod mcp;

use std::sync::OnceLock;

use anyhow::{Context, Result, anyhow, ensure};
use reqwest::header::{COOKIE, ORIGIN};
use serde_json::{Value, json};
use wanderlog_mcp::rest::{BASE, Rest};
use wanderlog_mcp::{USER_AGENT, auth};

/// First and last day of every test trip; any fixed dates work.
const START: &str = "2030-03-01";
const END: &str = "2030-03-02";

fn title() -> String {
    format!("wanderlog-mcp e2e {}", std::process::id())
}

/// The stored session, read once per run so the Keychain asks at most once.
fn cookie() -> Result<String> {
    static COOKIE: OnceLock<Result<String, String>> = OnceLock::new();
    COOKIE
        .get_or_init(|| auth::load().map(|(c, _)| c).map_err(|e| format!("{e:#}")))
        .clone()
        .map_err(|e| anyhow!(e))
}

struct Account {
    cookie: String,
    rest: Rest,
    user_id: u64,
}

impl Account {
    async fn load() -> Result<Self> {
        let cookie = cookie()?;
        let rest = Rest::new(&cookie)?;
        let user = rest
            .current_user()
            .await?
            .context("stored session is not logged in")?;
        Ok(Self {
            user_id: user["id"].as_u64().context("user without id")?,
            cookie,
            rest,
        })
    }

    /// Edit key of one of the account's own trips. Never print it: keys are bearer credentials.
    async fn own_key(&self, trip_id: u64) -> Result<String> {
        self.rest
            .trips()
            .await?
            .into_iter()
            .find(|t| t.id == trip_id && t.relation == "own")
            .map(|t| t.key)
            .with_context(|| format!("trip {trip_id} is not one of this account's trips"))
    }

    /// Run `test`, then delete the trip whatever `test` returned.
    async fn delete_after<T>(&self, key: &str, test: impl Future<Output = Result<T>>) -> Result<T> {
        let result = test.await;
        match (result, self.delete_trip(key).await) {
            (result, Ok(())) => result,
            (Ok(_), Err(e)) => Err(e.context("deleting the e2e trip failed")),
            (Err(e), Err(d)) => Err(e.context(format!("deleting the e2e trip also failed: {d:#}"))),
        }
    }

    /// Move a trip to the trash and delete it from there, as the web app does (docs/protocol.md).
    /// Test-only on purpose: neither the library nor the tools can delete trips.
    async fn delete_trip(&self, key: &str) -> Result<()> {
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .referer(false)
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
        for (what, request) in [
            (
                "move to trash",
                http.delete(format!("{BASE}/api/tripPlans/{key}")),
            ),
            (
                "delete from trash",
                http.post(format!("{BASE}/api/tripPlans/deleteFromTrash"))
                    .json(&json!({"keys": [key]})),
            ),
        ] {
            let response = request
                .header(COOKIE, format!("connect.sid={}", self.cookie))
                .header(ORIGIN, BASE)
                .send()
                .await
                .map_err(|e| anyhow!("{what}: {}", e.without_url()))?;
            let status = response.status();
            let body: Value = response.json().await.unwrap_or_default();
            ensure!(
                status.is_success() && body["success"] == true,
                "{what}: HTTP {status}"
            );
        }
        ensure!(
            !self.rest.trips().await?.iter().any(|t| t.key == key),
            "trip still listed after deletion"
        );
        Ok(())
    }
}
