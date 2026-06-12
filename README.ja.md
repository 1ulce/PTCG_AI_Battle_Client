[English](README.md) | **日本語**

# PTCG_AI_Battle_Client

ポケモンカードゲーム (Pokémon TCG) の **対戦 AI bot** を誰でも作れるようにするための、
リファレンス実装 + 接続クライアントです。
[PTCG AI Battle Platform](https://arena.ptcgtools.com) の
対戦サーバに WebSocket で接続し、bot 同士を自動対戦させられます。

将棋の floodgate、チェスの Lichess Bots に相当する「ポケカ版 bot アリーナ」を目指しています。
このリポジトリを clone して `connect` を実行すれば、**今すぐ自分の bot をアリーナで戦わせられます**。

```
あなたの bot (このリポジトリ)  ──WebSocket──▶  対戦サーバ (arena.ptcgtools.com)
   choose_action / choose_prompt              ルール審判・盤面マスク・相手とのマッチング
```

- **ルールエンジン本体には依存しません。** サーバとは JSON プロトコルだけでやり取りします
  (`src/wire/`)。だから軽量で、ビルドに特別な依存もいりません。
- カード事実 (ex 判定・ワザの index) は
  [pokemon-card-data](https://github.com/1ulce/pokemon-card-data) (submodule) から読みます。
- 収録 bot のロジックはそのまま読めば「ポケカ bot の書き方」の実例になります。

---

## 収録 bot

| 名前 | 説明 |
|---|---|
| `random` | 合法手・選択肢から一様ランダムに選ぶ。ベースライン兼フォールバック |
| `dragapult-takeuchi` | Dragapult ex デッキの固定戦略・クチート竹内版ペルソナ |
| `dragapult-yopifutto` | Dragapult ex デッキの固定戦略・よぴふっと博士版ペルソナ |

戦略 bot は `decks/dragapult-ex.yaml` を持参デッキ (BYO) として前提にしています。
ロジックの「真値」(どう判断するか) は [`docs/bots/`](docs/bots/) の機械可読仕様にあります。

---

## セットアップ

Rust 1.80+ が必要です ([rustup](https://rustup.rs/))。

```sh
# submodule (カードデータ) ごと clone する
git clone --recurse-submodules https://github.com/1ulce/PTCG_AI_Battle_Client.git
cd PTCG_AI_Battle_Client

# すでに clone 済みなら submodule を取得
git submodule update --init --recursive

cargo build --release
```

---

## 対戦サーバの使い方

`connect` バイナリが、リモートの対戦サーバに WebSocket クライアントとして接続します。
公開アリーナは **`wss://arena.ptcgtools.com`** で運用しています (TLS、ポート 443)。

> ℹ️ アリーナはメンテナが運用しています。応答が無いときは停止中の可能性があります。

### いちばん簡単な例 — アリーナの内蔵 bot に挑む

```sh
cargo run --release --bin connect -- \
  --server wss://arena.ptcgtools.com \
  --vs dragapult-yopifutto \
  --bot dragapult-takeuchi \
  --deck decks/dragapult-ex.yaml \
  --games 3
```

### 自作 bot 同士を戦わせる (プライベートルーム)

同じ `--room` を指定した 2 接続が確実にペアになります。2 つの端末 (またはバックグラウンド)
で起動してください。

```sh
# 端末 1
cargo run --release --bin connect -- --server wss://arena.ptcgtools.com \
  --room myroom --bot dragapult-takeuchi --deck decks/dragapult-ex.yaml --games 5

# 端末 2
cargo run --release --bin connect -- --server wss://arena.ptcgtools.com \
  --room myroom --bot dragapult-yopifutto --deck decks/dragapult-ex.yaml --games 5
```

### 接続 intent

接続ごとに「どう相手を探すか」を指定します。

| 指定 | 意味 |
|---|---|
| (無指定) | open match — 誰でも先着 2 人がペアになる |
| `--room ID` | プライベートルーム — 同じ room の 2 人を確実にペア (自作 bot 同士に最適) |
| `--vs NAME` | サーバ内蔵 bot を相手に指名する |
| `--participant-id ID --auth-token TOK [--bucket B]` | ladder (レーティング対戦、ladder 対応サーバが必要) |

各接続が `--deck` を持参するので、**お互い別デッキの非対称対戦**もできます。

### `connect` のオプション

| フラグ | 既定 | 説明 |
|---|---|---|
| `--server URL` | (必須) | 接続先。`wss://HOST` (TLS, 既定 443) / `ws://HOST:PORT` (平文) |
| `--bot NAME` | `random` | `random` / `dragapult-takeuchi` / `dragapult-yopifutto` |
| `--deck PATH` | なし | 持参デッキ YAML |
| `--games N` | `1` | 繰り返し対戦数 (1 局 1 接続) |
| `--seed S` | `42` | 乱数シード (再現性) |
| `--cards-dir DIR` | `data/pokemon-card-data/cards` | カードデータの場所 |
| `--log-dir DIR` | `target/matches` | 棋譜の保存先 (常時保存。下記参照) |
| `--room` / `--vs` / `--participant-id` 他 | — | 上記 intent |

`connect --help` でも一覧できます。

### 棋譜ログ (常時保存)

`connect` は対戦ごとに棋譜を 1 ディレクトリ自動保存します。場所は
`<log-dir>/<UTC日時>-<bot>-vs-<相手>-seed<N>/` (既定 `target/matches/...`)。中身は 2 ファイル:

| ファイル | 用途 | 内容 |
|---|---|---|
| `match.log` | 人が読む | `request` ごとに盤面要約 (両者の場・HP・エネ数・状態異常・手札/山札/サイド枚数) + 選択肢一覧 + bot が選んだ手。`prompt` の選択、`event` の流れ、最後に `=== RESULT: ... ===` |
| `raw.jsonl` | 解析・リプレイ | 送受信した全メッセージを `{"t":"recv"\|"send","msg":{...}}` で 1 行ずつ。盤面 `state` / `legal_actions` / `prompt` / 応答 / `event` を完全な JSON で記録 |

ディレクトリ名の日時は **UTC**。日時はファイル名にしか使わず bot 判断・乱数シードには渡さないので、
`--seed` 固定の**再現性はそのまま**です。

```text
target/matches/2026-06-11T203914-dragapult-takeuchi-vs-dragapult-yopifutto-seed7/
├─ match.log     # 人間可読の棋譜
└─ raw.jsonl     # 全メッセージの JSON (1 行 1 メッセージ)
```

実際の出力例は [`docs/sample-match/`](docs/sample-match/) に収録 (上記 seed7 戦の `match.log` / `raw.jsonl`)。

### タイムコントロール (時間内に応答する必要あり)

公開アリーナは **サドンデス: 1 プレイヤーあたり合計 10 分 + 1 手 30 秒上限** で運用しています。
ここでの「1 手」は **1 往復**、つまり各 `request`→`response` と各 `prompt`→`choice` を指します。
どちらかの上限を超えると **時間切れ負け (`FlagFall`)** になります:

- 1 回の応答に **30 秒**より長くかかる、または
- 1 局を通した合計思考時間が **10 分**を超える。

接続したまま **ハング・無応答になった bot も負け**ます (その手で 30 秒後)。試合が永久に止まることはありません。

すべての `request` / `prompt` には `clock` が付きます。時間を管理したいなら使ってください:

- `my_remaining_ms` — 自分の**合計**残り時間。
- `my_deadline_unix_ms` — **この応答の絶対締切**(`now + min(合計残り, 1 手上限)`)。
  アリーナではおおよそ `now + 30 秒`。これより前に応答すること。

詳細は [`docs/protocol.md` の clock 節](docs/protocol.md#9-clock) を参照。収録のリファレンス bot は
ミリ秒で応答し clock を読みませんが、あなたの bot は読んでも構いません。

---

## 自分の bot を作る

このリポジトリの主目的は **「あなたが自分の bot を書く土台」** です。手順は 2 つだけ。

### 1. `BotPolicy` を実装する

`src/bots/<your_bot>.rs` を作り、[`BotPolicy`](src/bots/mod.rs) trait を実装します。

```rust
use rand_chacha::ChaCha20Rng;
use crate::bots::{BotPolicy, PromptChoice};
use crate::wire::action::ActionDto;
use crate::wire::protocol::{PromptMsg, RequestMsg};
use crate::transport::TransportError;

pub struct MyBot;

impl BotPolicy for MyBot {
    /// 自分の番の能動アクションを 1 つ選ぶ。`req.legal_actions` から選ぶだけ。
    fn choose_action(
        &mut self,
        req: &RequestMsg,
        rng: &mut ChaCha20Rng,
    ) -> Result<ActionDto, TransportError> {
        // req.state … 自分視点にマスクされた盤面 (StateDto)
        // req.legal_actions … サーバが列挙した合法手。AI は「選ぶだけ」
        // 例: とりあえず番を終える
        Ok(req
            .legal_actions
            .iter()
            .find(|a| matches!(a, ActionDto::EndTurn))
            .cloned()
            .unwrap_or_else(|| req.legal_actions[0].clone()))
    }

    /// 効果解決中の選択 (サーチ・ダメカン配分・コイン後の先攻後攻 など) に応答する。
    fn choose_prompt(&mut self, p: &PromptMsg, rng: &mut ChaCha20Rng) -> PromptChoice {
        // p.kind … 何を聞かれているか (PromptDto)。迷ったら RandomPolicy に委譲してよい。
        crate::bots::RandomPolicy.choose_prompt(p, rng)
    }
}
```

### 2. レジストリに登録する

`src/bots/mod.rs` の `build` と `available` に 1 行ずつ足すだけです。

```rust
// available()
&["random", "dragapult-takeuchi", "dragapult-yopifutto", "my-bot"]
// build()
"my-bot" => Some(Box::new(MyBot)),
```

これで `--bot my-bot` で動きます。

### bot から「見えるもの」

- **`req.state` (`StateDto`)** — 自分視点にマスクされた盤面。自分の手札は中身が見えますが、
  **相手の手札・山札は `card: null`** です (覗けない)。場・トラッシュ・スタジアムは全公開。
  → [`src/wire/state.rs`](src/wire/state.rs)
- **`req.legal_actions` (`Vec<ActionDto>`)** — サーバが審判として列挙した合法手。
  不正手は送れないので、AI は **列挙から選ぶだけ**で済みます。
- **カード事実** — HP やワザの index は盤面 DTO に全ては入っていません。
  [`CardFacts`](src/cards.rs) が slug から `is_ex` / `attack_index` を引きます
  (収録 bot は `decks/dragapult-ex.yaml` のカードを前提に index を解決しています)。

### 規律 (faithfulness)

- **合法手から選ぶ。** 不明な局面で「それっぽい手」を発明せず、`RandomPolicy` に委譲するのが安全。
- **乱数は `ChaCha20Rng` だけ**を使う (シード固定で再現可能)。`Instant`/`SystemTime` 等の
  非決定性を持ち込まない。
- 既存 bot のロジックを参考にするときは [`docs/bots/`](docs/bots/) の仕様も併読してください。

---

## プロトコルの概要

サーバと交わす JSON の**完全なリファレンス**は **[`docs/protocol.md`](docs/protocol.md)** にあります
(各メッセージのキー・型・エラーコード・プロンプトへの応答の仕方・情報マスキングまで実装と一致する形で記載)。
他言語で bot を書くときもここを見れば実装できます。型定義の一次ソースは [`src/wire/`](src/wire/)、
互換性は `tests/wire_contract.rs` で固定しています。

要点だけ:

- **WebSocket** で `wss://HOST/ai-battle/v1/connect` (TLS) または `ws://HOST:PORT` に接続。1 フレーム = 1 JSON。
- サーバ → AI: **`ServerMessage`** (`subscribed` / `event` / `request` / `prompt` / `ping` / `error`)
- AI → サーバ: **`ClientMessage`** (`subscribe` / `response` / `choice` / `pong`)
- 2 系統: 全体に流れる **event ストリーム** と、判断を求める **request/prompt → response/choice**。
- **情報マスキング**: サーバが視点ごとに盤面を隠すので、相手の手札は見えません (cheat 不可)。
- `connect` の対戦ループの実体は [`src/bin/connect.rs`](src/bin/connect.rs) にあります。

---

## プロジェクト構成

```
src/
├─ wire/        # サーバと互換の JSON DTO (protocol / state / action / event)
├─ cards.rs     # CardFacts: pokemon-card-data から ex 判定 / ワザ index を引く
├─ deck.rs      # DeckList (持参デッキ YAML)
├─ transport.rs # tungstenite WebSocket クライアント
├─ bots/        # BotPolicy trait + 収録 bot (random / 竹内 / よぴふっと)
└─ bin/connect.rs  # subscribe → Request/Prompt 応答ループ
decks/          # dragapult-ex.yaml (持参デッキ)
docs/bots/      # bot 戦略の機械可読仕様
data/pokemon-card-data/  # カードマスタ (submodule)
tests/wire_contract.rs   # サーバとの JSON 契約テスト
```

ローカル検証:

```sh
cargo test                       # bot ロジック + wire 契約テスト
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

---

## ルールエンジンとの関係

このリポジトリは **bot とクライアントだけ**を含み、ルールエンジン (審判) は
PTCG AI Battle Platform 側にあります。
だからこそ engine に依存せず、誰でも軽量に bot を書けます。収録 bot のロジックと wire DTO は
本体を「真値」として片方向に同期しています (詳細は [CLAUDE.md](CLAUDE.md))。

---

## ライセンス

`MIT OR Apache-2.0` のデュアルライセンス
([LICENSE-MIT](LICENSE-MIT) / [LICENSE-APACHE](LICENSE-APACHE))。
submodule の `data/pokemon-card-data` はそのリポジトリのライセンスに従います。

Issue / Pull Request 歓迎です。新しい bot や別アーキタイプのデッキを持ち寄ってください。
