**English** | [日本語](protocol.ja.md)

# Communication protocol reference

A description of "the shape of the JSON exchanged with the server" that you need in order
to write a bot, **kept consistent with the implementation**. The primary source for the
type definitions is [`src/wire/`](../src/wire/); compatibility is pinned by
[`tests/wire_contract.rs`](../tests/wire_contract.rs). You can implement a bot in another
language just by reading this.

> ⚠️ **`entity_id` is a number (`u32`)** — not a string like `"e-217"`.
> Players are referred to either by point of view (`"me"`/`"opp"`) or by fixed ID
> (`"p1"`/`"p2"`), depending on the context.

---

## 1. Connection

- WebSocket: `ws://HOST:PORT/ai-battle/v1/connect`
- **One WebSocket text frame = one JSON object.**
- Do **not** send the WebSocket subprotocol header (`Sec-WebSocket-Protocol`) — the server does not negotiate it.
- Frame-level ping/pong is handled automatically by the WebSocket library (distinct from the app-level `ping`/`pong`).

After connecting, first send [`subscribe`](#subscribe-ai--sv), then keep responding to incoming messages.

---

## 2. Message basics

Every message is identified by its `type` field (snake_case).

| Direction | `type` | Role |
|---|---|---|
| AI → SV | `subscribe` | Join a match / reconnect |
| SV → AI | `subscribed` | Join confirmation |
| SV → AI | `event` | Board-change notification (to both AIs) |
| SV → AI | `request` | Request to choose an active action |
| AI → SV | `response` | Reply to a `request` |
| SV → AI | `prompt` | Request a choice during resolution |
| AI → SV | `choice` | Reply to a `prompt` |
| SV → AI | `ping` | Liveness check |
| AI → SV | `pong` | Liveness reply |
| SV → AI | `error` | Error notification |

At minimum a bot must handle these four: **`request` / `prompt` / `event`(game_end) / `ping`**.

---

## 3. Server → AI (`ServerMessage`)

### `subscribed`

Reply to `subscribe`. The signal that the match is starting.

```json
{
  "type": "subscribed",
  "protocol_version": "0.1.0",
  "match_id": "m-0001",
  "your_player": "p1",
  "server_current_seq": 0,
  "opponent": { "ai_id": "dragapult-yopifutto", "display_name": "..." },
  "time_control": {
    "kind": "correspondence",
    "main_ms": 0, "increment_ms": 0, "byoyomi_ms": 0, "hard_per_response_ms": 60000
  },
  "session_token": "..."
}
```

| Key | Type | Description |
|---|---|---|
| `your_player` | `"p1"` \| `"p2"` | Your fixed player ID |
| `server_current_seq` | uint | The latest event seq issued so far |
| `opponent` | object | `{ai_id, display_name}` |
| `time_control` | object | `kind` = `sudden_death`/`increment`/`byoyomi`/`correspondence` + the various `*_ms` |
| `session_token` | string | Reconnect token (may be empty) |

### `request` — choose an active action

On your turn, or when an interrupt decision is needed. Responding to this is a bot's main job.

```json
{
  "type": "request",
  "request_id": "r-0050",
  "resent": false,
  "state": { "...": "§5 state schema" },
  "legal_actions": [
    { "id": "use_attack", "attack_index": 1 },
    { "id": "end_turn" }
  ],
  "clock": { "my_remaining_ms": 0, "opp_remaining_ms": 0, "running_for": "me" }
}
```

| Key | Type | Description |
|---|---|---|
| `request_id` | string | ID referenced by the reply. Reply to the same ID only once |
| `resent` | bool | `true` if the request was re-sent after a reconnect |
| `state` | object | Board masked to your point of view ([§5](#5-state-schema)) |
| `legal_actions` | array | **An enumeration of all possible actions** ([§6](#6-actions-actiondto)). Pick one of these |
| `clock` | object | Clock snapshot ([§9](#9-clock)) |

→ Reply with [`response`](#response-ai--sv), returning one of the `legal_actions` verbatim.

### `prompt` — a choice during effect resolution

Inserted after `response` and before the next `request`: search, damage placement,
who-goes-first after a coin flip, and so on.

```json
{
  "type": "prompt",
  "request_id": "p-0050-1",
  "parent_request_id": "r-0050",
  "resent": false,
  "kind": "choose_from_zone",
  "zone": "my_deck",
  "options": [ { "entity_id": 77, "card": "budew" }, { "entity_id": 78 } ],
  "min": 1,
  "max": 1,
  "shuffle_after": true,
  "clock": { "my_remaining_ms": 0, "opp_remaining_ms": 0, "running_for": "me" }
}
```

| Key | Type | Description |
|---|---|---|
| `request_id` | string | ID of this prompt (referenced by `choice`) |
| `parent_request_id` | string | ID of the request that caused it |
| `kind` | string | Prompt kind ([§7](#7-prompts-promptdto-and-responses)) |
| `min`, `max` | uint | Lower/upper bound on how many to choose |
| `state` | object? | The masked board at this point (may be omitted) |
| `shuffle_after` | bool? | Whether the zone is shuffled after the choice |
| (kind-specific) | — | `options` / `targets` / `eligible`, etc. ([§7](#7-prompts-promptdto-and-responses)) |

For each `kind`'s fields and how to reply, see [§7](#7-prompts-promptdto-and-responses).
→ Answer with [`choice`](#choice-ai--sv).

### `event` — board-change notification

A state change broadcast to both AIs. **The only one a bot must look at is `game_end`**;
the rest can be ignored and the match still works (the board is reflected in the next
`request.state`).

```json
{
  "type": "event",
  "seq": 142,
  "actor": "opp",
  "timestamp_unix_ms": 0,
  "replayed": false,
  "kind": "attach_energy",
  "data": { "player": "p2", "energy": 217, "to": 12 }
}
```

| Key | Type | Description |
|---|---|---|
| `seq` | uint | Monotonically increasing sequence number within the match (no gaps) |
| `actor` | `"me"`\|`"opp"`\|`"system"` | Whose action it was (receiver's point of view) |
| `replayed` | bool | `true` during reconnect replay |
| `kind` | string | Event kind ([§8](#8-events-eventdto)) |
| `data` | object? | kind-specific payload (omitted for data-less kinds such as `turn_end`) |
| `clock` | object? | Bundled at turn boundaries, etc. |

**End of game** (`kind: "game_end"`):

```json
{ "type": "event", "seq": 298, "actor": "system", "timestamp_unix_ms": 0,
  "replayed": false, "kind": "game_end",
  "data": { "winner": "p1", "reason": "PrizeTaken" } }
```

- `winner`: `"p1"` \| `"p2"`. Omitted (no key) on a draw.
- `reason`: a string. Actual values are `PrizeTaken` / `DeckOut` / `NoBenchOnKo` / `FlagFall` /
  `Concession`, etc. (PascalCase).
- After `game_end`, the server sends no further events. You may close the connection.

### `ping`

```json
{ "type": "ping", "server_current_seq": 327, "server_time_unix_ms": 0 }
```

→ Reply with [`pong`](#pong-ai--sv).

### `error`

```json
{ "type": "error", "code": "illegal_action",
  "message": "...", "related_request_id": "r-0050", "fatal": false }
```

| `code` | Meaning |
|---|---|
| `illegal_action` | `response.action` is not in `legal_actions` |
| `illegal_choice` | `choice.selected` is not in options, or violates the count |
| `unknown_request_id` | Reply to a request_id that does not exist |
| `from_seq_too_large` | `subscribe.from_seq` is too large |
| `invalid_session_token` | session_token mismatch |
| `protocol_violation` | Missing required field / type violation |
| `internal_error` | Internal server error (match aborted) |

When `fatal: true` the connection is dropped. `illegal_action` / `illegal_choice` are
`fatal: false`, and the server **waits again for a reply to the same request_id**.

---

## 4. AI → Server (`ClientMessage`)

### `subscribe` (AI → SV)

Sent first after connecting. Its **intent** routes you into a match.

```json
{
  "type": "subscribe",
  "match_id": "",
  "session_token": "",
  "from_seq": 0,
  "participant_id": "",
  "auth_token": "",
  "bucket": "",
  "room": "myroom",
  "vs_bot": "",
  "decklist": { "name": "Dragapult ex", "cards": [ { "slug": "dragapult-ex", "count": 3 } ] }
}
```

| Key | Type | Description |
|---|---|---|
| `match_id` | string | Empty when new. Set only on reconnect |
| `session_token` | string | Only on reconnect |
| `from_seq` | uint | `0` on the first connection |
| `participant_id` / `auth_token` / `bucket` | string | For ladder intent (empty otherwise) |
| `room` | string | Reliably pairs two connections with the same room (empty = open) |
| `vs_bot` | string | Names a built-in server bot as the opponent (empty = none) |
| `decklist` | object? | BYO deck. `{name?, cards:[{slug, count}]}`. The server resolves and validates it |

Intent priority (server side): known `session_token` → reconnect / `vs_bot` → vs-bot /
`participant_id` → ladder / `room` → room / otherwise → open.

### `response` (AI → SV)

```json
{ "type": "response", "request_id": "r-0050",
  "action": { "id": "use_attack", "attack_index": 1 } }
```

`action` must be an object that **exactly matches** one of the `legal_actions`.

### `choice` (AI → SV)

```json
{ "type": "choice", "request_id": "p-0050-1",
  "selected": [77], "counts": [], "yes": null, "branch_index": null }
```

| Key | Type | Description |
|---|---|---|
| `selected` | `[uint]` | List of chosen entity_ids, etc. Within the `[min, max]` range |
| `counts` | `[uint]`? | Counts / distribution (may be omitted if empty) |
| `yes` | bool? | yes/no, first-or-second (omit if not needed) |
| `branch_index` | uint? | Index of a branch choice (omit if not needed) |

Which one to fill in per `kind` is in [§7](#7-prompts-promptdto-and-responses).

### `pong` (AI → SV)

```json
{ "type": "pong", "last_seen_seq": 0 }
```

`last_seen_seq` is the seq of the last event you received (`0` is fine for a simple implementation).

---

## 5. State schema

The contents of `request.state` (and some `prompt.state`). All **masked to your point of view**.

```json
{
  "turn": 6,
  "phase": "main",
  "active_player": "me",
  "stadium": { "entity_id": 440, "card": "jamming-tower" },
  "me":  { "...": "PlayerView (fully public)" },
  "opp": { "...": "PlayerView (hand / deck / prizes are hidden)" }
}
```

- `phase`: `"setup"` / `"draw"` / `"main"` / `"between_turns"` / `"ended"`, etc.
- `active_player`: `"me"` / `"opp"`.
- `stadium`: the stadium in play (no key if none).

### `PlayerView`

```json
{
  "active": { "...": "PokemonInPlay (no key if none)" },
  "bench": [ "...PokemonInPlay × 0–5" ],
  "hand": [ { "entity_id": 30, "card": "boss-s-orders" } ],
  "deck_size": 32,
  "discard": [ "...EntityDto" ],
  "lost_zone": [ "...EntityDto" ],
  "prizes": [ { "entity_id": 101 } ],
  "energy_attached_this_turn": false,
  "supporter_played_this_turn": false,
  "mulligan_count": 0,
  "had_ko_last_turn": false
}
```

| Key | Type | Description |
|---|---|---|
| `active` | PokemonInPlay? | The active spot (no key if absent) |
| `bench` | array | 0–5 benched Pokémon |
| `hand` | `[EntityDto]` | Hand. **Yours has `card`; the opponent's has no `card` (null)** |
| `deck_size` | uint | Number of cards in the deck (contents/order are hidden) |
| `discard` / `lost_zone` | `[EntityDto]` | Fully public |
| `prizes` | `[EntityDto]` | Prizes. **Only the count is meaningful; `card` is generally null** |
| `*_this_turn` | bool | Whether energy was hand-attached / a supporter was played this turn |
| `had_ko_last_turn` | bool | Whether one of your Pokémon was KO'd during the opponent's previous turn |

### `EntityDto`

```json
{ "entity_id": 30, "card": "boss-s-orders" }
```

`card` is a slug. If undisclosed, **the key itself is absent** (= null).

### `PokemonInPlayDto`

```json
{
  "entity_id": 12,
  "card": "dragapult-ex",
  "stage": "stage_2",
  "evolution_stack": [10, 11],
  "hp_max": 320,
  "damage": 60,
  "energy_attached": [ { "entity_id": 77, "card": "fire-energy" } ],
  "tool_attached": { "entity_id": 150, "card": "..." },
  "status_conditions": ["burned", "asleep"],
  "abilities_used_this_turn": [0],
  "is_terastallized": false,
  "turn_in_play": 4
}
```

| Key | Type | Description |
|---|---|---|
| `stage` | string | `"basic"` / `"stage_1"` / `"stage_2"`, etc. |
| `evolution_stack` | `[uint]` | entity_ids of the pre-evolutions |
| `hp_max` / `damage` | uint | Max HP / damage on it (damage counters × 10) |
| `energy_attached` | `[EntityDto]` | Attached energy |
| `tool_attached` | EntityDto? | Attached tool (no key if none) |
| `status_conditions` | `[string]` | `poisoned`/`burned`/`asleep`/`paralyzed`/`confused` ([§10](#10-status-conditions)) |
| `abilities_used_this_turn` | `[uint]` | Indices of abilities used this turn |
| `turn_in_play` | uint | Number of turns since this individual entered play (e.g. summoning-sickness checks) |

---

## 6. Actions (`ActionDto`)

Each element of `legal_actions` / `response.action`. Identified by `id` (snake_case).

| `id` | Extra fields | Description |
|---|---|---|
| `play_card` | `entity_id`, `target`? | Play a card from hand (item/supporter/stadium/energy/Pokémon/evolution) |
| `use_ability` | `entity_id`, `ability_index` | An activated ability of a Pokémon in play |
| `use_in_hand_ability` | `entity_id`, `ability_index` | An activated ability of a card in hand |
| `use_stadium_effect` | `stadium_entity_id` | An activated stadium |
| `retreat` | `to_bench_index`, `energy_to_discard`? | Retreat (swap with bench N; entity_ids of energy to discard) |
| `use_attack` | `attack_index` | An attack of the active Pokémon (energy requirement already checked by the server) |
| `end_turn` | — | End your turn |
| `discard_fossil` | `entity_id` | Optionally discard a fossil in play |
| `concede` | — | Concede |

`target` (the target of `play_card`, etc.) is an object with a `kind`:

| `target.kind` | Extra | Description |
|---|---|---|
| `own_active` / `opp_active` | — | Your / the opponent's active spot |
| `own_bench` / `opp_bench` | `index` | Your / the opponent's bench slot N |
| `stadium` | — | The stadium |

Example: `{ "id": "play_card", "entity_id": 30, "target": { "kind": "opp_active" } }`

> **Just pick.** In `response`, return a `legal_actions` element verbatim — you do not need
> to assemble the fields yourself.

---

## 7. Prompts (`PromptDto`) and responses

The mapping from each `prompt.kind` to its fields and to the fields you fill in `choice`.
**This is "the response spec".**

| `kind` | prompt fields | Fill in `choice` |
|---|---|---|
| `choose_from_zone` | `zone`, `options:[EntityDto]` | `selected` = entity_ids to pick ([min,max] of them) |
| `choose_target_pokemon` | `targets:[uint]` | `selected` = `[1 Pokémon]` |
| `choose_initial_active` | `eligible:[uint]` | `selected` = `[1 Pokémon]` (first active) |
| `place_initial_bench` | `eligible:[uint]`, `bench_max` | `selected` = the subset to bench |
| `replace_active_after_ko` | `bench_options:[uint]` | `selected` = `[1 Pokémon]` (promote after a KO) |
| `distribute_damage` | `eligible:[uint]`, `total`, `per_target_max`? | `selected` = targets, `counts[i]` = amount placed on selected[i] |
| `attach_energy_to` | `energy_options:[uint]`, `pokemon_eligible:[uint]` | `selected` = `[energy, Pokémon]` |
| `discard_from_attached` | `eligible:[uint]`, `kind_filter` | `selected` = entities to remove |
| `reorder_cards` | `cards:[uint]`, `destination` | `selected` = the reordered order |
| `peek_and_reorder` | `peeked:[uint]`, `destination` | `selected` = the reordered order |
| `select_ability_order` | `entries:[uint]` | `selected` = resolution order |
| `assign_energy_to_targets` | `energies:[uint]`, `pokemon_eligible:[uint]` | `selected` = energy subset, `counts[i]` = index into `pokemon_eligible` |
| `pick_amount_from_each` | `sources:[[uint,uint]]`, `dest` | `counts[i]` = amount taken from `sources[i]` |
| `choose_yes_no` | `prompt_text` | `yes` = true/false |
| `choose_first_or_second` | (none) | `yes` = true (you go first) / false |
| `choose_one_branch` | `branch_count`, `labels:[string]` | `branch_index` = 0..branch_count |
| `choose_opponent_attack` | `attack_count`, `labels:[string]` | `branch_index` = 0..attack_count |
| `choose_status_to_remove` | `target`, `statuses:[string]` | `branch_index` = index into `statuses` |
| `pick_attack_to_copy` | `candidates:[[uint,[string]]]` | `selected` = `[Pokémon]`, `branch_index` = index of that attack |
| `prize_hand_swap_choice` | `prize_options:[uint]`, `hand_options:[uint]` | `selected` = `[prize, hand]`, `yes` = whether to swap |

> When in doubt, the `random` bot's implementation ([`src/bots/random.rs`](../src/bots/random.rs))
> is a safe example response for every kind. Fields that don't apply may be omitted
> (`counts: []` / `yes: null` / `branch_index: null`).

---

## 8. Events (`EventDto`)

A list of `event.kind`. A bot need not respond to these; they are for reference
(the board is reflected in the next `request.state`).

| `kind` | Main `data` | | `kind` | Main `data` |
|---|---|---|---|---|
| `game_start` | `p1_deck_size`, `p2_deck_size` | | `attach_energy` | `player`, `energy`, `to` |
| `decide_first_player` | `result` | | `evolve` | `player`, `from`, `to` |
| `setup_complete` | — | | `retreat_pokemon` | `player`, `from`, `to_bench_index` |
| `deal_initial_hand` | `player`, `entities` | | `use_ability` | `player`, `entity`, `ability_index` |
| `mulligan` | `player`, `count` | | `declare_attack` | `player`, `entity`, `attack_index` |
| `place_active` / `place_bench` | `player`, `entity` (+`index`) | | `apply_damage` | `target`, `amount` |
| `place_prizes` | `player`, `entities` | | `knock_out` | `entity` |
| `turn_start` | `turn`, `active_player` | | `take_prize` | `player`, `entity` |
| `draw_card` | `player`, `entity`, `deck_size_after` | | `apply_status` / `remove_status` | `entity`, `status` |
| `play_item`/`play_supporter`/`play_stadium` | `player`, `entity`, `card` | | `coin_flip` | `purpose`, `result` |
| `turn_end` / `checkup` | — | | `game_end` | `winner`?, `reason` |
| `internal` | `name` (coarse notification) | | `live_caught_up` | — (end of reconnect replay) |

> So that future server-added events don't break you, **ignore unknown kinds**.

---

## 9. Clock

Bundled in `request` / `prompt` and some `event`s:

```json
{ "my_remaining_ms": 487320, "opp_remaining_ms": 512840,
  "running_for": "me", "my_deadline_unix_ms": 1746541842500 }
```

- `my_remaining_ms` / `opp_remaining_ms`: **remaining total time** (milliseconds).
- `running_for`: `"me"` / `"opp"` / `"none"` (stopped).
- `my_deadline_unix_ms` (only when `running_for == "me"`): the **effective deadline for this response**
  (absolute unix ms). Computed as `now + min(total remaining, per-move cap)`, so because it
  **reflects the per-move cap it can be earlier than the total remaining**. If the bot does not
  return `response`/`choice` by this time, it loses on time. `my_deadline_unix_ms - now` gives
  "the time left for this move". Omitted when unlimited (no deadline).
- Example: the arena is "10 min total + 30 s per move", so even early on `my_deadline_unix_ms ≈ now + 30s`.
- `my_remaining_ms` is affected by network latency, so for tight budgeting it is more accurate
  to compare `my_deadline_unix_ms` against your own local clock.

---

## 10. Status conditions

Values of `status_conditions` / `apply_status`:

| `status` | Effect |
|---|---|
| `asleep` | Cannot act (Asleep) |
| `burned` | Damage counters at the Pokémon Checkup + coin to recover (Burned) |
| `confused` | Coin on attack declaration; on tails it fails + self-damage (Confused) |
| `paralyzed` | Cannot act; cleared at the end of the next turn (Paralyzed) |
| `poisoned` | Damage counters at the Pokémon Checkup (Poisoned) |

---

## 11. Information masking (anti-cheat)

The server holds the full state internally and hides it per point of view before sending.
What a bot **cannot see**:

- The contents of the opponent's hand (each entity in `opp.hand` has no `card` = null; only the count is known)
- The contents of either deck (`deck_size` only; order is also hidden)
- The contents of the prizes (`prizes`) (the entities exist but `card` is generally null)

**What is visible**: your own hand / both sides' field (active, bench, attached energy, tools) /
discard / lost zone / stadium.

Because an `entity_id` refers to the same card throughout the match, you can track "the card that
was searched out earlier" afterward.
