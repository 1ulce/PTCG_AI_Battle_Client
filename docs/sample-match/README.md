**English** | [日本語](README.ja.md)

# Sample match log

A real example of the log that `connect` saves automatically for every game. One game
(`--seed 7`) on the public arena `wss://arena.ptcgtools.com` where `dragapult-takeuchi`
(you / P1) challenged the built-in bot `dragapult-yopifutto` (P2). Result:
`winner=P2 reason=PrizeTaken`.

Reproduce with:

```sh
cargo run --release --bin connect -- --server wss://arena.ptcgtools.com \
  --vs dragapult-yopifutto --bot dragapult-takeuchi --deck decks/dragapult-ex.yaml --seed 7
```

## Files

| File | Use | Contents |
|---|---|---|
| [`match.log`](match.log) | Human-readable | Per `request`: a board summary (each side's active/bench, HP, energy count, status conditions, hand/deck/prize counts) + the list of legal actions + the action the bot chose. Plus `prompt` choices, the `event` flow, and `=== RESULT: ... ===` at the end |
| [`raw.jsonl`](raw.jsonl) | Analysis / replay | Every sent/received message as `{"t":"recv"\|"send","msg":{...}}`, one per line. The board `state` / `legal_actions` / `prompt` / responses / `event` as complete JSON |

A single turn from `match.log` (board → legal actions → chosen move):

```text
== request r-0032 (turn 6, phase main, active me) ==
  me  active: budew#21 HP30/30
  me  bench : dragapult-ex#11 HP320/320 | duskull#12 HP60/60
  me  hand=4 deck=41 prizes=6 discard=4 lost=0
  opp active: fezandipiti-ex#80 HP190/210 E×1
  opp bench : dreepy#63 HP70/70 | duskull#72 HP60/60 | duskull#73 HP60/60
  opp hand=5 deck=42 prizes=6 discard=2 lost=0
  legal actions:
    [0] {"id":"end_turn"}
    [1] {"id":"play_card","entity_id":4}
    [2] {"id":"play_card","entity_id":15,"target":{"kind":"own_bench","index":1}}
    [3] {"id":"use_attack","attack_index":0}
    [4] {"id":"retreat","to_bench_index":0}
    [5] {"id":"retreat","to_bench_index":1}
  >> chose: {"id":"play_card","entity_id":15,"target":{"kind":"own_bench","index":1}}
```

> For the protocol (the meaning of keys, all action / prompt kinds) see [`../protocol.md`](../protocol.md).
