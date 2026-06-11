# CLAUDE.md

このファイルは Claude Code (claude.ai/code) およびコントリビュータが本リポジトリで
作業するときのガイドです。日本語で構いません。

## このリポジトリは何か

ポケモンカード対戦 AI bot の **リファレンス実装 + 接続クライアント**。
[PTCG AI Battle Platform](https://github.com/shun-1ulce/PTCG_AI_Battle_Platform) の対戦サーバに
WebSocket で接続し、bot 同士を自動対戦させる。**ルールエンジン本体には依存しない** のが最大の特徴。

- ルール審判・盤面マスク・マッチングは **サーバ側 (上流リポジトリ)** の責務。
- 本リポジトリは「自分の番に何をするか」を決める `BotPolicy` と、それをサーバに繋ぐ
  `connect` クライアントだけを持つ。
- 一般ユーザは公開アリーナ **`arena.ptcgtools.com`** (ポート 8765、平文 `ws://`) に繋いで遊ぶ。

## アーキテクチャ

```
src/
├─ wire/        # サーバと JSON 互換の DTO。engine 非依存で自前定義
│   ├─ protocol.rs  # ServerMessage / ClientMessage / SubscribeMsg / PromptDto 他
│   ├─ state.rs     # StateDto / PlayerView / PokemonInPlayDto / EntityDto
│   ├─ action.rs    # Action / ActionTarget / EntityId
│   └─ event.rs     # EventDto (受信用。ループが分岐するのは GameEnd のみ)
├─ cards.rs     # CardFacts: pokemon-card-data の YAML から is_ex / attack_index を引く軽量層
├─ deck.rs      # DeckList (持参デッキ YAML、serde のみ。resolve はサーバ側)
├─ transport.rs # WsClient: tungstenite 直叩きの WebSocket クライアント
├─ bots/        # BotPolicy trait + PromptChoice + 共有ヘルパー + 収録 bot
│   ├─ mod.rs   # trait / build / available / 盤面読みヘルパー / testutil
│   ├─ random.rs
│   ├─ dragapult_takeuchi.rs
│   └─ dragapult_yopifutto.rs
└─ bin/connect.rs  # subscribe → Request/Prompt 応答ループ + 棋譜ログ (target/matches/)
tests/wire_contract.rs   # サーバが送る JSON をデシリアライズできる契約テスト
data/pokemon-card-data/  # カードマスタ (submodule)
docs/protocol.md         # 通信プロトコルの JSON レベル リファレンス (実装と一致)
docs/bots/               # bot 戦略の機械可読仕様 (ロジックの真値)
```

プロトコル (サーバと交わす JSON の形・キー・エラー・プロンプト応答) の詳細は
[`docs/protocol.md`](docs/protocol.md)。serde 形の一次ソースは `src/wire/` で、これと
`docs/protocol.md` を食い違わせないこと (wire DTO を変えたら doc も更新)。

## 上流 (ルールエンジン) との関係 — 重要

bot ロジック (`src/bots/*`) と wire DTO (`src/wire/*`) は、上流 PTCG AI Battle Platform の
`crates/bots/` と `crates/engine-runtime/` を **真値として片方向に同期**したもの。

- **engine への依存を絶対に足さない。** `engine-core` / `engine-runtime` を `Cargo.toml` の
  dependencies に入れない。型は必ず `src/wire` / `src/cards` の自前型を使う。
- **bot ロジックは上流が真値。** ここで独自に戦略を「改善」しない。戦略の変更は上流で行い、
  ここへ同期する。バグ修正も原則上流に投げる。
- **wire DTO の serde 形は 1 バイトでも変えない。** フィールド名・`tag`/`rename_all`・
  `skip_serializing_if`・`flatten` は上流と完全一致させる。ずれると通信が壊れる
  (`tests/wire_contract.rs` が検知する)。
- 新しい bot や別デッキを **追加**するのは歓迎 (上流に無いものを足すのは OK)。既存 3 bot の
  ロジックを書き換えるのは同期前提でのみ。

上流からの同期で型を載せ替えるときの対応:

| 上流の型 | 本リポジトリの型 |
|---|---|
| `engine_runtime::state_dto::*` | `crate::wire::state::*` |
| `engine_runtime::protocol::*` | `crate::wire::protocol::*` |
| `engine_runtime::action_dto::ActionDto` (= `engine_core::actions::Action`) | `crate::wire::action::ActionDto` |
| `engine_core::actions::ActionTarget` / `types::EntityId` | `crate::wire::action::{ActionTarget, EntityId}` |
| `engine_core::game_state::CardRegistry` | `crate::cards::CardFacts` (フィールド名は `registry` のまま) |
| `engine_runtime::transport::TransportError` | `crate::transport::TransportError` |
| `crate::<helper>` (上流の bots crate ルート) | `super::<helper>` / `crate::bots::<helper>` |

`CardRegistry` の代替として bot が実際に使うのは 2 機能だけ:
`is_ex(slug)` (= `prize_value >= 2`) と `attack_index(slug, name)` (ワザ名 → YAML position 順 index)。
どちらも `CardFacts` が pokemon-card-data から提供する。

## 開発コマンド

```sh
cargo build --release
cargo test                                    # bot ロジック + wire 契約テスト
cargo clippy --all-targets -- -D warnings
cargo fmt --check

# 対戦 (公開アリーナの内蔵 bot に挑む)
cargo run --release --bin connect -- \
  --server ws://arena.ptcgtools.com:8765 --vs dragapult-yopifutto \
  --bot dragapult-takeuchi --deck decks/dragapult-ex.yaml --games 3

# 自作 bot 同士 (同じ --room を 2 接続)
cargo run --release --bin connect -- --server ws://arena.ptcgtools.com:8765 \
  --room r1 --bot dragapult-takeuchi --deck decks/dragapult-ex.yaml
```

`connect` の intent: 無指定=open / `--room ID`=プライベートルーム / `--vs NAME`=内蔵 bot 相手 /
`--participant-id`=ladder。各接続が `--deck` を持参 (非対称デッキ可)。

対戦ごとに棋譜が `target/matches/<UTC日時>-<bot>-vs-<相手>-seed<N>/` に常時保存される
(`--log-dir` で変更可)。`match.log` = 人間可読 (盤面要約 + 選択肢 + bot の選択 + 流れ)、
`raw.jsonl` = 送受信メッセージの完全 JSON (1 行 1 メッセージ、リプレイ/解析用)。

## bot の足し方

1. `src/bots/<name>.rs` に `BotPolicy` を実装した struct を書く。
   - `choose_action(req, rng)` — `req.legal_actions` から 1 つ選んで返す。
   - `choose_prompt(p, rng)` — 効果解決中の選択に応答。迷ったら `RandomPolicy` に委譲。
2. `src/bots/mod.rs` の `available()` と `build()` に 1 行ずつ登録。
3. テストを書く (`#[cfg(test)]`、`testutil` ヘルパーで `RequestMsg`/`PromptMsg` を組める)。

### bot 実装の規律

- **合法手から選ぶ。** 不明な局面で手を発明しない。分からなければ `RandomPolicy` フォールバック
  か `EndTurn` に倒す (未実装トレーナーズの誤爆を避ける既存パターンを踏襲)。
- **乱数は引数の `ChaCha20Rng` だけ。** `Instant`/`SystemTime`/`thread_rng` を使わない
  (シード固定の再現性を壊す)。`rng` の消費順を変えると再現性が変わる点に注意。
  (この禁止は **bot ロジック** が対象。`bin/connect.rs` が棋譜ディレクトリ名に `SystemTime` で
  UTC 日時を付けるのは可 — bot 判断や `rng` には一切渡さないので再現性に影響しない。)
- bot から見えるのは **マスク済み `StateDto`** (相手の手札・山札は `card: null`) と
  サーバ列挙の `legal_actions` のみ。カード固有の数値は `CardFacts` から引く。

## コミット / ブランチ

- Conventional Commits 風 (`feat:` / `fix:` / `docs:` / `refactor:` / `chore:`)、日本語可。
- 1 件ごとに `cargo test` + `clippy -D warnings` + `fmt --check` を緑に保つ。

## やってはいけないこと

- `engine-core` / `engine-runtime` への依存を足す。
- wire DTO の serde 形 (フィールド名 / tag / skip) を上流と非互換に変える。
- WebSocket サブプロトコルヘッダ (`ptcgl-ai-battle.v1`) を送る (上流 serve は negotiate しない)。
- 既存 3 bot のロジックを上流同期なしに書き換える。
