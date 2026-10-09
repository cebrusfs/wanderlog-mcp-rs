# Desktop packaging reference

One release build produces two client adapters: a Claude Desktop extension (`.mcpb`) and an
OpenAI local plugin marketplace (`-openai.zip`). Follow the
[README installation steps](../README.md#install) for prerequisites, the local build command,
authentication, client installation, and platform limitations. That is the canonical user guide.
There is no separate skill to maintain: the server already supplies its tools and workflow
instructions over MCP.

## Package layout

The OpenAI archive contains a marketplace root:

```text
<marketplace-root>/
  .agents/plugins/marketplace.json
  plugins/wanderlog-mcp/
    plugin.json
    mcp.json
    server/wanderlog-mcp
  README.md
  docs/
```

The Claude archive contains `manifest.json` and `server/wanderlog-mcp`. Its entry point uses
the host's `${__dirname}` substitution. The OpenAI `mcp.json` instead uses a plugin-relative
`./server/wanderlog-mcp` command, as required by the Agent Plugins specification. Neither
adapter includes a credential or the build machine's absolute path.

## Build and verify

`crates/wanderlog-mcp/Cargo.toml` owns the name and description; the workspace `Cargo.toml` owns the version. `scripts/package.mjs` generates both client
manifests, derives Claude's tool metadata from the compiled server, and copies that one binary.
It validates MCPB with the pinned official packer and OpenAI manifests against the versioned
[Agent Plugins schemas](https://agent-plugins.org/specification). Packaging requires network
access to fetch those schemas and any missing locked dependencies.

Verification re-extracts both archives into paths containing spaces, checks executable
permissions and binary equality, resolves Claude's empty and supplied cookie settings with its
official library, and compares MCP initialization and tool discovery against the release binary.
Smoke processes receive a dummy cookie and never call Wanderlog tools or access the Keychain.
This does not prove successful account login or installation in the clients' actual UI; those
remain manual checks. Packages are generated locally or by CI; they are not notarized or
public marketplace submissions. macOS or an organization's extension policy may block them.
