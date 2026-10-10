# Tool reference

For installation and authentication, start with the [user guide](../README.md).
This reference describes MCP tool inputs, edit behavior, and the safety model.

## Tools

| Tool | Kind | What it does |
|---|---|---|
| `list_trips` | read | Your trips (own, shared with you, friends') with ids |
| `get_trip` | read | Itinerary with `[s:]`/`[b:]` refs; `detail: "full"` adds place_id + address |
| `search_places` | read | Real places (Google via Wanderlog), biased to a trip's destination |
| `get_place` | read | Address, rating, hours, website; with `trip_id` also Wanderlog's description and typical visit length |
| `preview_edits` | read | Dry run of a batch of edits against the latest revision |
| `apply_edits` | write | Apply a batch atomically (one revision) |
| `create_trip` | write | New trip for a destination, optional dates and title |

Edit ops for `preview_edits` / `apply_edits` (each edit is one object with `op` plus fields):

| op | fields |
|---|---|
| `add_place` | `section`, either `place_id` or `source`, `include_photos?`, `position?`, `text?`, `start_time?`, `end_time?` |
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

For a place already in another trip, use `source` instead of looking it up again:

```json
{
  "op": "add_place",
  "section": "Places to visit",
  "source": {"trip_id": 123, "block": "b:456", "revision": 8},
  "text": "Candidate from the previous trip"
}
```

Use real refs and the source revision from `get_trip`. Each source trip is read once per batch;
the source block must still exist, and `revision`, when provided, must still match. This reuses
the place data and saved photos with a new destination block ID. Notes, times, reactions,
attachments and reservations are not copied; provide `text` and times explicitly when wanted.

`get_trip` and `get_place` populate a cache scoped to the current login and server process.
Only missing place IDs need a batch lookup, and successful lookups survive a later failure.
Photo lookup is off by default; `include_photos: true` requests missing photos and reports any
lookup failure before writing. Saved or cached photos are reused either way. The final edits
remain one atomic revision, regardless of how many lookup batches were needed.

A real `place_id` obtained through another Maps tool is accepted. Passing an external place
object to bypass all lookup is not supported yet; see the [API verification limits](protocol.md#place-resolution-verification).

Content limits and new-trip sharing defaults are listed in the [user guide](../README.md#use-it).

## Safety model

- **Your session stays local.** The session cookie is never shown to the model or written to logs.
  See [Sign in](../README.md#sign-in) for storage, renewal, and credential precedence.
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
  per tool unless you allow it); that approval is the authorization. The default flow is
  `get_trip`, which reports the current revision, then one `apply_edits` call for the whole batch
  with that revision as `base_revision`. If the trip changed in between — a tripmate's edit, or an
  earlier attempt of the same call — nothing is applied, so retries cannot double-apply.
  `preview_edits` shows the exact changes without writing, for when the user wants to review them
  first; it costs an extra call and connection.
- For a server with all writes disabled, use the [read-only setup](../README.md#cli-installation-alternative).
