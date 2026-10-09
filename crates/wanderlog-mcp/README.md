# wanderlog-mcp

Local stdio MCP server and CLI that let AI agents read and edit your Wanderlog trips, built on
[`wanderlog-client`](https://crates.io/crates/wanderlog-client).

```sh
cargo install wanderlog-mcp --locked
wanderlog-mcp auth login
wanderlog-mcp serve            # or: serve --read-only
```

The session is stored in the macOS Keychain; other platforms read `WANDERLOG_COOKIE`. Edits are
atomic per call, checked against the trip revision, and guarded against blind retries.

Setup for Claude, ChatGPT and Codex, the tool reference and the safety model are in the
[project README](https://github.com/cebrusfs/wanderlog-mcp-rs#readme). This project is not
affiliated with Wanderlog.
