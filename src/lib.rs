//! Let AI agents read and edit Wanderlog trips.
//!
//! Wanderlog has no public API. This crate speaks the private API of its web app, as observed in
//! live browser traffic: REST for reads and search, and ShareDB (json0 OT over a WebSocket) for
//! every itinerary edit. See docs/protocol.md.

pub mod auth;
pub mod dates;
pub mod edit;
pub mod json0;
pub mod render;
pub mod rest;
pub mod server;
pub mod sharedb;
pub mod trip;

/// User-Agent for requests to Wanderlog: identifies this tool instead of impersonating a browser.
pub const USER_AGENT: &str = concat!("wanderlog-mcp/", env!("CARGO_PKG_VERSION"));
