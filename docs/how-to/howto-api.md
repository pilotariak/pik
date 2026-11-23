# How to call the slots API

pik serves a REST API under `/api/v1`: slots, invitations, members, sessions, answers, lineups and recovery links. This
guide calls each route with `curl`, against the local Worker. The same calls work against `https://pik.pilotariak.com`.

[`openapi.json`](../../openapi.json) is the contract: every route, body, response and error. Each release publishes it
as an asset, and Kancha generates its types from it.

## Before you start

You need `curl`, `jq` and `uuidgen`. Start the Worker on an empty local database:

```bash
make migrate-local       # create the tables in .wrangler/state
make dev                 # wrangler dev → http://localhost:8787
```

In another terminal:

```bash
BASE=http://localhost:8787/api/v1
curl -s "$BASE/health"   # {"status":"ok"}
```

Paths below are relative to `$BASE`. Bodies are JSON, and so are responses, except errors (see
[Handle errors](#handle-errors)).

## Send the player token

pik has no accounts: each device holds one player token, returned once by the call that creates the device's player.
Keep it: pik stores only its SHA-256 and cannot give it back.

- **Scripts and the native app** send it in a header: `Authorization: Bearer <token>`.
- **The website** gets it in the `__Secure-pik_token` cookie (`HttpOnly`, `Secure`, `SameSite=Lax`, `Path=/api`), set
  by the same responses. The browser sends it back by itself. A write with the cookie must come with
  `Origin: https://kancha.pilotariak.com`, otherwise pik answers 403.

When a request has both, the `Authorization` header wins. Without a token, or with an unknown one, every route answers
401, except `GET /health`, `GET /join/:code` and the routes that create a player.

## Create a slot

The device that creates a slot becomes its first organizer. Without a token, pik creates a player and returns its token:

```bash
KEY=$(uuidgen)
curl -s -X POST "$BASE/slots" \
  -H "Content-Type: application/json" \
  -H "Idempotency-Key: $KEY" \
  -d '{
    "name": "Mercredi soir",
    "venue": "Trinquet Moderne",
    "court": "trinquet",
    "specialty": "gomme_creuse",
    "anchor_date": "2026-09-02",
    "interval_weeks": 1,
    "time_from": "18:00",
    "time_to": "19:30",
    "season_start": "2026-09-01",
    "season_end": "2027-06-30",
    "min_players": 4,
    "places": 8,
    "organizer_name": "Maite"
  }' | tee slot.json
```

```json
{"slot_id":"s_…","invite_code":"K7QW2MHT","member_id":"m_…","player_token":"…"}
```

Keep the values for the next steps:

```bash
SLOT=$(jq -r .slot_id slot.json)
CODE=$(jq -r .invite_code slot.json)
ORGANIZER=$(jq -r .player_token slot.json)
```

- Adjust the season to dates around today: `anchor_date` is the first session, within the season, and gives the
  weekday. `interval_weeks` is 1 (every week) or 2 (every other week). A season lasts at most 366 days.
- Optional fields and their defaults: `min_players` 4, `places` 4 (or `min_players` if larger), `target` 40 (30, 35 or
  40), `deadline_days_before` 1 and `deadline_time` `"20:00"`: players answer until the day before at 20:00, Paris
  time.
- `court` is `trinquet`, `mur_a_gauche` or `place_libre`.
- With a token, the slot goes to that device's player and `player_token` is `null`.

**Retry safely.** After a network failure, send the same request with the same `Idempotency-Key` (a UUID v4) within 24
hours. pik returns the first slot instead of creating a second one, with a fresh token: the first token stops working,
since it never reached you. The same key with another body answers 422.

## Invite players

Share the invite code, for example in a link that Kancha opens. Anyone can preview the slot and its free names, without
a token:

```bash
curl -s "$BASE/join/$CODE" | jq '{name, next_session, list_closed, free_names}'
```

A player adds their name ("not in the list"). A new device gets a player token:

```bash
PEIO=$(curl -s -X POST "$BASE/join/$CODE/members" \
  -H "Content-Type: application/json" -d '{"name": "Peio"}' | jq -r .player_token)
```

Or the organizer adds names first, and each player claims theirs ("C'est moi"):

```bash
ANA_ID=$(curl -s -X POST "$BASE/slots/$SLOT/members" \
  -H "Authorization: Bearer $ORGANIZER" \
  -H "Content-Type: application/json" -d '{"name": "Ana"}' | jq -r .id)

ANA=$(curl -s -X POST "$BASE/join/$CODE/claim" \
  -H "Content-Type: application/json" -d "{\"member_id\": \"$ANA_ID\"}" | jq -r .player_token)
```

- A device that already has a token sends it: it joins with the same player, and `player_token` is `null`. A device
  that is already a member of the slot gets 409 `already-member`.
- On these two routes, an unknown token counts as no token: a device whose player was purged gets a new one.
- Names are unique in a slot whatever their case: `PEIO` after `Peio` gets 409 `name-taken`.
- The invite code is only used to join. Rotating it does not affect members who already joined:

```bash
curl -s -X POST "$BASE/slots/$SLOT/invite-code" -H "Authorization: Bearer $ORGANIZER"
```

- Close the list to stop new names; free names can still be claimed:

```bash
curl -s -X PUT "$BASE/slots/$SLOT/list" -H "Authorization: Bearer $ORGANIZER" \
  -H "Content-Type: application/json" -d '{"closed": true}'
```

## Read a player's slots

`GET /me` lists the player's slots, one card each, with the next session, its state and counts (the Créneaux tab):

```bash
curl -s "$BASE/me" -H "Authorization: Bearer $PEIO" | jq '.slots[] | {name, role, next_session}'
```

`GET /slots/:id` returns one slot with its members and its next four sessions. A caller who is not a member gets 404,
even if the slot exists:

```bash
curl -s "$BASE/slots/$SLOT" -H "Authorization: Bearer $PEIO" | jq '{name, me, members, upcoming_sessions}'
DATE=$(curl -s "$BASE/slots/$SLOT" -H "Authorization: Bearer $PEIO" | jq -r '.upcoming_sessions[0].date')
```

A session date is a local date in Europe/Paris, `YYYY-MM-DD`. A date that is not a session of the slot (another
weekday, the off week, outside the season) answers 422 `not-a-session`. Instants in bodies, such as `deadline`, are
RFC 3339 in UTC.

To list sessions by month:

```bash
curl -s "$BASE/slots/$SLOT/sessions?from=2026-12-01&to=2026-12-31" -H "Authorization: Bearer $PEIO"
```

Without `from`, the range starts today in Paris; without `to`, it ends with the season. A range covers at most 366 days.

## Answer for a session

A player answers `ok`, `dispo` (comes if needed) or `ko`. The response carries the recomputed counts and state, to
confirm or roll back an optimistic update:

```bash
curl -s -X PUT "$BASE/slots/$SLOT/sessions/$DATE/answer" \
  -H "Authorization: Bearer $PEIO" \
  -H "Content-Type: application/json" -d '{"status": "ok"}' | jq .session
```

An organizer answers for a member, for example one who never joined, with `member_id`:

```bash
curl -s -X PUT "$BASE/slots/$SLOT/sessions/$DATE/answer" \
  -H "Authorization: Bearer $ORGANIZER" \
  -H "Content-Type: application/json" -d "{\"status\": \"dispo\", \"member_id\": \"$ANA_ID\"}"
```

- The last answer wins. Sending the same status again keeps the member's place in the call order.
- After the deadline, only `ko` is accepted: `ok` or `dispo` get 409 `deadline-passed`. A cancelled session takes no
  answer (409 `session-cancelled`).
- `state` is `on` (enough OK answers), `missing` (with the `missing` count, before the deadline), `to_complete` (the
  Dispo answers would complete it), `cancelled_short` (not enough players at the deadline) or `cancelled`. Dispo
  answers do not count toward the minimum.

## Change one session

An organizer cancels a date, writes a note, or changes the minimum, places or deadline of that date only. An absent
field keeps its value, `null` goes back to the slot's value:

```bash
curl -s -X PATCH "$BASE/slots/$SLOT/sessions/$DATE" \
  -H "Authorization: Bearer $ORGANIZER" \
  -H "Content-Type: application/json" \
  -d '{"cancelled": true, "note": "Trinquet fermé"}'

curl -s -X PATCH "$BASE/slots/$SLOT/sessions/$DATE" \
  -H "Authorization: Bearer $ORGANIZER" \
  -H "Content-Type: application/json" \
  -d '{"cancelled": false, "places": 6, "deadline_at": null}'
```

The places of a date must reach its minimum, or the change answers 422.

## Compose the lineup

`GET /slots/:id/sessions/:date` returns everything the Séance and Composer screens need: answers by status, members
without an answer, sessions played, guests, the call order and the lineup. Its `ETag` header is the lineup version,
`"0"` before the first lineup:

```bash
curl -s -D headers.txt "$BASE/slots/$SLOT/sessions/$DATE" \
  -H "Authorization: Bearer $ORGANIZER" | jq '{state, answers, call_order, lineup}'
ETAG=$(grep -i '^etag:' headers.txt | cut -d' ' -f2 | tr -d '\r')
```

Add the guests of the session, the whole list each time. A guest whose name is kept keeps their id, so sending the same
list again changes nothing:

```bash
GUEST=$(curl -s -X PUT "$BASE/slots/$SLOT/sessions/$DATE/guests" \
  -H "Authorization: Bearer $ORGANIZER" \
  -H "Content-Type: application/json" \
  -d '{"guests": [{"name": "Unai"}]}' | jq -r '.guests[0].id')
```

Then write the lineup with the `ETag` in `If-Match`. The selection is in order; a game has two teams of two players
from the selection, and a player plays in one game of a session at most:

```bash
MAITE_ID=$(jq -r .member_id slot.json)
curl -s -i -X PUT "$BASE/slots/$SLOT/sessions/$DATE/lineup" \
  -H "Authorization: Bearer $ORGANIZER" \
  -H "If-Match: $ETAG" \
  -H "Content-Type: application/json" \
  -d "{
    \"selection\": [{\"member\": \"$MAITE_ID\"}, {\"member\": \"$ANA_ID\"}, {\"guest\": \"$GUEST\"}],
    \"games\": []
  }"
```

The response carries the new `ETag`. Another organizer who still sends the old one gets 412 `lineup-changed`: read the
session again and redo the change. A write without `If-Match` gets 428 `if-match-required`. The selection cannot
exceed the places of the date (422).

## Manage members

An organizer makes another member an organizer, or fixes a misspelled name:

```bash
PEIO_ID=$(curl -s "$BASE/slots/$SLOT" -H "Authorization: Bearer $PEIO" | jq -r .me.member_id)
curl -s -X PATCH "$BASE/slots/$SLOT/members/$PEIO_ID" \
  -H "Authorization: Bearer $ORGANIZER" \
  -H "Content-Type: application/json" -d '{"role": "organizer"}'
```

- `DELETE /slots/:id/members/:member/claim` frees a name claimed by mistake. The member keeps their name and answers,
  and the name can be claimed again.
- `DELETE /slots/:id/members/me` leaves the slot. The member stays in past sessions as "Ancien joueur" (`name` is
  `null`), and their answers from today on are deleted.
- The last organizer of a slot can neither leave, lose the role nor have their name released: 409 `last-organizer`.
  Make another organizer first.

## Recover a lost device

A player who lost their device, or cleared their browser, asks an organizer for a recovery link. The organizer creates
it and sends it privately:

```bash
LINK=$(curl -s -X POST "$BASE/slots/$SLOT/members/$PEIO_ID/recovery" \
  -H "Authorization: Bearer $ORGANIZER" \
  -H "Idempotency-Key: $(uuidgen)" | jq -r .token)
```

The new device redeems it. Without a token, it gets a new player token; with one, the member joins that device's
player:

```bash
curl -s -X POST "$BASE/recover/$LINK"
```

The member keeps their answers. A link works once and for 24 hours: then it answers 410 `recovery-link-gone`. A link
restores one slot: a player in several slots needs a link from each.

## Change or delete the slot

An organizer changes the name, venue, times, minimum, places, target or deadline rule. The weekday and the frequency
are fixed for the season: `interval_weeks` or `anchor_date` answers 422, create a new slot instead.

```bash
curl -s -X PATCH "$BASE/slots/$SLOT" \
  -H "Authorization: Bearer $ORGANIZER" \
  -H "Content-Type: application/json" \
  -d '{"time_from": "18:30", "deadline_days_before": 2, "deadline_time": "12:00"}'
```

`DELETE /slots/:id` deletes the slot and all its data: members, answers, guests, lineups and recovery links.

## Handle errors

Errors are `application/problem+json` (RFC 9457). Read `type`, which never changes, rather than `title` or `detail`:

```json
{"type":"https://pik.pilotariak.com/problems/name-taken","title":"This name is taken","status":409}
```

| Status | `type` (after `/problems/`)                                                                             | When                                            |
| ------ | ------------------------------------------------------------------------------------------------------- | ----------------------------------------------- |
| 401    | `unauthorized`                                                                                          | No token, or an unknown one                     |
| 403    | `organizer-only`, `origin-not-allowed`                                                                  | A player on an organizer route, a bad `Origin`  |
| 404    | `not-found`                                                                                             | Unknown resource, or the caller is not a member |
| 405    | `method-not-allowed`                                                                                    | The path exists with other methods              |
| 409    | `name-taken`, `list-closed`, `session-cancelled`, `deadline-passed`, `last-organizer`, `already-member` | The state forbids the change                    |
| 410    | `recovery-link-gone`                                                                                    | The recovery link was used or has expired       |
| 412    | `lineup-changed`                                                                                        | Stale `If-Match` on the lineup                  |
| 422    | `invalid-request`, `not-a-session`, `idempotency-key-reused`                                            | Invalid body, parameter or header               |
| 428    | `if-match-required`                                                                                     | Lineup write without `If-Match`                 |
| 429    | `rate-limited`                                                                                          | Rate limit reached: wait `Retry-After` seconds  |

Slot creation (5), name claims and additions through the invite code (10) and recovery redemptions (5) are limited per
client address and per minute.

Every API response sends `Cache-Control: private, no-store`: do not cache slot data.

## Change the contract

After changing a route, its request or its response, update its `#[utoipa::path]` annotation and its types, then
regenerate and commit the contract:

```bash
make openapi
git add openapi.json
```

`make test` fails while `openapi.json` differs from the code. CI also compares it with `main` (`oasdiff breaking`): a
breaking change, such as a removed route or a new required field, goes to `/api/v2/` while `/api/v1/` keeps working.
