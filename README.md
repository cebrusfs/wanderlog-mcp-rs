# wanderlog-mcp

Let AI agents read and edit your [Wanderlog](https://wanderlog.com) trips. One Rust binary that is
both an [MCP](https://modelcontextprotocol.io) server (stdio) and a small CLI. Works with any local
MCP client: Claude Code, Claude Desktop, Codex CLI / IDE extension, and the ChatGPT desktop app.

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

Requires Rust 1.89+.

```sh
cargo install --path . --locked     # installs ~/.cargo/bin/wanderlog-mcp
which wanderlog-mcp                 # note the absolute path for GUI clients below
```

## Authenticate

```sh
wanderlog-mcp auth login     # prompts for your Wanderlog email and password (password hidden)
wanderlog-mcp auth status    # → OK: logged in as <username>
wanderlog-mcp trips          # lists your trips and their ids
```

Run `auth login` in an interactive terminal. It exchanges your email and password for a session,
checks that the session is logged in, and saves only the `connect.sid` cookie to the OS credential
store (Keychain on macOS). The password is used for that login only and is never saved. Failed
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

GUI apps do not inherit your shell `PATH`: use the absolute path from `which wanderlog-mcp`
(shown below as `/Users/<you>/.cargo/bin/wanderlog-mcp`). Add `--read-only` after `serve` for a
read-only setup.

**Claude Code** (all projects):

```sh
claude mcp add --transport stdio --scope user wanderlog -- /Users/<you>/.cargo/bin/wanderlog-mcp serve
claude mcp list        # then /mcp inside Claude Code
```

**Claude Desktop**: Settings → Developer → Edit Config (`~/Library/Application Support/Claude/claude_desktop_config.json`),
merge in the entry below, then quit and restart Claude Desktop. Logs: `~/Library/Logs/Claude/mcp-server-wanderlog.log`.

```json
{
  "mcpServers": {
    "wanderlog": { "command": "/Users/<you>/.cargo/bin/wanderlog-mcp", "args": ["serve"] }
  }
}
```

**Codex CLI, Codex IDE extension and the ChatGPT desktop app** share `~/.codex/config.toml`
(per OpenAI's docs; verified here with Codex CLI):

```sh
codex mcp add wanderlog -- /Users/<you>/.cargo/bin/wanderlog-mcp serve
```

or by hand:

```toml
[mcp_servers.wanderlog]
command = "/Users/<you>/.cargo/bin/wanderlog-mcp"
args = ["serve"]
```

claude.ai on the web/mobile and ChatGPT on the web can only reach *remote* MCP servers over
public HTTPS; this project intentionally does not host one (it would put your Wanderlog session on a
server).

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
WANDERLOG_E2E_TRIP_ID=<throwaway trip id> mise run e2e   # live round-trip (trip with dates); restores it
```

Unit tests cover the json0 engine, the edit planner (component shapes mirror captured web-client
ops), rendering, and the tool schemas (flat JSON objects, no `$ref`/`oneOf`, for OpenAI clients).
