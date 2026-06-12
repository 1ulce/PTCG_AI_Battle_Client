# 通信プロトコル リファレンス

bot を書くために必要な「サーバと交わす JSON の形」を、**実装と一致する形で**まとめたものです。
型定義の一次ソースは [`src/wire/`](../src/wire/)、互換性は
[`tests/wire_contract.rs`](../tests/wire_contract.rs) で固定しています。他言語で bot を書くときも
ここを見れば実装できます。

> ⚠️ **entity_id は数値 (`u32`)** です。`"e-217"` のような文字列ではありません。
> プレイヤーは場面によって `"me"`/`"opp"` (視点) と `"p1"`/`"p2"` (固定 ID) を使い分けます。

---

## 1. 接続

- WebSocket: `ws://HOST:PORT/ai-battle/v1/connect`
- **1 WebSocket テキストフレーム = 1 JSON オブジェクト**。
- WebSocket サブプロトコルヘッダ (`Sec-WebSocket-Protocol`) は**送らない** (サーバは negotiate しない)。
- フレームレベルの ping/pong は WebSocket ライブラリが自動応答 (アプリ層の `ping`/`pong` とは別物)。

接続したら最初に [`subscribe`](#subscribe-ai--sv) を送り、以降は受信メッセージに応答し続けます。

---

## 2. メッセージの基本

すべてのメッセージは `type` フィールドで種別を識別します (snake_case)。

| 方向 | `type` | 役割 |
|---|---|---|
| AI → SV | `subscribe` | 対戦に参加 / 再接続 |
| SV → AI | `subscribed` | 参加確認 |
| SV → AI | `event` | 局面変化通知 (両 AI へ) |
| SV → AI | `request` | 能動アクションの選択要求 |
| AI → SV | `response` | `request` への応答 |
| SV → AI | `prompt` | 解決中の選択要求 |
| AI → SV | `choice` | `prompt` への応答 |
| SV → AI | `ping` | 死活確認 |
| AI → SV | `pong` | 死活応答 |
| SV → AI | `error` | エラー通知 |

bot が最低限ハンドルすべきは **`request` / `prompt` / `event`(game_end) / `ping`** の 4 つです。

---

## 3. サーバ → AI (`ServerMessage`)

### `subscribed`

`subscribe` への応答。対戦が始まる合図。

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

| キー | 型 | 説明 |
|---|---|---|
| `your_player` | `"p1"` \| `"p2"` | 自分の固定プレイヤー ID |
| `server_current_seq` | uint | 現在発番済みの最新 event seq |
| `opponent` | object | `{ai_id, display_name}` |
| `time_control` | object | `kind` = `sudden_death`/`increment`/`byoyomi`/`correspondence` + 各 `*_ms` |
| `session_token` | string | 再接続用トークン (空のこともある) |

### `request` — 能動アクションを選ぶ

自分の番、または割り込み判断が必要なとき。bot のメインの仕事はこれへの応答です。

```json
{
  "type": "request",
  "request_id": "r-0050",
  "resent": false,
  "state": { "...": "§5 state スキーマ" },
  "legal_actions": [
    { "id": "use_attack", "attack_index": 1 },
    { "id": "end_turn" }
  ],
  "clock": { "my_remaining_ms": 0, "opp_remaining_ms": 0, "running_for": "me" }
}
```

| キー | 型 | 説明 |
|---|---|---|
| `request_id` | string | 応答で参照する ID。同じ ID への応答は 1 回だけ |
| `resent` | bool | 再接続で再送された request なら `true` |
| `state` | object | 自分視点にマスクされた盤面 ([§5](#5-state-スキーマ)) |
| `legal_actions` | array | **取りうる全アクションの列挙** ([§6](#6-アクション-actiondto))。この中から 1 つ選ぶ |
| `clock` | object | 時計スナップショット ([§9](#9-clock)) |

→ [`response`](#response-ai--sv) で `legal_actions` のどれか 1 つをそのまま返す。

### `prompt` — 効果解決中の選択

`response` の後・次の `request` の前に挟まる、サーチ・ダメカン配分・コイン後の先攻後攻などの選択。

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

| キー | 型 | 説明 |
|---|---|---|
| `request_id` | string | この prompt の ID (`choice` で参照) |
| `parent_request_id` | string | 起因となった request の ID |
| `kind` | string | プロンプト種別 ([§7](#7-プロンプト-promptdto-と-応答)) |
| `min`, `max` | uint | 選ぶ個数の下限・上限 |
| `state` | object? | その時点のマスク盤面 (省略されることがある) |
| `shuffle_after` | bool? | 選択後にゾーンがシャッフルされるか |
| (kind 固有) | — | `options` / `targets` / `eligible` など ([§7](#7-プロンプト-promptdto-と-応答)) |

`kind` ごとのフィールドと応答方法は [§7](#7-プロンプト-promptdto-と-応答) を参照。
→ [`choice`](#choice-ai--sv) で答える。

### `event` — 局面変化通知

両 AI にブロードキャストされる状態変化。**bot が必ず見る必要があるのは `game_end` だけ**で、
他は無視しても対戦は成立します (盤面は次の `request.state` に反映される)。

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

| キー | 型 | 説明 |
|---|---|---|
| `seq` | uint | マッチ内の単調増加連番 (欠番なし) |
| `actor` | `"me"`\|`"opp"`\|`"system"` | 誰の行動か (受信者視点) |
| `replayed` | bool | 再接続の再生中なら `true` |
| `kind` | string | event 種別 ([§8](#8-イベント-eventdto)) |
| `data` | object? | kind 固有のペイロード (`turn_end` 等データ無しの kind では省略) |
| `clock` | object? | ターン境界等で同梱 |

**終局** (`kind: "game_end"`):

```json
{ "type": "event", "seq": 298, "actor": "system", "timestamp_unix_ms": 0,
  "replayed": false, "kind": "game_end",
  "data": { "winner": "p1", "reason": "PrizeTaken" } }
```

- `winner`: `"p1"` \| `"p2"`。引き分けなら省略 (キーなし)。
- `reason`: 文字列。実際の値は `PrizeTaken` / `DeckOut` / `NoBenchOnKo` / `FlagFall` /
  `Concession` 等 (PascalCase)。
- `game_end` 受信後、サーバは追加 event を送りません。接続を閉じてよい。

### `ping`

```json
{ "type": "ping", "server_current_seq": 327, "server_time_unix_ms": 0 }
```

→ [`pong`](#pong-ai--sv) を返す。

### `error`

```json
{ "type": "error", "code": "illegal_action",
  "message": "...", "related_request_id": "r-0050", "fatal": false }
```

| `code` | 意味 |
|---|---|
| `illegal_action` | `response.action` が `legal_actions` に無い |
| `illegal_choice` | `choice.selected` が options に無い or 個数違反 |
| `unknown_request_id` | 存在しない request_id への応答 |
| `from_seq_too_large` | `subscribe.from_seq` が大きすぎる |
| `invalid_session_token` | session_token 不一致 |
| `protocol_violation` | 必須フィールド欠落・型違反 |
| `internal_error` | サーバ内部エラー (試合中断) |

`fatal: true` のときは接続が切断されます。`illegal_action` / `illegal_choice` は `fatal: false` で、
**同じ request_id の応答を再度待ち**ます。

---

## 4. AI → サーバ (`ClientMessage`)

### `subscribe` (AI → SV)

接続後に最初に送る。中身の **intent** で対戦に振り分けられます。

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

| キー | 型 | 説明 |
|---|---|---|
| `match_id` | string | 新規は空。再接続時のみ値 |
| `session_token` | string | 再接続時のみ |
| `from_seq` | uint | 初回は `0` |
| `participant_id` / `auth_token` / `bucket` | string | ladder intent 用 (それ以外は空) |
| `room` | string | 同じ room の 2 接続を確実にペア (空=open) |
| `vs_bot` | string | サーバ内蔵 bot 名を相手に指名 (空=なし) |
| `decklist` | object? | 持参デッキ。`{name?, cards:[{slug, count}]}`。サーバが resolve・検証する |

intent の優先順 (サーバ側): 既知 `session_token` → 再接続 / `vs_bot` → vs-bot /
`participant_id` → ladder / `room` → ルーム / それ以外 → open。

### `response` (AI → SV)

```json
{ "type": "response", "request_id": "r-0050",
  "action": { "id": "use_attack", "attack_index": 1 } }
```

`action` は `legal_actions` のいずれかと**完全一致**するオブジェクトを返します。

### `choice` (AI → SV)

```json
{ "type": "choice", "request_id": "p-0050-1",
  "selected": [77], "counts": [], "yes": null, "branch_index": null }
```

| キー | 型 | 説明 |
|---|---|---|
| `selected` | `[uint]` | 選んだ entity_id 等のリスト。`[min, max]` 範囲内 |
| `counts` | `[uint]`? | 個数・配分 (空なら省略可) |
| `yes` | bool? | yes/no・先攻後攻 (不要なら省略) |
| `branch_index` | uint? | 分岐選択の index (不要なら省略) |

`kind` ごとにどれを埋めるかは [§7](#7-プロンプト-promptdto-と-応答)。

### `pong` (AI → SV)

```json
{ "type": "pong", "last_seen_seq": 0 }
```

`last_seen_seq` は最後に受信した event の seq (簡易実装では `0` でよい)。

---

## 5. state スキーマ

`request.state` (および一部 `prompt.state`) の中身。すべて**自分視点にマスク済み**。

```json
{
  "turn": 6,
  "phase": "main",
  "active_player": "me",
  "stadium": { "entity_id": 440, "card": "jamming-tower" },
  "me":  { "...": "PlayerView (全公開)" },
  "opp": { "...": "PlayerView (手札・山札・サイドは隠れる)" }
}
```

- `phase`: `"setup"` / `"draw"` / `"main"` / `"between_turns"` / `"ended"` など。
- `active_player`: `"me"` / `"opp"`。
- `stadium`: 場のスタジアム (無ければキーなし)。

### `PlayerView`

```json
{
  "active": { "...": "PokemonInPlay (無ければキーなし)" },
  "bench": [ "...PokemonInPlay × 0〜5" ],
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

| キー | 型 | 説明 |
|---|---|---|
| `active` | PokemonInPlay? | バトル場 (居なければキーなし) |
| `bench` | array | ベンチ 0〜5 体 |
| `hand` | `[EntityDto]` | 手札。**自分は `card` 入り、相手は `card` なし (null)** |
| `deck_size` | uint | 山札の枚数 (中身・順序は非公開) |
| `discard` / `lost_zone` | `[EntityDto]` | 全公開 |
| `prizes` | `[EntityDto]` | サイド。**枚数だけ意味があり `card` は基本 null** |
| `*_this_turn` | bool | この番にエネ手張り / サポート使用済みか |
| `had_ko_last_turn` | bool | 前の相手の番に自分のポケモンが KO されたか |

### `EntityDto`

```json
{ "entity_id": 30, "card": "boss-s-orders" }
```

`card` は slug。未公開なら**キー自体が無い** (= null)。

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

| キー | 型 | 説明 |
|---|---|---|
| `stage` | string | `"basic"` / `"stage_1"` / `"stage_2"` 等 |
| `evolution_stack` | `[uint]` | 進化元の entity_id |
| `hp_max` / `damage` | uint | 最大 HP / 乗っているダメージ (ダメカン×10) |
| `energy_attached` | `[EntityDto]` | 付いているエネルギー |
| `tool_attached` | EntityDto? | 付いているどうぐ (無ければキーなし) |
| `status_conditions` | `[string]` | `poisoned`/`burned`/`asleep`/`paralyzed`/`confused` ([§10](#10-状態異常)) |
| `abilities_used_this_turn` | `[uint]` | この番に使った特性の index |
| `turn_in_play` | uint | この個体が場に出てからの番数 (召喚酔い判定等) |

---

## 6. アクション (`ActionDto`)

`legal_actions` の各要素 / `response.action`。`id` で種別を識別 (snake_case)。

| `id` | 追加フィールド | 説明 |
|---|---|---|
| `play_card` | `entity_id`, `target`? | 手札のカード (グッズ/サポート/スタジアム/エネ/ポケモン/進化) を使う |
| `use_ability` | `entity_id`, `ability_index` | 場のポケモンの起動特性 |
| `use_in_hand_ability` | `entity_id`, `ability_index` | 手札のカードの起動特性 |
| `use_stadium_effect` | `stadium_entity_id` | 起動型スタジアム |
| `retreat` | `to_bench_index`, `energy_to_discard`? | にげる (ベンチ N と入替、捨てるエネ entity_id 列) |
| `use_attack` | `attack_index` | バトル場のワザ (エネ条件はサーバ確認済み) |
| `end_turn` | — | 番を終える |
| `discard_fossil` | `entity_id` | 場の化石を任意トラッシュ |
| `concede` | — | 投了 |

`target` (`play_card` 等の対象) は `kind` 付きオブジェクト:

| `target.kind` | 追加 | 説明 |
|---|---|---|
| `own_active` / `opp_active` | — | 自分 / 相手のバトル場 |
| `own_bench` / `opp_bench` | `index` | 自分 / 相手のベンチ N 番 |
| `stadium` | — | スタジアム |

例: `{ "id": "play_card", "entity_id": 30, "target": { "kind": "opp_active" } }`

> **選ぶだけでよい。** `response` では `legal_actions` の要素をそのまま返せば、フィールドを
> 自前で組み立てる必要はありません。

---

## 7. プロンプト (`PromptDto`) と 応答

`prompt.kind` ごとのフィールドと、`choice` で埋めるフィールドの対応。**これが「レスポンスの仕様」**です。

| `kind` | prompt のフィールド | `choice` で埋める |
|---|---|---|
| `choose_from_zone` | `zone`, `options:[EntityDto]` | `selected` = 選ぶ entity_id ([min,max] 個) |
| `choose_target_pokemon` | `targets:[uint]` | `selected` = `[1 体]` |
| `choose_initial_active` | `eligible:[uint]` | `selected` = `[1 体]` (最初のバトル場) |
| `place_initial_bench` | `eligible:[uint]`, `bench_max` | `selected` = ベンチに置く subset |
| `replace_active_after_ko` | `bench_options:[uint]` | `selected` = `[1 体]` (KO 後の繰り出し) |
| `distribute_damage` | `eligible:[uint]`, `total`, `per_target_max`? | `selected` = 対象、`counts[i]` = selected[i] に乗せる数 |
| `attach_energy_to` | `energy_options:[uint]`, `pokemon_eligible:[uint]` | `selected` = `[エネ, ポケモン]` |
| `discard_from_attached` | `eligible:[uint]`, `kind_filter` | `selected` = 剥がす entity |
| `reorder_cards` | `cards:[uint]`, `destination` | `selected` = 並べ替えた順 |
| `peek_and_reorder` | `peeked:[uint]`, `destination` | `selected` = 並べ替えた順 |
| `select_ability_order` | `entries:[uint]` | `selected` = 解決順 |
| `assign_energy_to_targets` | `energies:[uint]`, `pokemon_eligible:[uint]` | `selected` = エネ subset、`counts[i]` = `pokemon_eligible` の index |
| `pick_amount_from_each` | `sources:[[uint,uint]]`, `dest` | `counts[i]` = `sources[i]` から取る数 |
| `choose_yes_no` | `prompt_text` | `yes` = true/false |
| `choose_first_or_second` | (なし) | `yes` = true (自分が先攻) / false |
| `choose_one_branch` | `branch_count`, `labels:[string]` | `branch_index` = 0..branch_count |
| `choose_opponent_attack` | `attack_count`, `labels:[string]` | `branch_index` = 0..attack_count |
| `choose_status_to_remove` | `target`, `statuses:[string]` | `branch_index` = `statuses` の index |
| `pick_attack_to_copy` | `candidates:[[uint,[string]]]` | `selected` = `[ポケモン]`、`branch_index` = そのワザの index |
| `prize_hand_swap_choice` | `prize_options:[uint]`, `hand_options:[uint]` | `selected` = `[prize, hand]`、`yes` = 入替するか |

> 迷ったら `random` bot の実装 ([`src/bots/random.rs`](../src/bots/random.rs)) が全 kind の無難な応答例です。
> 該当しないフィールドは省略 (`counts: []` / `yes: null` / `branch_index: null`) で構いません。

---

## 8. イベント (`EventDto`)

`event.kind` 一覧。bot が応答する必要はなく、参考情報です (盤面は次の `request.state` に反映される)。

| `kind` | 主な `data` | | `kind` | 主な `data` |
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
| `internal` | `name` (粗粒度通知) | | `live_caught_up` | — (再接続の再生終了) |

> サーバが将来 event を追加しても壊れないよう、**知らない kind は無視**してください。

---

## 9. clock

`request` / `prompt` および一部 `event` に同梱:

```json
{ "my_remaining_ms": 487320, "opp_remaining_ms": 512840,
  "running_for": "me", "my_deadline_unix_ms": 1746541842500 }
```

- `my_remaining_ms` / `opp_remaining_ms`: **総持ち時間の残り**（ミリ秒）。
- `running_for`: `"me"` / `"opp"` / `"none"`（停止中）。
- `my_deadline_unix_ms`（`running_for == "me"` のときのみ）: **この応答の実効締切**（絶対 unix ms）。
  `now + min(総残り, 1手上限)` で計算され、**1手ごとの上限を反映するので総残りより手前になりうる**。
  bot はこの時刻までに `response`/`choice` を返さないと時間切れ負け。`my_deadline_unix_ms - 今の時刻`
  で「この手に使える残り時間」が分かる。無制限のときは省略（締切なし）。
- 例: アリーナは「全体10分 + 1手30秒」なので、序盤でも `my_deadline_unix_ms ≈ now + 30s`。
- `my_remaining_ms` はネットワーク遅延の影響を受けるので、シビアに使うなら `my_deadline_unix_ms`
  を自分のローカル時計と比べるほうが正確。

---

## 10. 状態異常

`status_conditions` / `apply_status` の値:

| `status` | 効果 |
|---|---|
| `asleep` | 行動できない (ねむり) |
| `burned` | ポケモンチェックでダメカン + コインで回復 (やけど) |
| `confused` | ワザ宣言時コイン、裏で失敗 + 自分にダメージ (こんらん) |
| `paralyzed` | 行動できない、次の番終了時に解除 (まひ) |
| `poisoned` | ポケモンチェックでダメカン (どく) |

---

## 11. 情報マスキング (cheat 防止)

サーバは内部で完全状態を持ち、視点ごとに隠してから送ります。bot から**見えないもの**:

- 相手の手札の中身 (`opp.hand` の各 entity は `card` なし = null。枚数だけ分かる)
- 自分・相手の山札の中身 (`deck_size` のみ。順序も非公開)
- サイド (`prizes`) の中身 (entity はあるが `card` は基本 null)

**見えるもの**: 自分の手札 / 両者の場 (バトル場・ベンチ・付いているエネ・どうぐ) /
トラッシュ / ロストゾーン / スタジアム。

`entity_id` はマッチ中ずっと同じカードを指すので、「あのときサーチされた札」を後から追跡できます。
