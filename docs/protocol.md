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

All responses are `{"success": bool, ...}`; failures carry `messages[]`/`error`.

| Purpose | Request | Notes |
|---|---|---|
| Current user | `GET /api/user` | `{user: null}` when logged out |
| Trip list | `GET /api/tripPlans/home` | `ownTripPlans[]`, `friendsPrivateSharedTripPlans[]`, `friendsTripPlans[]`; items have `id`, `key`, `keyType` (`edit`/...), `title`, `startDate`, `endDate`, `placeCount`, `editedAt` |
| Trip | `GET /api/tripPlans/{key}?clientSchemaVersion=2` | `{tripPlan, resources{geo, placeMetadata, ...}}`; without the query param: "app version too old"; `tripPlan.overallVersion` = ShareDB version |
| Destination search | `GET /api/geo/autocomplete/{query}` | `data[]{id, name, countryName, latitude, longitude, bounds[w,s,e,n]}` |
| Place search | `GET /api/placesAPI/autocomplete/v2?request={json}` | json = `{input, sessiontoken: uuid4, location: {longitude, latitude}, radius: metres, language}`; `location` and `radius` are both required (else `success:false`), but only bias results: `0,0` + 50 km still finds places worldwide; results mix `{place_id, structured_formatting}` with `{type: "search"}` rows (no place_id) |
| Place details | `GET /api/placesAPI/getPlaceDetails/v2?placeId=&language=en` | Google-style object; stored verbatim as `block.place` |
| Place photos | `POST /api/placePhotos/{place_id}` body `{place}` | `data[]` image keys → `block.imageKeys` |
| Place metadata | `GET /api/places/metadata?placeIds=&listId={key}&listType=tripPlan&ensurePlaceDetailsAreFresh=true&includeNeedsBooking=true` | description, `minMinutesSpent`/`maxMinutesSpent`, categories |
| Create trip | `POST /api/tripPlans` | body `{geoIds:[id], initialMapsPlaceIds:[], initialSections:null, initialEmailId:null, type:"plan", startDate, endDate, privacy:"friends", isMapEmbed:false, title:null, autogenerateItineraryOptions:null, language:"en"}` → `data{key, viewKey, id, title}` |

The web app also calls `/api/tripPlans/{key}/settings`, `/api/recommendations/v2`,
`/api/flights/*`, analytics and chat endpoints; none are needed here.

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
