# Wanderlog web-app protocol (as used by this crate)

Wanderlog has no public API. This protocol is based on live traffic of the official web app
(Chrome, 2026-10-06) and re-checks with this crate's own client. Password-login evidence and its
verification limits are recorded below. The protocol is private and may change without notice;
when it breaks, re-capture the browser flow and update this file first.

Placeholders: `{key}` = trip edit key (16 lowercase letters), `{id}` = numeric ids.

## Auth

- Session cookie `connect.sid` (HttpOnly, Secure, SameSite=Lax, ~1 year expiry, re-issued at login).
  It authenticates both REST and the WebSocket upgrade.
- **Trip keys are bearer capabilities**: subscribing to a trip works with the key alone, without any
  cookie. Never expose keys to models or logs; this crate addresses trips by numeric id.

### Password login

Verified against the [official login page](https://wanderlog.com/login) and its
[public frontend bundle](https://itin-compiled.azureedge.net/7dc4f271/compiled/main.1edee1715a296a.js)
on 2026-10-08:

- The web client sends `POST /api/user/login` with JSON fields `email`, `password`, and
  `platform: "web"`, alongside version and analytics metadata. The form uses an email address;
  username login is not verified.
- The client expects `{success: true, user: {...}}`; application failures use `messages[]` and
  `errTypes`. The CLI keeps login errors generic because response bodies may contain credentials.
- The CLI sends the three fields above, requires `connect.sid` from `Set-Cookie`, then checks
  `GET /api/user` with only that cookie before saving it. It does not follow redirects or persist
  the email/password. An unsuccessful login does not replace the stored session.

The successful password-login round trip, login-response `Set-Cookie`, and whether the server
requires any additional browser metadata have not been verified against a real account. Local
HTTP tests cover request fields, cookie extraction, session verification, and failure handling.

## REST (`https://wanderlog.com`)

Application responses are `{"success": bool, ...}`; failures carry `messages[]`/`error`.
HTTP errors can instead contain HTML, so status must be checked before parsing JSON.

| Purpose | Request | Notes |
|---|---|---|
| Current user | `GET /api/user` | `{user: null}` when logged out |
| Trip list | `GET /api/tripPlans/home` | `ownTripPlans[]`, `friendsPrivateSharedTripPlans[]`, `friendsTripPlans[]`; items have `id`, `key`, `keyType` (`edit`/...), `title`, `startDate`, `endDate`, `placeCount`, `editedAt` |
| Trip | `GET /api/tripPlans/{key}?clientSchemaVersion=2` | `{tripPlan, resources{geo, placeMetadata, ...}}`; without the query param: "app version too old"; `tripPlan.overallVersion` = ShareDB version |
| Destination search | `GET /api/geo/autocomplete/{query}` | `data[]{id, name, countryName, latitude, longitude, bounds[w,s,e,n]}` |
| Place search | `GET /api/placesAPI/autocomplete/v2?request={json}` | json = `{input, sessiontoken: uuid4, location: {longitude, latitude}, radius: metres, language}`; `location` and `radius` are both required (else `success:false`), but only bias results: `0,0` + 50 km still finds places worldwide; results mix `{place_id, structured_formatting}` with `{type: "search"}` rows (no place_id) |
| Place details | `GET /api/placesAPI/getPlaceDetails/v2?placeId=&language=en` | Google-style object; stored verbatim as `block.place` |
| Multiple place details | `GET /api/placesAPI/getMultiplePlaceDetails?placeIds[]=ID_A&placeIds[]=ID_B&language=en` | Repeated array parameters (encoded as `placeIds%5B%5D`), not comma-separated; `data[]` keyed by `place_id` |
| Place photos | `POST /api/placePhotos/{place_id}` body `{place}` | `data[]` image keys → `block.imageKeys` |
| Place metadata | `GET /api/places/metadata?placeIds=&listId={key}&listType=tripPlan&ensurePlaceDetailsAreFresh=true&includeNeedsBooking=true` | description, `minMinutesSpent`/`maxMinutesSpent`, categories |
| Create trip | `POST /api/tripPlans` | body `{geoIds:[id], initialMapsPlaceIds:[], initialSections:null, initialEmailId:null, type:"plan", startDate, endDate, privacy:"friends", isMapEmbed:false, title:null, autogenerateItineraryOptions:null, language:"en"}` → `data{key, viewKey, id, title}` |
| Move trip to trash | `DELETE /api/tripPlans/{key}` | `{success: true}`; the trip leaves `/api/tripPlans/home` |
| Restore from trash | `POST /api/tripPlans/restore` body `{keys:[key]}` | from the frontend bundle only; never called |
| Delete from trash | `POST /api/tripPlans/deleteFromTrash` body `{keys:[key]}` | `{success: true}`; permanent |

The web app also calls `/api/tripPlans/{key}/settings`, `/api/recommendations/v2`,
`/api/flights/*`, analytics and chat endpoints; none are needed here.

### Trip deletion

The three trash endpoints come from the
[web app's frontend bundle](https://itin-compiled.azureedge.net/03e05897/compiled/main.7eba9da7f94524.js)
(2026-10-10). The same day, one temporary trip was created, moved to the trash and deleted from
it on a real account: every request returned HTTP 200 with `success: true`, and afterwards the
trip was absent from `/api/tripPlans`, `/api/tripPlans/home` and `/api/user`. Where the web app
lists the trash was not traced.

Unverified: `GET /api/tripPlans/{key}?clientSchemaVersion=2` still returned HTTP 200 after the
permanent delete. Its body was not inspected, so whether deletion revokes the trip key is
unknown; treat a leaked key as live. Only the live test helper deletes trips; the library and
the tools deliberately cannot.

### Place resolution verification

Rechecked on 2026-10-08 against the
[official Places client](https://itin-compiled.azureedge.net/7dc4f271/compiled/8388.main.4d6573227c7f1d.js)
and three authenticated, read-only API requests: autocomplete, single details, and multiple details.
All returned HTTP 200 with `success: true`. The multiple-details request used two IDs observed in
autocomplete; both returned matching `place_id` values. Its objects included `name`,
`formatted_address`, `geometry.location`, `types`, ratings, hours, website and `photo_urls`, matching
the single-details representation. No session cookie, trip key or account response is stored here.

The client resolves cache misses in sequential batches of five. That is a conservative client
choice; the server's maximum batch size and quota accounting are unknown. Missing IDs cause a
pre-write failure, retaining successful results. A bulk failure never falls back to a burst of
individual requests.

The [web client's copy helper](https://itin-compiled.azureedge.net/7dc4f271/compiled/2864.main.f6aa6434d09e11.js)
clones existing blocks and emits JSON0 insertions without calling place details. Its
[block factory](https://itin-compiled.azureedge.net/7dc4f271/compiled/70.main.9764172e4f181d.js)
does not require `imageKeys`. This crate reuses only the place payload and photo keys, and creates
the rest of the destination block independently. User-facing source and photo semantics are in
the [edit tool guide](tools.md#tools).

Arbitrary external or custom minimal place payloads have not been validated through a live write
and are not accepted as tool input. This investigation performed no itinerary writes or quota
stress tests. Automated tests use localhost fixtures, including simulated non-JSON 429 responses.

### HTTP rate limiting

HTTP 429 is recognized before JSON decoding. `Retry-After` accepts seconds or an HTTP-date;
missing/invalid headers use a local 60-second cooldown, not an inferred server quota. Clones of
the current REST session share the cooldown across endpoints. Calls during it fail locally with
the remaining delay; no request, including POST, is automatically replayed. A new login starts
a new REST session and place cache.

MCP errors retain readable text and add `structuredContent` with `code: "RATE_LIMITED"`,
`retry_after_seconds` (null without a valid header), `retry_in_seconds`, `stage`, and `write_state`.
Only errors explicitly caught before an itinerary write carry `write_state: "not_started"`;
other contexts use `not_reported`. The separate ShareDB unknown-outcome guard still applies.

## ShareDB (all itinerary edits)

`wss://wanderlog.com/api/tripPlans/wsOverall/{key}?clientSchemaVersion=2` with header
`Origin: https://wanderlog.com` (+ cookie). Without `clientSchemaVersion=2` or without `Origin` the
server closes the socket right after it opens. (The browser also sends `X-WL-User-ID` and
`X-WL-Anonymous-ID` query params; they are not required.) A separate `/api/tripPlans/wsCursors/{key}`
socket carries presence only.

```
→ {"a":"hs","id":null,"protocol":1,"protocolMinor":2}
← {"a":"init","protocol":1,"protocolMinor":2,"id":"<session>","type":"http://sharejs.org/types/JSONv0"}
← {"a":"hs","protocol":1,"protocolMinor":2,"id":"<session>","type":"http://sharejs.org/types/JSONv0"}
→ {"a":"s","c":"TripPlans","d":"{key}"}
← {"a":"s","c":"TripPlans","d":"{key}","data":{"v":<version>,"data":<trip document>}}
→ {"a":"op","c":"TripPlans","d":"{key}","v":<version>,"seq":<n>,"x":{},"op":[<json0 components>]}
← {"a":"op","c":"TripPlans","d":"{key}","v":<applied at>,"seq":<n>,"src":"<session>"}          (ack, no op body)
← {"a":"op",...,"src":"<other session>","seq":k,"op":[...]}                                     (tripmate edits)
← {"code":4001,"message":"Too many requests"}                                                    (bare error form)
```

All components of one `op` apply atomically. An op submitted at an older `v` is transformed
server-side against concurrent ops, so a client only needs a fresh snapshot to build ops from.

## Trip document

Same object as REST `tripPlan`: `title`, `startDate`, `endDate`, `days`, `privacy`, `placeCount`,
`itinerary{sections[], budget{expenses[], ...}, journal{stops[]}}`, plus bookkeeping fields.

Sections: `{id, type, mode, heading, text, blocks[], placeMarkerColor, placeMarkerIcon, date?}`

| type / mode | meaning |
|---|---|
| `textOnly` / `placeList` | trip "Notes" (rich text only) |
| `normal` / `placeList` | a place list ("Places to visit", custom lists) |
| `normal` / `dayPlan` | a day (`date: YYYY-MM-DD`) |
| `hotels`, `flights`, `transit` | reservations; created on the first booking of each kind, inserted near the top (indices shift!) |

Blocks: `place` (`place`, `text`, `startTime`/`endTime` `HH:MM`, `imageKeys`, `addedBy`, `imageSize`,
`upvotedBy`, `travelMode`, `attachments`; lodging adds `hotel{checkIn,checkOut,...}`, `guests`),
`note` (`text`), `checklist` (`title`, `items[{id, checked, text}]`), `flight`, `train`/`bus`/`ferry`.
Rich text is a Quill delta `{ops:[{insert, attributes?}]}` that always ends with `"\n"`.
New ids are client-generated random integers below 1e9.

## json0 components sent by the web client

| Edit | Components |
|---|---|
| Rename trip | `{"p":["title"],"t":"text0","o":[{"p":0,"d":"old"},{"p":0,"i":"new"}]}` |
| Set dates on a dateless trip | `startDate`/`endDate`/`days` as `od`/`oi`, then one `li` day section each |
| Extend / shift / delete day | `endDate`+`days` `od`/`oi` + `li` appended day; shift = every day's `date` `od`/`oi`; delete = `ld` section. Reservations/expenses/journal dates are not shifted |
| Add place | `li` block at `[..sections,s,"blocks",i]` (+ `oi` `imageKeys` in the same op) |
| Note / section text | `{"p":[...,"text"],"t":"rich-text","o":<Quill delta>}` |
| Place times | `{"p":[...,"startTime"],"od":old,"oi":"17:00"}` (same for `endTime`; `null` when unset) |
| Section heading | `{"p":[..sections,s,"heading"],"od":old,"oi":new}` |
| New list | `li` section `{heading:"", text, blocks:[], placeMarkerColor, placeMarkerIcon:"map-marker", id, type:"normal", mode:"placeList"}` |
| Reorder within section | `{"p":[..blocks,i],"lm":j}` |
| Move across sections | `ld` from source + `li` into target in one op (the web client assigns a new id; this crate keeps the id so `expenses[].blockId` links survive) |
| Delete item | `ld` with the full block |
| Note block | `li {id, type:"note", text, addedBy, attachments:[]}` |
| Checklist | `li {id, type:"checklist", items:[], attachments:[], addedBy, title:""}`; item `li {id, checked:false, text}`; item text = `od`/`oi` of the whole delta; tick = `checked` `od`/`oi` |
| Expense | `li` into `itinerary.budget.expenses` `{id, amount{amount,currencyCode}, category, description, date, blockId, paidByUserId, paidByUser, splitWith, associatedDate?}` |
| Mark visited | `li` into `itinerary.journal.stops` `{type:"confirmed", title, id, dateTime, place, text, media, endDateTime}` |
