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

### GitHub Actions

**CI** (`.github/workflows/ci.yml`) runs on pushes, pull requests, and manual dispatches using a
macOS runner. It calls `mise run ci`, with no Wanderlog credentials or live API tests. Dependency
downloads remain available; there is no firewall or network sandbox.

**Desktop packages** (`.github/workflows/release.yml`) is prepared for future releases and is
**disabled by default**. To use it after the workflow reaches the default branch:

1. Set the repository Actions variable `ENABLE_RELEASE_WORKFLOW` to `true`.
2. Manually run **Desktop packages** with **publish** unchecked to run CI, build Apple Silicon
   packages, and upload the `.mcpb`, OpenAI `.zip`, and checksums as a workflow artifact.
3. When ready to publish, first create and push an existing `v<version>` tag matching
   `Cargo.toml`. Run the workflow from that tag with **publish** checked. After checks and
   packaging pass, it verifies the tag still points at the built commit and creates a GitHub
   Release with those assets. It does not create tags or overwrite an existing release.

Tag pushes alone do not publish. Binaries remain generated artifacts, not repository files.
The publishing job alone receives `contents: write`; checks and builds use read access.
