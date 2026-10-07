# wanderlog-mcp

Let AI agents read and edit your [Wanderlog](https://wanderlog.com) trips. One Rust binary that is
both an [MCP](https://modelcontextprotocol.io) server (stdio) and a small CLI. Connect it to local
MCP clients such as Claude Code, Claude Desktop Chat, Codex CLI / IDE extension, and local
Work / Codex conversations in the ChatGPT desktop app.

> **Unofficial.** Wanderlog has no public API. This tool speaks the private API of the Wanderlog
> web app as observed in its own browser traffic ([docs/protocol.md](docs/protocol.md)). It can
> break whenever Wanderlog changes, and automated use may be against Wanderlog's terms. Use it for
> your own trips, at human pace.

## How it works

- **Reads** use Wanderlog's REST endpoints (trip list, trip, place search/details).
- **Edits** go through ShareDB, the realtime engine behind Wanderlog's live collaboration: each
  `apply_edits` call becomes **one atomic revision**, built from a fresh snapshot. Tripmates see it
  live, exactly as if you had edited in the app.
- The agent sees a compact view of each trip with stable refs: `[s:<id>]` for sections (notes,
  lists, days) and `[b:<id>]` for items, plus 1-based positions used by the edit tools.

## Safety model

- **Your session stays local.** The `connect.sid` cookie lives in the macOS Keychain (or the
  `WANDERLOG_COOKIE` env var) and is never shown to the model or written to logs.
- **Trip keys never reach the model.** Wanderlog trip keys work like passwords (anyone holding an
  edit key can open the trip), so the agent only ever sees numeric trip ids.
- **Trip text is treated as data.** Notes, names and headings may be written by tripmates or third
  parties: free text is wrapped in `«…»` (our delimiters neutralised, invisible/bidi characters
  stripped), other values (dates, times, codes) appear plain only when well-formed and are quoted
  otherwise, and the server tells the model that quoted text is data, never instructions. As a last
  step every tool output is scrubbed of trip keys, the session cookie and keys inside Wanderlog
  share links.
- **Writes are explicit.** Read tools carry the read-only annotation and `apply_edits` the
  destructive one, so clients that honour MCP annotations ask before running it (Claude Code asks
  per tool unless you allow it). The intended flow is `preview_edits` (dry run, shows the exact
  changes and a `base_revision`) → the user agrees to the goal → one `apply_edits` call for the
  whole batch with that `base_revision`. If the trip changed in between — a tripmate's edit, or an
  earlier attempt of the same call — nothing is applied, so retries cannot double-apply.
- Reservations (lodging, flights, transit) are read-only; `create_trip` always uses Wanderlog's
  default sharing level ("friends").
- `serve --read-only` disables every write tool.

## Install

**Currently macOS only.** The supported authentication workflow depends on macOS Keychain.
Windows / Linux credential integration and packaging have not been made portable. Supplying
`WANDERLOG_COOKIE` does not make those platforms supported. Packages contain a native binary for
one CPU architecture (`arm64` or `x64`), not a universal binary; Apple Silicon is the verified build.

### 1. Build the desktop packages locally

Install **Rust 1.89+**, **Bun**, and **mise**, then open a terminal in this repository's root:

```sh
mise run package
```

This shortcut compiles the Rust server on your Mac, packages it for both clients, and verifies
the extracted packages. Binaries are generated locally, not committed to this repository.
The command prints `Artifacts: <output-directory>` and creates these files under `$TMPDIR`:

| Output | Use |
| --- | --- |
| `wanderlog-mcp-<version>-macos-<arch>.mcpb` | Install in Claude Desktop |
| `wanderlog-mcp-<version>-macos-<arch>-openai/` | Ready-to-use OpenAI local marketplace |
| `wanderlog-mcp-<version>-macos-<arch>-openai.zip` | Archive of that same marketplace folder |

Move the packages you want to keep out of temporary storage, for example into
`~/Applications/Wanderlog/`. Keep the OpenAI marketplace folder intact: its configuration uses
relative paths to its bundled executable. Both packages contain that same executable and need
no Rust or Bun at runtime. Building requires network access for dependencies and schema checks.

### 2. Sign in once

If you already have a session in Keychain, continue to your client below. Otherwise, open a
terminal in the generated `-openai/` folder and use its bundled executable, even if you only
plan to install Claude:

```sh
./plugins/wanderlog-mcp/server/wanderlog-mcp auth login
./plugins/wanderlog-mcp/server/wanderlog-mcp auth status
```

Prefer a cookie? Replace `auth login` with `auth set`; it only asks for a cookie. For Claude,
you can also skip this terminal step and paste the cookie into the extension's optional field
during installation. See [Authenticate](#authenticate) for cookie retrieval and session renewal.

### 3a. Install in Claude Desktop Chat

1. Open **Settings → Extensions → Advanced settings → Install Extension**.
2. Select the generated `.mcpb` file matching your Mac's architecture.
3. Leave **Wanderlog session cookie** blank to use your Keychain session, or paste a `connect.sid`
   cookie into that sensitive field. The extension has no email/password login form.
4. Enable the extension, then select **Wanderlog** from **+ → Connectors** in a new chat.

Claude stores a supplied cookie in its own secure storage and passes it as `WANDERLOG_COOKIE`.
It overrides the shared Keychain session for this extension only; it is not imported into the
CLI's Keychain item. Clear the extension field to use the shared session again. `auth clear`
does not clear Claude's setting. Restart the extension's server if requested after a change.
See [Claude's local MCP guide](https://support.claude.com/en/articles/10949351-getting-started-with-local-mcp-servers-on-claude-desktop)
for the client's installation flow.

### 3b. Install in ChatGPT desktop / OpenAI

For **local Work / Codex conversations**, register the generated `-openai/` folder from its
permanent location with the Codex CLI. If you downloaded the ZIP, extract it first. Open a
terminal in the whole marketplace folder (the one containing `.agents/` and `plugins/`), then run:

```sh
codex plugin marketplace add "$PWD"
```

Restart ChatGPT desktop, open **Plugins Directory**, choose **Wanderlog local**, and install
**Wanderlog**. Start a new local Work / Codex conversation with the plugin enabled. The plugin
uses the Keychain session from step 2; installing it does not sign you in to Wanderlog.

To set up a repository without the registration command, copy the generated
`.agents/plugins/marketplace.json` and `plugins/wanderlog-mcp/` to those same paths under the
local repository you open in ChatGPT desktop. In Finder, **Command–Shift–.** shows `.agents`.
If a marketplace file already exists, merge the generated entry into its `plugins` array instead
of replacing it. The entry's `source.path` is relative to the repository root. Restart the app
and install from that marketplace in Plugins Directory as above.

This local plugin does not enable browser/mobile chat or every desktop Chat mode. Availability
depends on client version and workspace policy. OpenAI documents
[desktop stdio support](https://learn.chatgpt.com/docs/extend/mcp) and
[local marketplaces](https://developers.openai.com/plugins/build/plugins#build-your-own-curated-plugin-list).
Actual client UI installation and live account login have not been verified here; see
[packaging verification](docs/desktop.md#build-and-verify) for what is checked automatically.

### Standalone CLI (optional)

To put `wanderlog-mcp` on your command line instead of using the bundled executable:

```sh
mise run install                   # installs ~/.cargo/bin/wanderlog-mcp
which wanderlog-mcp                 # note the absolute path for GUI clients below
```

## Authenticate

The examples below use the standalone CLI. With a desktop package, run them from its
marketplace folder using `./plugins/wanderlog-mcp/server/wanderlog-mcp` in place of
`wanderlog-mcp`.

```sh
wanderlog-mcp auth login     # prompts for your Wanderlog email and password (password hidden)
wanderlog-mcp auth status    # → OK: logged in as <username>
wanderlog-mcp trips          # lists your trips and their ids
```

Run `auth login` in an interactive terminal. It exchanges your email and password for a session,
checks that the session is logged in, and saves only the `connect.sid` cookie to macOS Keychain.
The password is used for that login only and is never saved. Failed
logins leave the previously stored cookie untouched.

If you prefer to provide a cookie, `auth set` accepts one without asking for email or password:

1. Log in at <https://wanderlog.com> in Chrome (including through Google, Apple or Facebook).
2. Open DevTools → **Application** → **Cookies** → `https://wanderlog.com`, select `connect.sid`
   and copy its **Value** (it starts with `s%3A`).
3. Run `pbpaste | wanderlog-mcp auth set`, or run `wanderlog-mcp auth set` and paste at the hidden
   prompt. The cookie is verified before it replaces the stored session.

When the session expires, run `auth login` again or provide a fresh cookie with `auth set`.
Running MCP servers pick up the new cookie on their next call, with no restart needed.
`wanderlog-mcp auth clear` removes the stored cookie. The optional `WANDERLOG_COOKIE` environment
variable takes precedence over the credential store.

## Connect your agent

Follow [Install](#install) for desktop packages. The following manual setup is also available
for the standalone CLI.

GUI apps do not inherit your shell `PATH`. These shell commands expand `$HOME` before saving
the executable's absolute path in the client configuration. If you installed the CLI elsewhere,
use the path from `command -v wanderlog-mcp`. Add `--read-only` after `serve` for a read-only setup.

**Claude Code** (all projects):

```sh
claude mcp add --transport stdio --scope user wanderlog -- "$HOME/.cargo/bin/wanderlog-mcp" serve
claude mcp list        # then /mcp inside Claude Code
```

**Claude Desktop Chat**: follow [the extension installation steps](#3a-install-in-claude-desktop-chat)
above; its bundled launch configuration needs no user-specific path.

**Codex CLI, Codex IDE extension and the ChatGPT desktop app** share `~/.codex/config.toml`
(per OpenAI's docs; verified here with Codex CLI):

```sh
codex mcp add wanderlog -- "$HOME/.cargo/bin/wanderlog-mcp" serve
```

These local packages do not support browser or mobile chat. They supply no remote MCP endpoint
or tunnel; see [Install](#install) for the supported conversation surfaces.

## Tools

| Tool | Kind | What it does |
|---|---|---|
| `list_trips` | read | Your trips (own, shared with you, friends') with ids |
| `get_trip` | read | Itinerary with `[s:]`/`[b:]` refs; `detail: "full"` adds place_id + address |
| `search_places` | read | Real places (Google via Wanderlog), biased to a trip's destination |
| `get_place` | read | Address, rating, hours, website; with `trip_id` also Wanderlog's description and typical visit length |
| `preview_edits` | read | Dry run of a batch of edits against the latest revision |
| `apply_edits` | write | Apply a batch atomically (one revision) |
| `create_trip` | write | New trip for a destination, optional dates and title (shared with friends, the app default) |

Edit ops for `preview_edits` / `apply_edits` (each edit is one object with `op` plus fields):

| op | fields |
|---|---|
| `add_place` | `section`, `place_id`, `position?`, `text?`, `start_time?`, `end_time?` |
| `add_note` | `section`, `text`, `position?` |
| `add_checklist` | `section`, `items`, `heading?` (title), `position?` |
| `update_block` | `block`, `text?` + `text_mode?` (`replace`/`append`), `start_time?`, `end_time?`, `heading?` (checklist title) |
| `move_block` | `block`, `section`, `position?` |
| `remove_block` | `block` |
| `add_list` | `heading` |
| `update_section` | `section`, `heading?`, `text?`, `text_mode?` |
| `remove_section` | `section` (empty lists only) |
| `rename_trip` | `title` |
| `set_dates` | `start_date`, `end_date` (days keep their order; shrinking refuses to drop days that still have items) |

`section` accepts `s:<id>`, a day date `YYYY-MM-DD`, `day:<n>`, or a list's exact heading; `block`
takes `b:<id>`. Times are `HH:MM` (24h). Edits in one batch run in order and see each other; a
batch holds at most 100 edits and trips can span at most 90 days. `move_block`/`remove_block` work
on places, notes and checklists in lists and days.

Not supported yet (shown read-only): lodging, flights and transit reservations, budget expenses,
journal ("visited").

## Troubleshooting

- `not authorised` / `not logged in`: the session expired or you logged out; redo **Authenticate**.
- macOS asks whether `wanderlog-mcp` may use the Keychain item (possible after reinstalling the
  binary): choose **Always Allow**.
- `did not confirm the edit, so it may or may not have been applied`: the connection dropped after
  sending. Run `get_trip` to check before retrying, so nothing is applied twice.
- `Wanderlog refused the request (4001)`: rate limited; wait a minute and batch more edits per call.
- `changed since that preview`: someone (or an earlier attempt) edited the trip after
  `preview_edits`; preview again and confirm the new result.

## Development

```sh
mise run check         # cargo fmt --check, clippy -D warnings, unit tests
mise run package       # macOS packages + archive/stdio smoke checks; requires Bun and network
WANDERLOG_E2E_TRIP_ID=<throwaway trip id> mise run e2e   # live round-trip (trip with dates); restores it
```

Unit tests cover the json0 engine, the edit planner (component shapes mirror captured web-client
ops), rendering, and the tool schemas (flat JSON objects, no `$ref`/`oneOf`, for OpenAI clients).
The [packaging guide](docs/desktop.md#build-and-verify) describes artifact verification separately
from live account and desktop UI testing.
