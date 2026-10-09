//! Local stdio MCP and credential adapters; legacy client paths remain re-exported.
pub mod auth;
pub mod server;
pub use wanderlog_client::{USER_AGENT, dates, edit, json0, render, rest, sharedb, trip};
#[cfg(test)]
mod test_support;
