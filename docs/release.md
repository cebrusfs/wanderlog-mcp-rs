# Releasing

Both crates share the workspace version in `Cargo.toml`, and `wanderlog-mcp` requires exactly
that `wanderlog-client` version, so they are always released together. A `v<version>` tag starts
`.github/workflows/release.yml`, which checks, packages and drafts a GitHub Release; publishing
that draft publishes the crates.

Two actions cannot be undone, so they come last, after every check has passed:

- **Publishing to crates.io:** a version can only be yanked, never replaced or reused.
- **Publishing the GitHub Release:** with immutable releases, its tag and assets are frozen.

Everything before them, the tag and the draft included, can be deleted and redone.

## Steps

1. **Prepare.** Bump `version` in `Cargo.toml` and the `wanderlog-client` requirement in
   `crates/wanderlog-mcp/Cargo.toml`, run `cargo update --workspace` and `mise run ci`, commit,
   push to `main`, and wait for CI to pass.
2. **Tag the pushed commit.** Tag `main` as it is on GitHub, after pushing; a commit rewritten on
   push (for example signed) gets a new id.

   ```sh
   jj tag set v<version> -r main
   jj git push --tag v<version>
   ```

3. **Wait for the draft.** In the Actions tab, the Release run runs the full CI (including the
   [minimum Rust check](development.md#github-actions)), builds the desktop packages, checks that
   the tag names the packaged version and that `SHA256SUMS` matches, attests the packages' build
   provenance, and creates a **draft** release with the `.mcpb`, the OpenAI `.zip` and
   `SHA256SUMS`. If it fails, see [starting over](#starting-over).
4. **Publish the draft.** Review it on the Releases page and click **Publish release**. That
   starts the **Publish crates** job: it publishes `wanderlog-client`, then `wanderlog-mcp`,
   through crates.io Trusted Publishing (a short-lived OIDC token; no stored secret). Versions
   already on crates.io are skipped, so a rerun succeeds without publishing anything.
5. **Verify** the release:

   ```sh
   gh release verify v<version> -R cebrusfs/wanderlog-mcp-rs
   gh release download v<version> -R cebrusfs/wanderlog-mcp-rs -p '*.mcpb'
   gh attestation verify wanderlog-mcp-<version>-macos-arm64.mcpb -R cebrusfs/wanderlog-mcp-rs
   ```

## Starting over

Until the draft is published, a failed run or a wrong draft is redone from scratch: delete the
draft (if one was created) and the tag, fix the cause, and tag again.

```sh
gh release delete v<version> -R cebrusfs/wanderlog-mcp-rs --yes
jj tag delete v<version>
jj git push --deleted
```

Once a version is on crates.io, it cannot be published again: fix the problem in a new version
(and yank the broken one if needed).

## Reference: one-time setup

Done once for the repository and the crates; a normal release needs none of it.

- **Immutable releases:** enabled in the repository settings before the first release.
- **Optional:** restrict the `crates-io` environment to `v*` tags under **Settings →
  Environments**.

### The first crates.io release

crates.io accepts Trusted Publishing only for crates that already exist, so the first version is
published by hand, between steps 3 and 4: after the tag's checks pass, before the draft is
published (publishing it would make the job try Trusted Publishing for crates that do not exist).

1. Verify your email address on crates.io (Account Settings), which publishing requires.
2. Create an API token under **Account Settings → API Tokens** with the `publish-new` and
   `publish-update` scopes and a short expiry.
3. In your own terminal, publish from the tagged commit. `jj new` puts an empty working copy on
   it, so Cargo sees a clean tree and records the tagged commit:

   ```sh
   jj new v<version>
   read -rs CARGO_REGISTRY_TOKEN && export CARGO_REGISTRY_TOKEN   # paste the token; not echoed
   cargo publish --workspace --locked   # wanderlog-client first, then wanderlog-mcp
   unset CARGO_REGISTRY_TOKEN
   ```

4. Revoke the token under **API Tokens**.
5. For each crate on crates.io, add a Trusted Publisher under **Settings → Trusted Publishing**:
   owner `cebrusfs`, repository `wanderlog-mcp-rs`, workflow `release.yml`, environment
   `crates-io`.

Then publish the draft (step 4); the job finds both crates published and skips them.
