//! Local stdio MCP, CLI and operating-system credential persistence.
//! Prefer `wanderlog-client` for programmatic access.
//! Re-exports preserve legacy public module paths.

pub mod auth;
pub mod server;

pub use wanderlog_client::{USER_AGENT, dates, edit, json0, render, rest, sharedb, trip};
