//! Unofficial REST/ShareDB client. Callers supply sessions; no CLI or credential persistence.
//! The code license supplies no service access or trademark authorization.
//!
//! ```
//! use wanderlog_client::{edit,trip};
//! let doc = serde_json::json!({"itinerary":{"sections":[]}});
//! assert!(trip::sections(&doc).is_empty());
//! assert!(edit::check_batch(&[]).is_err());
//! ```
pub mod clock;
pub mod dates;
pub mod edit;
pub mod errors;
pub mod json0;
pub mod render;
pub mod rest;
pub mod session;
pub mod sharedb;
pub mod trip;
pub const USER_AGENT: &str = concat!("wanderlog-client/", env!("CARGO_PKG_VERSION"));
