# wanderlog-client

Unofficial low-level Rust client for Wanderlog REST/ShareDB and itinerary editing.
The reusable core of https://github.com/cebrusfs/wanderlog-mcp-rs without an MCP
runtime, CLI parser, or operating-system credential store.

```rust,no_run
use wanderlog_client::{auth, Client};

# async fn example() -> anyhow::Result<()> {
let cookie = auth::normalize(&std::env::var("WANDERLOG_COOKIE")?)?;
let client = Client::new(&cookie)?;
for trip in client.trips().await? {
    println!("{}: {}", trip.id, trip.title); // Never print trip.key.
}
# Ok(())
# }
```

`Client` aliases `rest::Rest`. Modules `sharedb`, `edit`, `json0`, `trip`, `render`
and `dates` expose the existing transport, planner and utilities. Cookie
normalization does not load or store credentials. The off-by-default
`test-support` feature is internal and not part of the stable API.

Direct writes do not include the MCP server's full safety policy. Check
permissions, revisions and uncertain outcomes; never blindly replay writes.

Package: `wanderlog-client`; Rust import: `wanderlog_client`. Use a reviewed Git
revision or local path until the first crates.io publication. A manifest name
does not reserve that name in the registry.

Apache-2.0 (LICENSE and NOTICE). Not affiliated with or endorsed by Wanderlog.
The code license does not authorize upstream API access, third-party data use,
or official branding. The private protocol can change without notice.
