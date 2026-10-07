//! `wanderlog-mcp`: MCP server (stdio) and CLI for editing Wanderlog trips with AI agents.

use std::io::{IsTerminal, Read};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use serde_json::Value;

use wanderlog_mcp::server::{Detail, TripArgs, WanderlogServer, serve_stdio};
use wanderlog_mcp::{auth, rest::Rest};

#[derive(Parser)]
#[command(
    name = "wanderlog-mcp",
    version,
    about = "Let AI agents read and edit your Wanderlog trips (MCP server + CLI)"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run the MCP server on stdin/stdout (this is what MCP clients launch).
    Serve {
        /// Disable every tool that writes to Wanderlog.
        #[arg(long)]
        read_only: bool,
    },
    /// Manage the Wanderlog session cookie kept in the OS credential store.
    Auth {
        #[command(subcommand)]
        action: AuthAction,
    },
    /// List your trips.
    Trips,
    /// Print a trip exactly as agents see it.
    Show {
        trip_id: u64,
        /// Include place_id and address for each place.
        #[arg(long)]
        full: bool,
    },
}

#[derive(Subcommand)]
enum AuthAction {
    /// Read the `connect.sid` cookie value from stdin, verify it logs in, then store it.
    Set,
    /// Check whether a working session is configured.
    Status,
    /// Delete the stored cookie.
    Clear,
}

#[tokio::main]
async fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Serve { read_only } => serve_stdio(read_only).await,
        Command::Auth {
            action: AuthAction::Set,
        } => auth_set().await,
        Command::Auth {
            action: AuthAction::Status,
        } => auth_status().await,
        Command::Auth {
            action: AuthAction::Clear,
        } => {
            let removed = auth::clear()?;
            println!(
                "{}",
                if removed {
                    "Stored Wanderlog session deleted."
                } else {
                    "No stored session."
                }
            );
            Ok(())
        }
        Command::Trips => {
            let server = WanderlogServer::new(|| auth::load().map(|(cookie, _)| cookie), true);
            // Redacted like MCP output: agents can run this CLI through a shell.
            let text = server.list_trips_impl().await?;
            print!("{}", server.redact(text).await);
            Ok(())
        }
        Command::Show { trip_id, full } => {
            let server = WanderlogServer::new(|| auth::load().map(|(cookie, _)| cookie), true);
            let detail = Some(if full { Detail::Full } else { Detail::Compact });
            let text = server.get_trip_impl(TripArgs { trip_id, detail }).await?;
            print!("{}", server.redact(text).await);
            Ok(())
        }
    }
}

async fn auth_set() -> Result<()> {
    let interactive = std::io::stdin().is_terminal();
    let raw = if interactive {
        eprintln!("Tip: `pbpaste | wanderlog-mcp auth set` also works.");
        // rpassword reads from the TTY with echo off and errors out if it cannot hide the input,
        // so the cookie is never shown on screen.
        rpassword::prompt_password(
            "Paste the value of the wanderlog.com `connect.sid` cookie (hidden): ",
        )
        .context(
            "cannot read a hidden value from this terminal; use `pbpaste | wanderlog-mcp auth set`",
        )?
    } else {
        let mut buf = String::new();
        std::io::stdin().read_to_string(&mut buf)?;
        buf
    };
    let cookie = auth::normalize(&raw)?;
    let user = Rest::new(&cookie)?
        .current_user()
        .await?
        .context("Wanderlog does not accept this cookie as a logged-in session (wrong value, or logged out?)")?;
    auth::store(&cookie)?;
    println!(
        "Saved to the OS credential store. Logged in as {}.",
        username(&user)
    );
    println!("The cookie is probably still on your clipboard: clear it with `pbcopy < /dev/null`.");
    Ok(())
}

async fn auth_status() -> Result<()> {
    let (cookie, source) = match auth::load() {
        Ok(found) => found,
        Err(e) => {
            println!("Not configured: {e:#}");
            return Ok(());
        }
    };
    let from = match source {
        auth::Source::Env => format!("the {} environment variable", auth::ENV_VAR),
        auth::Source::Keychain => "the OS credential store".to_owned(),
    };
    match Rest::new(&cookie)?.current_user().await? {
        Some(user) => println!(
            "OK: logged in as {} (session from {from}).",
            username(&user)
        ),
        None => println!(
            "The session from {from} is no longer logged in. Run `wanderlog-mcp auth set` with a fresh cookie."
        ),
    }
    Ok(())
}

fn username(user: &Value) -> String {
    let field = |k: &str| {
        user.get(k)
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
    };
    field("username")
        .or_else(|| field("name"))
        .unwrap_or("(unknown)")
        .to_owned()
}
