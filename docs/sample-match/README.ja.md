[English](README.md) | **日本語**

# サンプル棋譜

`connect` が対戦ごとに自動保存する棋譜の実例です。公開アリーナ `wss://arena.ptcgtools.com` で
`dragapult-takeuchi` (自分 / P1) が内蔵 bot `dragapult-yopifutto` (P2) に挑んだ 1 局
(`--seed 7`)。結果は `winner=P2 reason=PrizeTaken`。

再現コマンド:

```sh
cargo run --release --bin connect -- --server wss://arena.ptcgtools.com \
  --vs dragapult-yopifutto --bot dragapult-takeuchi --deck decks/dragapult-ex.yaml --seed 7
```

## ファイル

| ファイル | 用途 | 中身 |
|---|---|---|
| [`match.log`](match.log) | 人が読む | `request` ごとに盤面要約 (両者の場・HP・エネ数・状態異常・手札/山札/サイド枚数) + 選択肢一覧 + bot が選んだ手。`prompt` の選択、`event` の流れ、末尾に `=== RESULT: ... ===` |
| [`raw.jsonl`](raw.jsonl) | 解析・リプレイ | 送受信した全メッセージを `{"t":"recv"\|"send","msg":{...}}` で 1 行ずつ記録。盤面 `state` / `legal_actions` / `prompt` / 応答 / `event` を完全な JSON で |

`match.log` の 1 ターン分の例 (盤面 → 選択肢 → 選んだ手):

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

> プロトコル (キーの意味・全アクション/プロンプト種別) は [`../protocol.md`](../protocol.md) を参照。
