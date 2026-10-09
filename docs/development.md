# Development

For installation and client setup, use the [README](../README.md#install).
Run development tasks from a source checkout. The pinned tools and canonical tasks live
in `mise.toml` at the repository root.

## Architecture

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
mise run check         # cargo fmt --check, clippy -D warnings, unit/doc tests
mise run coverage      # unit/mock LCOV report; requires llvm-tools-preview
mise run package       # macOS packages + archive/stdio smoke checks; requires Bun and network
```

Unit tests cover the json0 engine, the edit planner (component shapes mirror captured web-client
ops), rendering, and the tool schemas (flat JSON objects, no `$ref`/`oneOf`, for OpenAI clients).
HTTP and WebSocket tests use local mock servers. `check` selects library, binary, and doc tests
explicitly; it never runs the live integration target, even if its `#[ignore]` marker changes.
For optional live validation, set `WANDERLOG_E2E_TRIP_ID` to your own throwaway trip with dates,
authenticate locally, and run `mise run e2e`. That task talks to Wanderlog and restores the trip
after its round-trip; it is never invoked by CI.

The [packaging guide](desktop.md#build-and-verify) describes artifact verification separately
from live account and desktop UI testing.

### Coverage

Install the active Rust toolchain's coverage component with
`mise exec -- rustup component add llvm-tools-preview`, then run `mise run coverage`.
Mise pins `cargo-llvm-cov`; the task selects only library and binary unit/mock tests, excluding
live integration tests and doctests. Source paths are relative to the repository so reports are
portable between runners. It writes line coverage to `$TMPDIR/wanderlog-mcp-lcov.info`.
Set `COVERAGE_FILE` to override the report path. No minimum coverage percentage is enforced.

### GitHub Actions

**CI** (`.github/workflows/ci.yml`) runs on pushes, pull requests, and manual dispatches using a
macOS runner. It calls `mise run ci`, with no Wanderlog credentials or live API tests. Dependency
downloads remain available; there is no firewall or network sandbox. After checks pass, it runs
`mise run package` and uploads Apple Silicon `.mcpb`, OpenAI `.zip`, and SHA256 checksums as
**desktop-macos-arm64** for seven days. See the [README](../README.md#desktop-packages-recommended)
for downloading and installing these packages.

**Coverage** (`.github/workflows/coverage.yml`) runs `mise run coverage` on pushes, pull requests,
and manual dispatches, retaining **coverage-lcov** for seven days. Successful main-branch runs
upload that report to [Codecov](https://codecov.io/gh/cebrusfs/wanderlog-mcp-rs). Connect the public
repository in Codecov before the first upload; no `CODECOV_TOKEN` secret is needed because the
upload uses [GitHub OIDC](https://github.com/codecov/codecov-action#using-oidc). Only the main-branch
upload job receives `id-token: write`; test jobs use read access, and pull requests do not upload.
The README badge shows main-branch line coverage after Codecov processes the first report.
Upload failures fail the Coverage workflow; they do not block the separate CI/package workflow.

**Desktop packages** (`.github/workflows/release.yml`) reuses CI's checks and packages for manual
releases. This manual workflow remains **disabled by default**. To use it after the workflow
reaches the default branch:

1. Set the repository Actions variable `ENABLE_RELEASE_WORKFLOW` to `true`.
2. Manually run **Desktop packages** with **publish** unchecked to run CI, build Apple Silicon
   packages, and upload the same workflow artifact as ordinary CI.
3. When ready to publish, first create and push an existing `v<version>` tag matching
   `Cargo.toml`. Run the workflow from that tag with **publish** checked. After checks and
   packaging pass, it verifies the tag still points at the built commit and creates a GitHub
   Release with those assets. It does not create tags or overwrite an existing release.

Tag pushes alone do not publish. Binaries remain generated artifacts, not repository files.
The publishing job alone receives `contents: write`; checks and builds use read access.

## Rust workspace and releases

`wanderlog-client` (`wanderlog_client`) provides the reusable API without MCP or
Keychain. `wanderlog-mcp` retains the local stdio server, CLI and credentials.
The repository and executable names remain unchanged; legacy module paths are
re-exported. New Rust consumers should use the client crate directly.

Both crates use Apache-2.0; see LICENSE and NOTICE. This grants code rights,
not upstream service, data, trademark or official affiliation rights. See
[release instructions](https://github.com/cebrusfs/wanderlog-mcp-rs/blob/main/docs/publishing.md),
CONTRIBUTING.md, SECURITY.md and PRIVACY.md. A Cargo name is not a registry
reservation. Remote MCP is out of scope. No release is triggered by a push.
