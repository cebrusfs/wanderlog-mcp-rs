# Releasing

Both crates share the workspace version in `Cargo.toml`, and `wanderlog-mcp` requires exactly
that `wanderlog-client` version, so they are always released together. Releases come from
`.github/workflows/release.yml`; nothing is published without a maintainer publishing a draft.

## Steps

1. Bump `version` in `Cargo.toml` and the `wanderlog-client` requirement in
   `crates/wanderlog-mcp/Cargo.toml`, run `cargo update --workspace` and `mise run ci`, and commit.
2. Tag that commit `v<version>` and push the tag, e.g.
   `git tag v0.2.0 <commit> && git push origin v0.2.0`.
3. The workflow runs the full CI, builds the desktop packages, checks that the tag names the
   packaged version and that `SHA256SUMS` matches, attests the packages' build provenance, and
   creates a **draft** release with the `.mcpb`, the OpenAI `.zip` and `SHA256SUMS`.
4. Review the draft and click **Publish release**. With immutable releases enabled, the tag and
   assets can no longer change.
5. Publishing starts the **Publish crates** job: it publishes `wanderlog-client`, then
   `wanderlog-mcp`, through crates.io Trusted Publishing (a short-lived OIDC token; no stored
   secret). Versions already on crates.io are skipped, so the job can be rerun safely.

If the draft job fails, delete the draft and the tag, fix the cause and tag again. A published
version number is never reused: crates.io versions can only be yanked, and immutable releases keep
their tag.

## One-time setup

- **Immutable releases:** enable them in the repository settings before the first release.
- **First crates.io release:** crates.io only accepts Trusted Publishing for crates that already
  exist, so publish the first version by hand from the tagged commit, before publishing the draft:

  ```sh
  cargo publish --locked -p wanderlog-client
  cargo publish --locked -p wanderlog-mcp
  ```

  Use a crates.io API token scoped to publishing and revoke it afterwards. Then, for each crate on
  crates.io, add a Trusted Publisher under **Settings → Trusted Publishing**: owner `cebrusfs`,
  repository `wanderlog-mcp-rs`, workflow `release.yml`, environment `crates-io`. Publishing the
  draft afterwards skips both crates.
- **Optional:** restrict the `crates-io` environment to `v*` tags under **Settings →
  Environments**.

## Verifying a release

```sh
gh release verify v<version> -R cebrusfs/wanderlog-mcp-rs
gh attestation verify wanderlog-mcp-<version>-macos-arm64.mcpb -R cebrusfs/wanderlog-mcp-rs
```
