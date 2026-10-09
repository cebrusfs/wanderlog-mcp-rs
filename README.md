# Wanderlog MCP in Rust

[![CI](https://github.com/cebrusfs/wanderlog-mcp-rs/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/cebrusfs/wanderlog-mcp-rs/actions/workflows/ci.yml)
[![Coverage](https://codecov.io/gh/cebrusfs/wanderlog-mcp-rs/branch/main/graph/badge.svg)](https://codecov.io/gh/cebrusfs/wanderlog-mcp-rs)
[![Rust](https://img.shields.io/badge/Rust-000000?logo=rust&logoColor=white)](https://www.rust-lang.org/)
[![macOS](https://img.shields.io/badge/platform-macOS-lightgrey)](#install)

Connect your AI assistant to [Wanderlog](https://wanderlog.com): browse trips, find places,
and update itineraries from a conversation. A Rust [MCP](https://modelcontextprotocol.io)
server and CLI for Claude Code, Claude Desktop, Codex, and local Work / Codex conversations
in the ChatGPT desktop app.

**Unofficial.** This uses Wanderlog's private web API, which may change without notice.
Use it for your own trips, at human pace; automated use may be against Wanderlog's terms.

## Install

**macOS only.** Authentication uses macOS Keychain. Apple Silicon is verified; CI provides
`arm64` packages, and local builds use your Mac's architecture (`arm64` or `x64`). Windows and
Linux are not supported, including when supplying a cookie through an environment variable.
The [`wanderlog-client`](crates/wanderlog-client) library itself is portable.

Choose the setup for your client:

| Client | Setup |
| --- | --- |
| Claude Desktop Chat | [Install the extension package (recommended)](#desktop-packages-recommended) |
| ChatGPT desktop | [Install the local plugin package (recommended)](#desktop-packages-recommended) |
| Claude Code / Codex CLI / IDE extension | [Install with Cargo and register the server](#cli-installation-alternative) |

### Desktop packages (recommended)

Download **desktop-macos-arm64** from the **Artifacts** section of a successful
[main-branch CI run](https://github.com/cebrusfs/wanderlog-mcp-rs/actions/workflows/ci.yml?query=branch%3Amain).
GitHub requires signing in to download workflow artifacts; they are retained for seven days.
Extract the download to access the `.mcpb`, OpenAI `.zip`, and `SHA256SUMS` files. From that folder,
you can verify the archives with `shasum -a 256 -c SHA256SUMS`.

For Intel Macs or when no recent CI artifact is available, [build packages locally](#build-packages-locally).
The packages contain the server binary, so they need no Rust or Bun at runtime.

| Output | Use |
| --- | --- |
| `wanderlog-mcp-<version>-macos-<arch>.mcpb` | Claude Desktop extension |
| `wanderlog-mcp-<version>-macos-<arch>-openai.zip` | ChatGPT desktop local marketplace; extract before installing |

Move the packages you want to keep to a permanent location, such as `~/Applications/Wanderlog/`.
Keep the extracted marketplace folder intact.

#### Claude Desktop

1. Open **Settings → Extensions → Advanced settings → Install Extension**.
2. Select the `.mcpb` file for your Mac's architecture.
3. Leave **Wanderlog session cookie** blank to use your Keychain login, or supply a cookie as
   described under [Sign in](#sign-in).
4. Enable the extension and choose **Wanderlog** from **+ → Connectors** in a new chat.

See [Claude's local MCP guide](https://support.claude.com/en/articles/10949351-getting-started-with-local-mcp-servers-on-claude-desktop)
if your app's setup screen differs.

#### ChatGPT desktop

Open a terminal in the permanent `-openai/` marketplace folder (extract the ZIP first if needed):

```sh
codex plugin marketplace add "$PWD"
```

Restart ChatGPT desktop, open **Plugins Directory**, choose **Wanderlog local**, and install
**Wanderlog**. [Sign in](#sign-in), then start a new local Work / Codex conversation with the
plugin enabled. This requires the Codex CLI for marketplace registration.

For a repo-scoped installation instead, copy the generated `.agents/plugins/marketplace.json`
and `plugins/wanderlog-mcp/` to those same paths under the repository you open in ChatGPT desktop.
Merge an existing marketplace's `plugins` array rather than replacing it. In Finder,
**Command–Shift–.** shows `.agents`. Restart the app and install from Plugins Directory.
See OpenAI's [local marketplace guide](https://developers.openai.com/plugins/build/plugins#build-your-own-curated-plugin-list).

These packages run locally; they do not provide a remote endpoint for browser or mobile chat.
Client availability depends on your app version and workspace policy.

#### Build packages locally

Install [Rust](https://www.rust-lang.org/tools/install) (minimum version in `Cargo.toml`)
and [mise](https://mise.jdx.dev/getting-started.html). Mise installs the pinned Bun version
for packaging. Then run:

```sh
git clone https://github.com/cebrusfs/wanderlog-mcp-rs.git
cd wanderlog-mcp-rs
mise install
mise run package
```

The command prints `Artifacts: <output-directory>` and creates the packages and an extracted
`-openai/` marketplace folder under `$TMPDIR`. Move them to a permanent location before installing.
Building requires network access.

### CLI installation (alternative)

If you prefer an installed binary and manual MCP registration, install
[Rust](https://www.rust-lang.org/tools/install) (minimum version in `Cargo.toml`), then run:

```sh
git clone https://github.com/cebrusfs/wanderlog-mcp-rs.git
cd wanderlog-mcp-rs
cargo install --path crates/wanderlog-mcp --locked
```

This installs `wanderlog-mcp` into `~/.cargo/bin` by default and does not require Bun or mise.

For **Claude Code**:

```sh
claude mcp add --transport stdio --scope user wanderlog -- "$HOME/.cargo/bin/wanderlog-mcp" serve
claude mcp list
```

For **Codex CLI / IDE extension and local ChatGPT desktop conversations**:

```sh
codex mcp add wanderlog -- "$HOME/.cargo/bin/wanderlog-mcp" serve
codex mcp list
```

Codex and ChatGPT desktop share this [MCP configuration](https://learn.chatgpt.com/docs/extend/mcp).
If you installed the binary elsewhere, use its absolute path from `command -v wanderlog-mcp`.
Add `--read-only` after `serve` to disable writes. [Sign in](#sign-in), then start a new conversation.

## Sign in

With the CLI installed, run these commands in an interactive terminal:

```sh
wanderlog-mcp auth login
wanderlog-mcp auth status
```

With a desktop package instead, extract the OpenAI ZIP, open its `-openai/` folder, and replace `wanderlog-mcp` with
`./plugins/wanderlog-mcp/server/wanderlog-mcp`. This works for Claude Desktop users too.
The password input is hidden; only the verified session cookie is saved to Keychain.

To sign in with a browser cookie (including Google, Apple, or Facebook login):

1. Log in at <https://wanderlog.com> in Chrome.
2. Open DevTools → **Application → Cookies → https://wanderlog.com** and copy the `connect.sid` value.
3. Run `pbpaste | wanderlog-mcp auth set`, or `wanderlog-mcp auth set` for a hidden input prompt.

The Claude extension's optional cookie field uses Claude's secure storage and overrides Keychain
for that extension. Clear the field to use Keychain again; `auth clear` does not clear that field.
The `WANDERLOG_COOKIE` environment variable also takes precedence over Keychain.
When a session expires, repeat login or cookie import. `wanderlog-mcp auth clear` removes the
Keychain session; running MCP servers pick up a renewed session on their next call.

## Use it

Try asking your assistant:

- “List my Wanderlog trips and show the itinerary for my Tokyo trip.”
- “Find ramen places near this trip's destination.”
- “Preview adding Tokyo Tower to the second day, then wait for my confirmation.”

You can also browse from the terminal:

```sh
wanderlog-mcp trips
wanderlog-mcp show <trip-id>
```

Lodging, flights, transit reservations, budget expenses, and the visited journal are read-only.
New trips use Wanderlog's default sharing level, **friends**; change it in the app if needed.
See the [tool reference](docs/tools.md) for supported edits and the safety model.

## Troubleshooting

- **Not logged in:** repeat [Sign in](#sign-in).
- **Keychain access prompt:** allow the server to access its stored session; reinstalling can trigger another prompt.
- **Rate limited:** wait for the reported retry delay, or at least a minute if none is given;
  batch related edits together.
- **Trip changed since preview:** ask for a new preview before applying edits.
- **Edit outcome unknown:** check the trip before retrying to avoid duplicate changes.

## Documentation

- [Tool reference and safety model](docs/tools.md)
- [Development, checks, and releases](docs/development.md)
- [Desktop packaging reference](docs/desktop.md)
- [Wanderlog protocol reference](docs/protocol.md)
