# ptcg-dragapult-bots

[PTCG AI Battle Platform](https://github.com/shun-1ulce/PTCG_AI_Battle_Platform) 向けの
**Dragapult ex 専用リファレンス AI bot** と、それをリモート対戦サーバ (`serve`) に繋ぐ
**薄い WebSocket クライアント** (`connect`)。

ルールエンジン本体には依存しません。サーバとは protocol JSON だけでやり取りし、カード事実
(ex 判定・ワザ index) は [pokemon-card-data](https://github.com/1ulce/pokemon-card-data)
(submodule) から読みます。bot ロジックの戦略仕様 (機械可読版) は [`docs/bots/`](docs/bots/)。

## 収録 bot

| 名前 | 説明 |
|---|---|
| `random` | 合法手・選択肢から一様ランダム (ベースライン / フォールバック) |
| `dragapult-takeuchi` | Dragapult ex 固定戦略・クチート竹内版 |
| `dragapult-yopifutto` | Dragapult ex 固定戦略・よぴふっと博士版 |

戦略 bot は `decks/dragapult-ex.yaml` を持参デッキ (BYO) として前提にしています。

## セットアップ

```sh
git clone --recurse-submodules https://github.com/shun-1ulce/ptcg-dragapult-bots.git
cd ptcg-dragapult-bots
# すでに clone 済みなら:
git submodule update --init --recursive
cargo build --release
```

## 使い方

リモートの統合 `serve` に接続して対戦します。`--server` は必須。

```sh
# プライベートルームで自作 2 bot を対戦させる (同じ room を 2 接続)
cargo run --release --bin connect -- \
  --server ws://HOST:8765 --room myroom \
  --bot dragapult-takeuchi --deck decks/dragapult-ex.yaml --games 5

cargo run --release --bin connect -- \
  --server ws://HOST:8765 --room myroom \
  --bot dragapult-yopifutto --deck decks/dragapult-ex.yaml --games 5
```

intent (接続ごとに指定):

| 指定 | 意味 |
|---|---|
| (無指定) | open match (誰でも先着 2 人ペア) |
| `--room ID` | プライベートルーム (同じ room の 2 人を確実にペア) |
| `--vs NAME` | サーバ内蔵 bot を相手にリクエスト |
| `--participant-id ID --auth-token TOK [--bucket B]` | ladder (要 ladder サーバ) |

主なオプション: `--bot NAME` / `--deck PATH` / `--games N` / `--seed S` /
`--cards-dir DIR` (既定 `data/pokemon-card-data/cards`)。`connect --help` も参照。

## 自作 bot を足す

1. `src/bots/<name>.rs` に [`BotPolicy`](src/bots/mod.rs) を実装した struct を書く
2. `src/bots/mod.rs` の `build` / `available` に 1 行ずつ登録する

bot は protocol DTO ([`src/wire`](src/wire)) と [`CardFacts`](src/cards.rs) だけを使って
判断します。盤面で見えるもの / 見えないものは `StateDto` のフィールドを参照してください。

## ライセンス

`MIT OR Apache-2.0` のデュアルライセンス ([LICENSE-MIT](LICENSE-MIT) /
[LICENSE-APACHE](LICENSE-APACHE))。submodule の `data/pokemon-card-data` は別途
そのリポジトリのライセンスに従います。
