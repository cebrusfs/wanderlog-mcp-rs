# Package publishing

## Names and scope

| Package | Import | Purpose |
| --- | --- | --- |
| wanderlog-client | wanderlog_client | REST/ShareDB, models and edit planner |
| wanderlog-mcp | wanderlog_mcp | Local stdio MCP, CLI and macOS credentials |

Keep repository `cebrusfs/wanderlog-mcp-rs` and executable `wanderlog-mcp`.
Initially release both crates together with matching versions; MCP uses an exact
client requirement. Both crates use Apache-2.0. This is not a grant of upstream
API, service, data, trademark or official-branding rights. Remote MCP is deferred.

## Validation

```sh
mise install
mise run ci
cargo test --workspace --locked
cargo test -p wanderlog-client --locked --no-default-features
cargo doc --workspace --locked --no-deps
python3 scripts/release-check.py
cargo tree -p wanderlog-client --edges normal
cargo package --locked -p wanderlog-client
cargo package --list --locked -p wanderlog-mcp
```

The standalone client's normal dependency tree must not include rmcp, clap,
keyring or rpassword. Workspace feature unification must not hide missing
standalone features. Client portability does not change the application's
macOS support boundary. Ordinary tests must not need live credentials.

Before the exact client version exists on crates.io, verified MCP packaging or
publish dry-run may be blocked by registry resolution. Package listing is not
proof that the verified package passed. Do not substitute `--no-verify` for a
successful verified dry-run.

## First release and partial recovery

Check both names on crates.io immediately before uploading. A manifest does not
reserve a name. Verify ownership, licensing, upstream terms, CI, clean checkout
and intended commit. Publication is immutable; never blindly retry an upload
with an uncertain result. Inspect the registry before resuming a partial release.

```sh
cargo publish --dry-run --locked -p wanderlog-client
cargo publish --locked -p wanderlog-client
# Wait for that exact version to appear in the registry index.
cargo publish --dry-run --locked -p wanderlog-mcp
cargo publish --locked -p wanderlog-mcp
```

Use scoped, short-lived credentials in a trusted environment. Never commit a
token or paste it into chat, issues or PRs. Revoke bootstrap credentials after use.

## Manual Actions workflow

`crates.yml` defaults to validation only. Actual publication requires all of:
`publish=true`, repository variable `ENABLE_CRATES_IO_PUBLISH=true`, an existing
matching version tag on main history, and the `crates-io` GitHub environment with
a scoped `CARGO_REGISTRY_TOKEN` secret. Configure required reviewers and tag
restrictions on this environment before enabling publication. These external
settings are not created by committing workflow files.

Choose `both`, `client` or `mcp`. Resume with `mcp` only after verifying that the
matching client version is published and indexed. Uploads are never blindly
retried. The desktop GitHub Release workflow is separate and retains its existing
explicit opt-in. Ordinary pushes and PRs publish nothing.

Later, replace stored tokens with crates.io trusted publishing/OIDC after
checking current eligibility and registering each crate's publisher. No OIDC
identity is registered automatically here.

For a new version, update both manifests and both MCP client requirements,
refresh Cargo.lock without unrelated upgrades, update CHANGELOG and run checks.
Only create a matching `vX.Y.Z` tag on a reviewed commit when ready to release.

References:
- https://doc.rust-lang.org/cargo/reference/publishing.html
- https://doc.rust-lang.org/cargo/commands/cargo-package.html
- https://www.apache.org/licenses/LICENSE-2.0
