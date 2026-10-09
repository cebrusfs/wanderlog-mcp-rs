//! Unofficial Rust API for Wanderlog's private web protocol.
//!
//! REST reads, ShareDB, models and edit planning without an MCP runtime,
//! CLI parser, or OS credential store. Credentials are supplied by callers.
//! The code license does not grant upstream service or trademark rights.
//!
//! ```no_run
//! use wanderlog_client::{Client, auth};
//! # async fn example() -> anyhow::Result<()> {
//! let cookie = auth::normalize(&std::env::var("WANDERLOG_COOKIE")?)?;
//! let client = Client::new(&cookie)?;
//! for trip in client.trips().await? {
//!     // Trip keys are bearer secrets; never print trip.key.
//!     println!("{}: {}", trip.id, trip.title);
//! }
//! # Ok(())
//! # }
//! ```
//!
//! Low-level writes do not automatically inherit MCP-level redaction,
//! read-only mode, duplicate-write detection or user confirmation behavior.
//! Handle revision conflicts and uncertain outcomes without blindly retrying.

pub mod auth;
pub mod dates;
pub mod edit;
pub mod json0;
pub mod render;
pub mod rest;
pub mod sharedb;
pub mod trip;

#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
pub mod test_support;

pub use rest::{RateLimited, Rest, Rest as Client, TripSummary};
pub use sharedb::{Connection, OutcomeUnknown, Snapshot};

/// Identifies this library to the upstream service without impersonating browsers.
pub const USER_AGENT: &str = concat!("wanderlog-client/", env!("CARGO_PKG_VERSION"));
