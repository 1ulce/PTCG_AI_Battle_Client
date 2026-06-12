**English** | [日本語](README.ja.md)

# PTCG_AI_Battle_Client

A reference implementation + connection client that lets anyone build a
**battle AI bot** for the Pokémon Trading Card Game. It connects over WebSocket
to the battle server of the
[PTCG AI Battle Platform](https://arena.ptcgtools.com) and runs bot-vs-bot matches.

The goal is a "Pokémon TCG bot arena" — the equivalent of shogi's floodgate or
chess's Lichess Bots. Clone this repo and run `connect`, and you can
**put your own bot into the arena right now**.

```
your bot (this repo)            ──WebSocket──▶  battle server (arena.ptcgtools.com)
   choose_action / choose_prompt                rules / board masking / matchmaking
```

- **No dependency on the rules engine itself.** Communication with the server is
  pure JSON protocol (`src/wire/`). That keeps it lightweight, with no special
  build dependencies.
- Card facts (ex detection, attack index) are read from
  [pokemon-card-data](https://github.com/1ulce/pokemon-card-data) (a submodule).
- The bundled bots' logic doubles as a worked example of "how to write a Pokémon TCG bot".

---

## Bundled bots

| Name | Description |
|---|---|
| `random` | Picks uniformly at random from legal actions / choices. Baseline and fallback |
| `dragapult-takeuchi` | Fixed strategy for the Dragapult ex deck — "Kuchiito Takeuchi" persona |
| `dragapult-yopifutto` | Fixed strategy for the Dragapult ex deck — "Dr. Yopifutto" persona |

The strategy bots assume `decks/dragapult-ex.yaml` as their bring-your-own (BYO) deck.
The "source of truth" for the logic (how they decide) lives in the machine-readable
specs under [`docs/bots/`](docs/bots/).

---

## Setup

Requires Rust 1.80+ ([rustup](https://rustup.rs/)).

```sh
# Clone together with the submodule (card data)
git clone --recurse-submodules https://github.com/1ulce/PTCG_AI_Battle_Client.git
cd PTCG_AI_Battle_Client

# If you already cloned without submodules, fetch them
git submodule update --init --recursive

cargo build --release
```

---

## Using the battle server

The `connect` binary acts as a WebSocket client to the remote battle server.
The public arena runs at **`wss://arena.ptcgtools.com`** (TLS, port 443).

> ℹ️ The arena is operated by a maintainer. If it does not respond, it may be down.

### Simplest example — challenge a built-in arena bot

```sh
cargo run --release --bin connect -- \
  --server wss://arena.ptcgtools.com \
  --vs dragapult-yopifutto \
  --bot dragapult-takeuchi \
  --deck decks/dragapult-ex.yaml \
  --games 3
```

### Pit your own bots against each other (private room)

Two connections that pass the same `--room` are guaranteed to be paired. Start
them in two terminals (or in the background).

```sh
# terminal 1
cargo run --release --bin connect -- --server wss://arena.ptcgtools.com \
  --room myroom --bot dragapult-takeuchi --deck decks/dragapult-ex.yaml --games 5

# terminal 2
cargo run --release --bin connect -- --server wss://arena.ptcgtools.com \
  --room myroom --bot dragapult-yopifutto --deck decks/dragapult-ex.yaml --games 5
```

### Connection intent

Each connection specifies how to find an opponent.

| Option | Meaning |
|---|---|
| (none) | open match — the first two arrivals are paired |
| `--room ID` | private room — the two clients with the same room are paired (best for your-own-bots) |
| `--vs NAME` | name a built-in server bot as the opponent |
| `--participant-id ID --auth-token TOK [--bucket B]` | ladder (rated play; requires a ladder-capable server) |

Because each connection brings its own `--deck`, you can also run
**asymmetric matches with different decks** on each side.

### `connect` options

| Flag | Default | Description |
|---|---|---|
| `--server URL` | (required) | Target. `wss://HOST` (TLS, default 443) / `ws://HOST:PORT` (plaintext) |
| `--bot NAME` | `random` | `random` / `dragapult-takeuchi` / `dragapult-yopifutto` |
| `--deck PATH` | none | BYO deck YAML |
| `--games N` | `1` | Number of games to repeat (1 game per connection) |
| `--seed S` | `42` | RNG seed (reproducibility) |
| `--cards-dir DIR` | `data/pokemon-card-data/cards` | Location of card data |
| `--log-dir DIR` | `target/matches` | Where match logs are saved (always on; see below) |
| `--room` / `--vs` / `--participant-id`, etc. | — | Intent (above) |

`connect --help` also lists them.

### Match logs (always saved)

`connect` automatically saves a log of every game into its own directory at
`<log-dir>/<UTC-timestamp>-<bot>-vs-<opponent>-seed<N>/` (default `target/matches/...`).
Each directory holds two files:

| File | Use | Contents |
|---|---|---|
| `match.log` | Human-readable | Per `request`: a board summary (each side's active/bench, HP, energy count, status conditions, hand/deck/prize counts) + the list of legal actions + the action the bot chose. Plus `prompt` choices, the `event` flow, and `=== RESULT: ... ===` at the end |
| `raw.jsonl` | Analysis / replay | Every sent/received message as `{"t":"recv"\|"send","msg":{...}}`, one per line. The board `state` / `legal_actions` / `prompt` / responses / `event` are recorded as complete JSON |

The timestamp in the directory name is **UTC**. It is used only for the file name
— never fed to bot decisions or the RNG — so reproducibility with a fixed `--seed`
is unaffected.

```text
target/matches/2026-06-11T203914-dragapult-takeuchi-vs-dragapult-yopifutto-seed7/
├─ match.log     # human-readable game log
└─ raw.jsonl     # JSON of all messages (one message per line)
```

A real example is included under [`docs/sample-match/`](docs/sample-match/)
(the `match.log` / `raw.jsonl` of the seed7 game above).

### Time control (your bot must answer in time)

The public arena runs **sudden death: 10 minutes total per player + a 30-second cap per
move**. "One move" here means **one round-trip**: each `request`→`response` and each
`prompt`→`choice`. Your bot **loses on time (`FlagFall`)** if either limit is exceeded:

- a single response takes longer than **30 s**, or
- your total thinking time across the whole game exceeds **10 min**.

A bot that stays connected but **hangs / never answers also loses** (after 30 s on that
move) — the match won't stall forever.

Every `request` / `prompt` carries a `clock`. Use it if you want to budget your time:

- `my_remaining_ms` — your **total** remaining time.
- `my_deadline_unix_ms` — the **absolute deadline for this response** (`now + min(total
  remaining, per-move cap)`); on the arena this is roughly `now + 30 s`. Answer before it.

See the [clock section of `docs/protocol.md`](docs/protocol.md#9-clock) for details. The
bundled reference bots respond in milliseconds and don't read the clock, but your bot may.

---

## Writing your own bot

The main purpose of this repo is to be **the foundation on which you write your own bot**.
There are only two steps.

### 1. Implement `BotPolicy`

Create `src/bots/<your_bot>.rs` and implement the [`BotPolicy`](src/bots/mod.rs) trait.

```rust
use rand_chacha::ChaCha20Rng;
use crate::bots::{BotPolicy, PromptChoice};
use crate::wire::action::ActionDto;
use crate::wire::protocol::{PromptMsg, RequestMsg};
use crate::transport::TransportError;

pub struct MyBot;

impl BotPolicy for MyBot {
    /// Choose one active action on your turn. You only pick from `req.legal_actions`.
    fn choose_action(
        &mut self,
        req: &RequestMsg,
        rng: &mut ChaCha20Rng,
    ) -> Result<ActionDto, TransportError> {
        // req.state … the board masked to your point of view (StateDto)
        // req.legal_actions … the legal moves the server enumerated. The AI just "picks"
        // e.g. just end the turn
        Ok(req
            .legal_actions
            .iter()
            .find(|a| matches!(a, ActionDto::EndTurn))
            .cloned()
            .unwrap_or_else(|| req.legal_actions[0].clone()))
    }

    /// Respond to choices made during effect resolution (search, damage placement,
    /// who-goes-first after a coin flip, etc.).
    fn choose_prompt(&mut self, p: &PromptMsg, rng: &mut ChaCha20Rng) -> PromptChoice {
        // p.kind … what is being asked (PromptDto). When in doubt, delegate to RandomPolicy.
        crate::bots::RandomPolicy.choose_prompt(p, rng)
    }
}
```

### 2. Register it

Add one line each to `build` and `available` in `src/bots/mod.rs`.

```rust
// available()
&["random", "dragapult-takeuchi", "dragapult-yopifutto", "my-bot"]
// build()
"my-bot" => Some(Box::new(MyBot)),
```

Now it runs with `--bot my-bot`.

### What a bot can "see"

- **`req.state` (`StateDto`)** — the board masked to your point of view. You can see
  the contents of your own hand, but the **opponent's hand and decks are `card: null`**
  (not peekable). The field, discard, and stadium are fully public.
  → [`src/wire/state.rs`](src/wire/state.rs)
- **`req.legal_actions` (`Vec<ActionDto>`)** — the legal moves the server enumerated
  as referee. You cannot send illegal moves, so the AI just **picks from the list**.
- **Card facts** — HP and attack indices are not entirely contained in the board DTO.
  [`CardFacts`](src/cards.rs) looks up `is_ex` / `attack_index` from a slug (the bundled
  bots resolve indices assuming the cards in `decks/dragapult-ex.yaml`).

### Discipline (faithfulness)

- **Pick from legal actions.** Don't invent a "plausible-looking" move in an unknown
  situation — delegating to `RandomPolicy` is the safe default.
- **Use only `ChaCha20Rng`** for randomness (reproducible with a fixed seed). Don't
  bring in non-determinism such as `Instant`/`SystemTime`.
- When studying the existing bots' logic, also read the specs under [`docs/bots/`](docs/bots/).

---

## Protocol overview

The **complete reference** for the JSON exchanged with the server is in
**[`docs/protocol.md`](docs/protocol.md)** (every message's keys, types, error codes,
how to respond to prompts, and information masking — all kept consistent with the
implementation). You can implement a bot in another language just by reading it. The
primary source for the type definitions is [`src/wire/`](src/wire/), and compatibility
is pinned by `tests/wire_contract.rs`.

The essentials:

- Connect via **WebSocket** to `wss://HOST/ai-battle/v1/connect` (TLS) or `ws://HOST:PORT`. One frame = one JSON object.
- Server → AI: **`ServerMessage`** (`subscribed` / `event` / `request` / `prompt` / `ping` / `error`)
- AI → Server: **`ClientMessage`** (`subscribe` / `response` / `choice` / `pong`)
- Two streams: the broadcast **event stream**, and the decision loop **request/prompt → response/choice**.
- **Information masking**: the server hides the board per point of view, so you cannot see the opponent's hand (no cheating).
- The actual battle loop of `connect` lives in [`src/bin/connect.rs`](src/bin/connect.rs).

---

## Project layout

```
src/
├─ wire/        # JSON DTOs compatible with the server (protocol / state / action / event)
├─ cards.rs     # CardFacts: looks up ex / attack index from pokemon-card-data
├─ deck.rs      # DeckList (BYO deck YAML)
├─ transport.rs # tungstenite WebSocket client
├─ bots/        # BotPolicy trait + bundled bots (random / takeuchi / yopifutto)
└─ bin/connect.rs  # subscribe → Request/Prompt response loop
decks/          # dragapult-ex.yaml (BYO deck)
docs/bots/      # machine-readable specs for bot strategies
data/pokemon-card-data/  # card master (submodule)
tests/wire_contract.rs   # JSON contract test against the server
```

Local checks:

```sh
cargo test                       # bot logic + wire contract tests
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

---

## Relationship to the rules engine

This repo contains **only the bots and the client**; the rules engine (referee) lives
on the PTCG AI Battle Platform side. That is exactly why it can stay engine-free and let
anyone write a bot lightly. The bundled bots' logic and the wire DTOs are synced
one-way from upstream as the "source of truth" (details in [CLAUDE.md](CLAUDE.md)).

---

## License

Dual-licensed under `MIT OR Apache-2.0`
([LICENSE-MIT](LICENSE-MIT) / [LICENSE-APACHE](LICENSE-APACHE)).
The `data/pokemon-card-data` submodule follows its own repository's license.

Issues / Pull Requests welcome. Bring your own new bots or different archetype decks.
