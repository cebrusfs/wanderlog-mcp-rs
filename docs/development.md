# Development

For installation and client setup, use the [README](../README.md#install).
Run development tasks from a source checkout. The pinned tools and canonical tasks live
in `mise.toml` at the repository root.

## Architecture

The repository is a Cargo workspace with two crates:

- `crates/wanderlog-client`: REST and ShareDB clients, trip models, rendering, JSON0 and edit
  planning. Callers supply the session; it has no MCP, CLI or credential-store dependency.
- `crates/wanderlog-mcp`: the stdio MCP server, the CLI and Keychain session storage.

- **Reads** use Wanderlog's REST endpoints (trip list, trip, place search/details).
- **Edits** go through ShareDB, the realtime engine behind Wanderlog's live collaboration: each
  `apply_edits` call becomes **one atomic revision**, built from a fresh snapshot. Tripmates see it
  live, exactly as if you had edited in the app.
- The agent sees a compact view of each trip with stable refs: `[s:<id>]` for sections (notes,
  lists, days) and `[b:<id>]` for items, plus 1-based positions used by the edit tools.

See the [tool reference](tools.md) for edit semantics and output safeguards, and the
[protocol reference](protocol.md) for the observed Wanderlog API and verification limits.

## Checks

```sh
mise run ci            # workflow lint + the project checks below; no live API tests
mise run check         # cargo fmt --check, clippy -D warnings, workspace tests and docs
mise run coverage      # unit/mock LCOV report; requires llvm-tools-preview
mise run package       # macOS packages + archive/stdio smoke checks; requires Bun and network
```

Unit tests cover the json0 engine, the edit planner (component shapes mirror captured web-client
ops), rendering, and the tool schemas (flat JSON objects, no `$ref`/`oneOf`, for OpenAI clients).
HTTP and WebSocket tests use local mock servers, and a CLI test drives the built binary over stdio
with a synthetic cookie. The live integration target requires the `live-tests` feature, so `check`
never builds it; the Keychain round trip (synthetic account) requires `native-keychain-tests`.
For optional live validation, set `WANDERLOG_E2E_TRIP_ID` to your own throwaway trip with dates,
authenticate locally, and run `mise run e2e`. That task talks to Wanderlog and restores the trip
after its round-trip; it is never invoked by CI.

The [packaging guide](desktop.md#build-and-verify) describes artifact verification separately
from live account and desktop UI testing.

### Coverage

Install the active Rust toolchain's coverage component with
`mise exec -- rustup component add llvm-tools-preview`, then run `mise run coverage`.
Mise pins `cargo-llvm-cov`; the task runs the workspace's unit, mock and CLI tests (never the
live target) and reports production code only, excluding test files. Source paths are relative to
the repository so reports are portable between runners. It writes line coverage to
`$TMPDIR/wanderlog-mcp-lcov.info` and fails below 90% line coverage. Set `COVERAGE_FILE` to
override the report path.

### GitHub Actions

**CI** (`.github/workflows/ci.yml`) runs on branch pushes, pull requests, and manual dispatches using a
macOS runner. It calls `mise run ci` and the Keychain round trip, with no Wanderlog credentials or
live API tests. A Linux and Windows job checks `wanderlog-client` on its own and fails if MCP, CLI
or credential crates leak into its dependency tree. Dependency downloads remain available; there
is no firewall or network sandbox. After checks pass, it runs
`mise run package` and uploads Apple Silicon `.mcpb`, OpenAI `.zip`, and SHA256 checksums as
**desktop-macos-arm64** for seven days. See the [README](../README.md#desktop-packages-recommended)
for downloading and installing these packages.

**Coverage** (`.github/workflows/coverage.yml`) runs `mise run coverage` on branch pushes, pull requests,
and manual dispatches, retaining **coverage-lcov** for seven days. Successful main-branch runs
upload that report to [Codecov](https://codecov.io/gh/cebrusfs/wanderlog-mcp-rs). Connect the public
repository in Codecov before the first upload; no `CODECOV_TOKEN` secret is needed because the
upload uses [GitHub OIDC](https://github.com/codecov/codecov-action#using-oidc). Only the main-branch
upload job receives `id-token: write`; test jobs use read access, and pull requests do not upload.
The README badge shows main-branch line coverage after Codecov processes the first report.
Upload failures fail the Coverage workflow; they do not block the separate CI/package workflow.

**Release** (`.github/workflows/release.yml`) runs on `v*` tags: it reuses CI, attests the
packages and drafts a GitHub Release; publishing the draft publishes the crates. See
[releasing](release.md). Only its release jobs receive write or OIDC permissions.
