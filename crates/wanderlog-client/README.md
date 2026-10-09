# wanderlog-client

Unofficial Rust client for Wanderlog's private REST API and ShareDB WebSocket protocol:
trip models, agent-oriented rendering, JSON0 edits and atomic itinerary planning. It has no
MCP runtime, CLI or credential store; callers supply the `connect.sid` session and own its
storage. The optional `schema` feature derives JSON Schema for the edit types.

```rust
use wanderlog_client::{rest::Rest, sharedb::Snapshot};
let client = Rest::new("")?;
let snapshot = Snapshot { version: 3, doc: serde_json::json!({}) };
snapshot.validate_revision(3)?;
# Ok::<(), anyhow::Error>(())
```

- `Rest::with_base_url`, `Rest::with_clock`, `Connection::open_url` and `edit::plan_with_rng`
  accept explicit endpoints, clocks and randomness for deterministic tests. Custom endpoints
  receive the supplied credentials; TLS is required except on loopback hosts.
- `OutcomeUnknown` means a write may have been applied: read the trip again before retrying.
  `RevisionConflict` means the trip changed since the revision the caller planned against.

The protocol is private and may change without notice; see the
[protocol notes](https://github.com/cebrusfs/wanderlog-mcp-rs/blob/main/docs/protocol.md).
This project is not affiliated with Wanderlog.
