# Dragapult ex BOT（クチート竹内版）— 機械可読戦略仕様（正規化版）

> **出典**: [`dragapult-ex.takeuchi.md`](dragapult-ex.takeuchi.md)（ユーザー提供の人間向け原文、クチート竹内版、2026-06-09）。
> 本ファイルは原文から**演出・口上・物理動作を除去**し、**意思決定のみを決定的ステップ機械**として
> 正規化したもの。実装は `crates/bots/src/dragapult_takeuchi.rs`（`DragapultTakeuchiBot`、`BotPolicy` 実装）にあり、
> この正規化版を直接の参照とする。bot 共通の枠組み（trait / レジストリ）は `crates/bots/src/lib.rs`、
> `ptcg-cli` からは `--bot dragapult-takeuchi` で選択する。
> 原文と本ファイルが食い違ったら**原文を正**とし、本ファイルを再正規化する。
> 人間の指示はバグ（対象未指定・条件漏れ・矛盾）を含む前提。補完できないものは
> §9 GAP に列挙し、勝手に意味を足さない。実行時の既定挙動は §2 のグローバル方針に従う。

---

## 1. 用語 ↔ slug 対応（`decks/dragapult-ex.yaml`）

| 戦略の呼称 | slug | 種別 |
|---|---|---|
| ドラメシヤ | `dreepy` | たね |
| ドロンチ | `drakloak` | 1進化（←dreepy） |
| ドラパルトex | `dragapult-ex` | 2進化（←drakloak）/ ex |
| ヨマワル | `duskull` | たね |
| サマヨール | `dusclops` | 1進化（←duskull） |
| ヨノワール | `dusknoir` | 2進化（←dusclops） |
| ニャースex | `meowth-ex` | たね / ex |
| キチキギスex | `fezandipiti-ex` | たね / ex |
| スボミー | `budew` | たね |
| ハイパーボール | `ultra-ball` | グッズ |
| ふしぎなアメ | `rare-candy` | グッズ |
| なかよしポフィン | `buddy-buddy-poffin` | グッズ |
| ポケパッド | `poke-pad` | グッズ |
| 夜のタンカ | `night-stretcher` | グッズ |
| アンフェアスタンプ | `unfair-stamp` | グッズ / ACE SPEC |
| スペシャルレッドカード | `special-red-card` | グッズ |
| リーリエの決心 | `lillie-s-determination` | サポート |
| アカマツ | `crispin` | サポート |
| ボスの指令 | `boss-s-orders` | サポート |
| ジャミングタワー | `jamming-tower` | スタジアム |
| ロケット団の監視塔 | `team-rocket-s-watchtower` | スタジアム |
| 炎エネルギー | `fire-energy` | 基本エネ |
| 超エネルギー | `psychic-energy` | 基本エネ |

関連特性・ワザ（要 POL 実装確認）:
- `ていさつしれい`（`drakloak`）/ `おくのてキャッチ`（`meowth-ex`）/ `カースドボム`（`dusknoir`）
- `ファントムダイブ` `ジェットヘッド`（`dragapult-ex`）/ `むずむずかふん`（`budew`）

---

## 2. グローバル方針（全ステップ共通・最優先）

- **G1（対象未指定→ランダム）**: ある行動が対象を取るのに、戦略が対象を一意に決めていない場合、
  **条件に合致する合法な候補からランダムに1つ**選ぶ。候補が0なら行動自体をスキップ。
- **G2（ステップ順次）**: 各ターンは STEP を番号順に評価。各 STEP は「**実行可能なら実行、
  不可能ならスキップして次の STEP へ**」。
- **G3（PICK-FIRST 優先度）**: 「優先度リスト」は上から評価し、**ガード条件を満たす最初の候補**を選ぶ。
- **G3a（優先順位フォールスルー = 常識）**: 優先順位リストは、**上位の取得対象が手に入らなければ
  自動的に次位へ落ちる**。tier を「上位 tier が空のときだけ下位を見る」と排他にしてはならない
  （複数 tier は全て連結したフォールスルー型の単一リストにする）。例: poke-pad で最優先 `budew` が
  山札に無ければ次 tier (`dreepy` 等) → 既定 `budew` の順で、山札にある最初の候補を取る
  （2026-06-10 ユーザー確定。SKILL「優先順位フォールスルー」と同義）。
- **G3b（同ランク衝突→ランダム）**: 複数の候補が**同ランク**（原文で複数項目がともに「最優先」等）
  で並び、ガードを同時に満たす場合は、その候補群から **G1（ランダム）** で1つ選ぶ
  （2026-06-09 ユーザー確定「C」）。総順序へ無理に倒さない。
- **G4（未規定→ランダム合法手）**: どの RULE にも当てはまらず、かつ STEP が「やること」を指定して
  いない場合は、その局面で**合法手からランダム**に選ぶ（最終手段。番を進める方向＝可能なら end_turn を含む）。
- **G5（演出除去）**: シャッフル / 口上 / 5分スリープ / 「カメラに見えるように」/ ジャンケンの掛け声 /
  「右端」「1番下」等の物理位置指定は**実装しない**。位置指定が対象選択に化けている箇所は G1 で
  ランダム化する（例: ハイパーボールの「右端2枚トラッシュ」→ トラッシュ対象2枚をランダム）。
- **G6（ジャンケン）**: 勝ち→**後攻**を選択 / 負け→相手の選択に従う（掛け声は無視）。
  ✅ 実装済 (2026-06-09): エンジンに `ChooseFirstOrSecond` prompt を新設し、コイン勝者に
  先攻/後攻の選択権を持たせた (`yes=true`→先攻)。DragapultBot は勝者のとき `yes=Some(false)`
  で後攻を選ぶ。当初「protocol に選択肢が無く実装不能か」と懸念したが、エンジン拡張で対応。
- **G7（REPEAT 明示）**: 「全て出す」「なるべく進化させる」等は、対象が尽きるか合法でなくなるまで
  STEP 内で反復（`REPEAT` と明記）。

---

## 3. 述語（predicate）定義

仕様中で繰り返し使う条件。`canX(...)` は真偽、`P_X` は局面フラグ。

- `cost_met(p, attack)` … ポケモン `p` の装着エネが `attack` のコストを満たす（コストは当該カード
  YAML を正とする）。**NOTE**: 戦略は「炎+超 両方」を頻繁に要求 → `ファントムダイブ` は概ね
  炎1・超1 を含む（厳密値は `dragapult-ex.yaml` 参照）。
- `canPhantomDive(p)` ≡ `p.slug == dragapult-ex && cost_met(p, ファントムダイブ)`。
- `canJetHead(p)` ≡ `p.slug == dragapult-ex && cost_met(p, ジェットヘッド)`。
- `P_phantomReadyActive` ≡ バトル場が dragapult-ex かつ `canPhantomDive(バトル場)`。
- `P_phantomReadyBench` ≡ ベンチに `canPhantomDive` な dragapult-ex が存在。
- `P_bossUse`（ボスの指令を**使う**条件）≡
  「ファントムダイブで相手 ex を倒せる」**OR**「ファントムダイブで相手ポケモンを2匹以上倒せる」
  **OR**「自分のサイド残り = 1」。
- `P_bossWin`（ボスの指令で**勝つ**条件）≡
  「ファントムダイブで相手を倒してサイドを取り切れる」**OR**「自分のサイド残り = 1」。
- `P_phantomUnlikely`（「ファントムダイブが使えなそうな番」）≡
  **判定時点で、ファントムダイブが現在の合法な技選択肢に含まれていない**。
  逆に「使えそう（使える番）」= 判定時点でファントムダイブが技選択肢にある。
  **手札先読みはしない**（「〇〇を付ければ使える」等は検索が重いため考慮しない。2026-06-09 確定）。
  実装上は「バトル場が `dragapult-ex` かつ `cost_met(バトル場, ファントムダイブ)`」の真偽でよい
  （= `P_phantomReadyActive`）。各 STEP の判定タイミングでの現況で評価する。
- `energyOverflow(p, type)` ≡ `p` に `type` を付けると同種2個（炎炎 または 超超）になる。
  エネ付与は常に `!energyOverflow` を満たす type を選ぶ（G1 で type 未定なら満たす中からランダム）。
- `isFirstTurnGoingSecond` ≡ 後攻側の自分の最初の番。

---

## 4. セットアップ（ゲーム開始時）

- `S1` ジャンケン: §G6。
- `S2` バトル場の最初の1体（PICK-FIRST、手札の出せるたねから）:
  - **先攻**: `dreepy` → `budew` → `duskull` → `fezandipiti-ex` → `meowth-ex`
  - **後攻**: `budew` → `dreepy` → `duskull` → `fezandipiti-ex` → `meowth-ex`
- `S3` ベンチ展開・マリガン処理はエンジン既定（手札にたねがなければ規定のマリガン）。**GAP-2**:
  原文はベンチ初期配置を指定しない → セットアップ段のベンチは G4（ランダム合法）に委ねる。

---

## 5. 共通サブルーチン

各ターン STEP から呼ぶ。すべて「合法な範囲で・対象未指定は G1」。

### 5.1 `PLACE_BASICS`（たねをベンチに出す）— REPEAT
ベンチに空きがある間、手札のたねを PICK-FIRST で出す:
`dreepy` → （番により ヨマワル/スボミーの順が変わる。各ターン STEP 側の指定を優先）。
**ex（`meowth-ex` / `fezandipiti-ex`）は「条件を満たさない限り出さない」**（出す条件は各 STEP が明示した時のみ）。

### 5.2 `EVOLVE_ALL`（進化させる）— REPEAT
進化可能な対象を、進化優先度 PICK-FIRST で進化させる:
`dragapult-ex`（最優先）→ `drakloak` → `dusclops` → `dusknoir`。
- ふしぎなアメ（`rare-candy`）による進化もこのタイミングで行う（→ `RARE_CANDY` の対象規則）。
- **進化抑制ルール（ドロンチ止め）**: 場に `dragapult-ex` が1匹以上いる場合、`drakloak`→`dragapult-ex`
  の進化は**行わず** `drakloak` で止める。場に `dragapult-ex` がいなくなったら `drakloak` を
  `dragapult-ex` に進化させてよい。

### 5.3 `GOODS`（ポケモンを加える/その他グッズ）
グッズ使用の全体優先度（PICK-FIRST、使える物から）:
`poke-pad` → `buddy-buddy-poffin` → `ultra-ball` → `night-stretcher`。
加えたポケモンは**直後に場に出すか進化させる**（`PLACE_BASICS` / `EVOLVE_ALL` を再実行）。

- **`poke-pad` の取得対象**:
  - **最優先 tier**（原文で「最優先」表記。ガードを満たすものを集め、複数なら G3b でランダム）:
    - （スボミー条件）`isFirstTurnGoingSecond` または `P_phantomUnlikely` → `budew`
    - 場に「進化できる `dreepy`」がいる → `drakloak`
      （「進化できる」= 召喚酔いでない = 自分の番で `turn_in_play >= 1`。`turn_in_play` は自番開始ごと
      に +1・配置直後は 0 なので、この番に出したばかりの `dreepy` を除外する。2026-06-10 ユーザー確定で
      従来の「場にいる」近似から厳密化）
  - **次 tier**（最優先 tier の後に連結。最優先の取得対象が山札に無ければここへフォールスルー。PICK-FIRST 上から）:
    1. 場に `dreepy` がいない → `dreepy`
    2. 場に `drakloak` ≥2 かつ 場に `duskull` がいる →
       （手札に `rare-candy` かつ `duskull` がベンチ）? `dusknoir` : {`dusclops`,`dusknoir`} から G1
    3. 場に `drakloak` ≥2 かつ 場に `duskull` がいない → `duskull`
  - **既定（最低）**: `budew`

- **`buddy-buddy-poffin` の取得対象**（たね限定。PICK-FIRST）:
  1. （スボミー条件）`isFirstTurnGoingSecond` または `P_phantomUnlikely` → `budew`
  2. `dreepy`
  3. `duskull`（`dreepy`+`drakloak`+`dragapult-ex` の合計 ≥3 のとき）

- **`ultra-ball` の手順**:
  - トラッシュ: **手札からランダム2枚**（原文「右端2枚」は物理位置指定のため G5 でランダム化）。
    温存・最適化ロジックは設けない（原文・公式ルールに根拠なし。GAP-3 参照）。手札が
    トラッシュ2枚を捻出できない場合は使用不可。
  - 取得対象:
    - **最優先 tier**（ガードを満たすものを集め、複数なら G3b でランダム。`meowth-ex` 系は重複排除）:
      - 手札にサポートが無い **OR** 手札のサポートが `boss-s-orders` のみ → `meowth-ex`
      - `P_bossWin` → `meowth-ex`
      - （スボミー条件）`isFirstTurnGoingSecond` または `P_phantomUnlikely` → `budew`
    - **次 tier**（最優先 tier の後に連結。最優先の取得対象が山札に無ければここへフォールスルー。PICK-FIRST 上から）:
      1. 場に `dragapult-ex` も `drakloak` もいない → `drakloak`
      2. エネ付き `drakloak` がいる → `dragapult-ex`
      3. エネ付き `dreepy` がいる かつ 手札に `rare-candy` → `dragapult-ex`
      4. `dusclops`
      5. `dusknoir`
    - **既定（最低）**: `budew`

- **`night-stretcher` の取得対象**: その STEP で必要としているポケモン（無指定なら、トラッシュの
  回収可能候補から G1）。「1番下」は G5 で無視。

### 5.4 `RARE_CANDY`（ふしぎなアメ）対象（PICK-FIRST）
1. （炎+超 付き `dreepy` がいる）かつ（手札に `dragapult-ex`） → その `dreepy` を `dragapult-ex` に
2. （既定）`duskull` を `dusknoir` に
ただし §5.2 の進化抑制ルール（ドロンチ止め／ドラパルト不在時の解除）が `dragapult-ex` 化に優先。

### 5.5 `ENERGY_ATTACH`（手張り）
- 付与先 PICK-FIRST: `dragapult-ex` → `drakloak` → `dreepy`（各ターン STEP が別指定する場合はそちら優先）。
- **ベンチ優先**（原文「ベンチからつけて」＝「ベンチにつけることを優先して」の意）: 付与先は
  まず**ベンチのポケモン**に限定して上記 PICK-FIRST を適用し、ベンチに該当候補がいなければ
  バトル場の該当ポケモンへ。各ステップが付与先を明示した場合・にげエネ確保が必要な場合はそちらを優先。
- type は `!energyOverflow`（炎炎/超超を作らない）を満たすものから選ぶ。STEP が「超優先」等を
  指定したらそれに従い、なお `!energyOverflow` を満たす範囲で。満たす type が無ければ付与スキップ。
- 同一カードに同タイプ2個（炎炎/超超）を作らない、を不変条件とする。

### 5.6 `SUPPORT`（サポート使用）— 各ターン STEP の分岐に従う
共通の取得元: `meowth-ex`「おくのてキャッチ」でサポートを手札に加えられる（場に出して使用）。
- 手札に使えるサポートが無い、または `boss-s-orders` のみで `P_bossUse` 不成立のとき:
  `ultra-ball` で `meowth-ex` を出し「おくのてキャッチ」で**サポートを補充**（最優先）。
- `meowth-ex` で持ってくるサポートの優先度:
  1. （`dragapult-ex` がいて `ファントムダイブ` のエネ未充足）→ `crispin`（アカマツ）
     - アカマツの付与先 PICK-FIRST: `dreepy`/`drakloak`/`dragapult-ex`（`!energyOverflow`）
  2. `P_bossUse` → `boss-s-orders`
     - ボスで呼ぶ相手 = **ダメカン無し かつ `budew` 以外 かつ HP ≤ 200** の相手ポケモン（複数は G1）
  3. （既定）`lillie-s-determination`（リーリエの決心）

### 5.7 `CURSED_BOMB`（`dusknoir` カースドボム）
発動方針: `dusknoir` に進化したら原則すぐ使う。対象 PICK-FIRST（相手の場）:
1. 相手の ex（複数 → 最大 HP の ex）
2. エネ付き相手 `drakloak`（エネ数が多い方）
3. エネ付き相手 `dreepy`（エネ数が多い方）
4. エネ無し相手 `drakloak`
5. エネ無し相手 `dreepy`
6. 相手 `dusclops` / `dusknoir`（複数は G1）

例外（強制発動）: バトル場の `duskull`/`dusclops` がにげられない状況で、
（ベンチの `dragapult-ex` が `canPhantomDive`）**または**（`budew` が場にいる）なら カースドボムを使う。
このときダメカンを乗せる相手は**ランダム**（原文「無作為」）。

### 5.8 `RETREAT`（にげる判断）
- `canPhantomDive(バトル場)==false` かつ `P_phantomReadyBench` → バトル場をにがす
  （にげエネ不足なら `ENERGY_ATTACH` でバトル場に付けてからにがす）。
- `canPhantomDive(バトル場)==false` かつ `!P_phantomReadyBench` かつ
  「バトル場がワザを使えない（`dreepy`/`drakloak` のワザは**使わない扱い**）」 →
  バトル場にエネを付けてにがし、`budew` をバトル場に出す → `むずむずかふん`
  （ベンチに `budew` がいなければ `GOODS` で `budew` を出してから）。
- バトル場が `duskull` → 進化させて `CURSED_BOMB`。
- バトル場が `dusclops`/`dusknoir` → `CURSED_BOMB`。

### 5.9 `REFILL_ACTIVE`（バトル場が空になった時の繰り出し）
バトル場が空になる全般 = 相手ワザできぜつ / 自分のカースドボム自滅 等。**ターンで切り分ける**
（2026-06-10 ユーザー確定: 原文は相手番を「まだワザを使われていない」「相手ワザできぜつ」の 2
サブケースに分けるが、**KO 原因は `StateDto` / `ReplaceActiveAfterKo` prompt に無く判別不能**。
判別材料が engine から来ないため、相手番は「相手ワザできぜつ」の優先度に一本化してよい、と確定）:
- **自分の番**（自分のカースドボム自滅等）: `budew` → （いなければ）エネ付きポケモンを G1。
- **相手の番**（相手ワザできぜつ）: `budew` → `canPhantomDive` な `dragapult-ex` → エネ付き `drakloak`
  → エネ付き `dreepy` → `CURSED_BOMB` が使えるポケモン → `duskull` → `dreepy`
- **GAP-10（解決済 = 設計確定）** 相手番の「まだ相手にワザを使われていない局面」(`budew → dreepy →
  duskull`) は、それを発火させた KO の原因 (相手ワザ / 状態異常 / 自滅) を bot が観測できないため
  忠実に分離できない。忠実に分けるには engine が KO 原因を prompt/state に載せる必要がある。

---

## 6. ターン: 自分の最初の番 — 先攻

目標: `dreepy` を出す。STEP 順次（G2）。

- `T1.1` 特性「ていさつしれい」: **スキップ**（最初の番は `drakloak` 不在で使用不可）。
- `T1.2` `PLACE_BASICS`（たね優先: `dreepy` → 他）。
- `T1.3` `GOODS`（ポケモンを加える）:
  - 「手札のポケモンを出し切った / 進化を全て行った / 手札にポケモンが無い」状態で、ポケモンに
    つながるグッズがあれば**必ず使う**。
  - `dreepy` を2匹以上出せたら `duskull` も出す。
  - `dreepy`,`dreepy`,`duskull` を出してもなおポケモン補充グッズがあれば `drakloak` を加える。
  - 手札に `lillie-s-determination` が無ければ、`ultra-ball`→`meowth-ex` を出し「おくのてキャッチ」で
    `lillie-s-determination` を手札に加えておく（この番は使わず保持）。
- `T1.4` `ENERGY_ATTACH`: `dreepy` に付ける（**超優先**）。付与先は**バトル場**（先攻初手はバトル場 dreepy 想定）。
- `T1.5` その他: `meowth-ex` の「おくのてキャッチ」使用済 かつ 手札に `team-rocket-s-watchtower` が
  あれば出す。
- `T1.6` end_turn（先攻初手はワザ不可）。

---

## 7. ターン: 自分の最初の番 — 後攻

目標: `budew` をバトル場に出して `むずむずかふん`。

- `T2.1` 特性「ていさつしれい」: **スキップ**。
- `T2.2` `PLACE_BASICS`。
- `T2.3` `GOODS`:
  - 手札に `lillie-s-determination` が無ければ `ultra-ball`→`meowth-ex`→「おくのてキャッチ」で
    `lillie-s-determination` を加え、**この番に使う**。
  - `budew` を出せたら `dreepy` 2匹以上 と `duskull` を出す（`dreepy` 優先）。
- `T2.4` `ENERGY_ATTACH`:
  - バトル場が `budew` → ベンチの `dreepy` に付ける（**超優先**）。
  - バトル場が `budew` 以外 かつ ベンチに `budew` あり → バトル場にエネを付けてにがし、
    `budew` をバトル場に出す。
  - バトル場が `budew` 以外 かつ ベンチに `budew` なし → ベンチの `dreepy` に付ける。
- `T2.5` `SUPPORT`: 手札に `lillie-s-determination` があれば使う。
- `T2.6`（サポート後）:
  - 手札のたね → ベンチ空きがあれば出す。
  - 手札の進化ポケモン or `rare-candy` で進化可 → 進化（`EVOLVE_ALL` + §5.2 進化抑制ルール）。
  - 手札のポケモン補充グッズ → 使う（たねは出す / 進化は進化させる）。
  - `meowth-ex`「おくのてキャッチ」使用済 かつ 手札に `team-rocket-s-watchtower` → 出す。
- `T2.7` ワザ: バトル場が `budew` なら `むずむずかふん`。

---

## 8. ターン: 2番目の番 / 3番目以降

### 8.1 2番目の番

目標: `dreepy` を `drakloak` に進化させる。

- `U1` 特性「ていさつしれい」: `drakloak` が場にいれば**全行動に優先して**使用。
  原文の「常に2枚目を手札に加える」はカメラ演出に紐づく物理指定であり、効果上は実質ランダム
  選択と等価のため **G1（ランダム）** で扱う（候補から加える1枚をランダム選択。引く枚数・
  選択方法は `drakloak.yaml` の効果定義を正とする）。
- `U2` `PLACE_BASICS`（この番のたね優先: `dreepy` → `budew` → `duskull`）。
- `U3` `EVOLVE_ALL`（進化はベンチの `dreepy` から。`rare-candy` も使用。§5.2 抑制ルール適用）。
  - 手札に `rare-candy` かつ エネ付き `dreepy` がいる → `ultra-ball` で `dragapult-ex` を加えて進化。
- `U4` `GOODS`。`drakloak` を加えたら進化させて直後に「ていさつしれい」。
- `U5` `ENERGY_ATTACH`: 付与先 `drakloak` → `dreepy`。用意できるなら `dragapult-ex` 優先。
  ベンチへ・`!energyOverflow`。
- `U6` `SUPPORT`:
  - `boss-s-orders` は**使わない**。
  - `lillie-s-determination` を最優先で使用。無ければ `crispin`（アカマツ）。
  - アカマツ: 同タイプ2個が同一ポケに乗らないよう付与。
- `U7`（サポート後）: `T2.6` と同じ4項目（たね展開 / 進化〔抑制ルール込み〕/ 補充グッズ / 監視塔）。
- `U8` ワザ:
  - `P_phantomReadyActive` → `ファントムダイブ`。
  - `P_phantomReadyBench` → バトル場をにがす（→ にがした後に攻撃継続）。
  - `ファントムダイブ` を使う時のベンチダメカン: 相手の **HP が低いポケモン**から優先的に乗せる。
    乗せ量が相手の最大 HP を超える分は、次に HP が低いポケモンへ繰り越す。
  - 使えない場合: バトル場が `dragapult-ex` で `ファントムダイブ` 不可 → `ジェットヘッド`。
    それも不可 → バトル場にエネを付けてにがし `budew` で `むずむずかふん`。

### 8.2 3番目の番以降

目標: `ファントムダイブ` を使うことを最優先。

- `V1` 特性「ていさつしれい」: 全行動に優先（U1 と同じ）。
- `V2` `PLACE_BASICS`（たね優先: `dreepy` → `duskull` → `budew`）。
  - `EVOLVE_ALL`（`rare-candy` 含む、§5.2 抑制ルール）。
    手札 `rare-candy` かつ エネ付き `dreepy` → `ultra-ball` で `dragapult-ex` を加え進化。
  - `team-rocket-s-watchtower` が出ていて、この番 `meowth-ex`「おくのてキャッチ」で
    `boss-s-orders` を加えたい → `jamming-tower` を出して上書き。
- `V3` `GOODS`（`night-stretcher` 含む）。`drakloak` を加えたら進化→「ていさつしれい」。§5.2 抑制ルール。
- `V4` `ENERGY_ATTACH`: `ファントムダイブ` のコストを満たすように付与。`!energyOverflow`。
  - 1匹（`dragapult-ex`/`drakloak`/`dreepy`）が炎+超を両方持ったら、次の `dragapult-ex`→`drakloak`→`dreepy` へ。
  - ベンチの誰か1匹が炎+超を持ち、かつバトル場がにげエネを要する → バトル場へ付与。
- `V5` `SUPPORT`（分岐。上から該当を1つ）:
  - **(a) `P_phantomReadyActive`**:
    - 既定 → `crispin`、付与先 `drakloak`→`dreepy`。
    - ただし `P_bossUse` → `boss-s-orders`（呼ぶ相手 = ダメカン無し・`budew` 以外・HP ≤ 200、複数 G1）が最優先。
    - 手札 ≤ 4枚 かつ `P_bossUse` 不成立 → `lillie-s-determination`。
  - **(b) `P_phantomReadyBench`**:
    - 既定 → `crispin`。バトル場がにげエネを要する→バトル場へ付与 / 不要→`drakloak`→`dreepy` へ。
    - ただし `P_bossUse` → `boss-s-orders`（同上の対象規則）が最優先。
    - 手札 ≤ 4枚 かつ `P_bossUse` 不成立 → `lillie-s-determination`。
  - **(c) `ファントムダイブ` 未充足**:
    - 場に `dragapult-ex` あり かつ（炎 or 超 のどちらか/両方が未装着）→ `crispin`。
      付与先 `dragapult-ex`（バトル場優先）→`drakloak`→`dreepy`、`!energyOverflow`。
    - 場に `dragapult-ex` なし → `lillie-s-determination`。
- `V6`（サポート後・順不同 G1）:
  - 手札のたね → ベンチ空きに出す。
  - 進化可（手札 or `rare-candy`）→ 進化（§5.2 抑制ルール）。
  - ポケモン補充グッズ → 使う。
  - 前の番にポケモンを倒されている かつ 手札に `unfair-stamp` → 使う。
  - 相手のサイド残り ≤ 3 かつ 手札に `special-red-card` → 使う。
  - 相手が `meowth-ex` を場に出していない かつ 自分が `meowth-ex`「おくのてキャッチ」使用済
    → `team-rocket-s-watchtower` を出す。
- `V7` ワザ: `U8` と同一規則（`ファントムダイブ`／にがして攻撃／`ジェットヘッド`／`むずむずかふん`、
  ダメカン配分も同じ）。

---

## 9. GAP / 曖昧点（実装前に要判断・勝手に補完しない）

- **GAP-1（解決済）** `P_phantomUnlikely`＝「判定時点でファントムダイブが技選択肢に無い」で確定
  （2026-06-09、ユーザー判断）。手札先読み（「付ければ使える」）は**しない**。
  実装は `P_phantomReadyActive` の否定で足りる。
- **GAP-2（解決済）** セットアップ時のベンチ初期配置は原文指定なし → **G4（ランダム合法）で確定**
  （2026-06-09、ユーザー判断）。
- **GAP-3（解決済）** `ultra-ball` のトラッシュ2枚は **手札からの純粋ランダム2枚**で確定
  （2026-06-09、ユーザー判断）。原文「右端2枚」以外の温存・最適化指定は原文・公式ルールに無く、
  以前あった「温存ロジック」は私の補完だったため撤回した。
- **GAP-4（解決済）** `ENERGY_ATTACH` の「ベンチからつけて」＝「**ベンチにつけることを優先**」で確定
  （2026-06-09、ユーザー判断）。ハード制約ではなく preference（ベンチに該当候補が無ければバトル場へ、
  ステップ明示・にげエネ確保が優先）。
- **GAP-5（解決済）** 「ていさつしれい」の「2枚目を加える」は実質ランダムと等価のため **G1
  （ランダム）で確定**（2026-06-09、ユーザー判断）。引く枚数・選択方法は `drakloak.yaml` を正とする。
- **GAP-6** カースドボムの対象に「相手のドロンチ/ドラメシヤ」が出る＝**ミラー前提**。
  非ミラー相手では候補0になり PICK-FIRST が後段（ex / `dusclops` 等）へ落ちる。`play --deck` の
  ミラー運用では問題なし。
- **GAP-7** 「動画用スリープ」「一方的に勝ちそう」の判定は G5 で実装対象外（演出）。
- **GAP-9（解決済、2026-06-09 engine 修正）** 旧 `rare-candy` POL 実装は stub
  （`run_evolve_rare_candy` が `hand.first()` を Stage2 とみなし誤進化、召喚酔いも未除外でマッチ中断）
  だったが、engine 側で修正済み（commit `3bded44`：`evolution::rare_candy_chain_matches` で
  stage2→stage1→basic の名前チェーンを registry 検証 + `ZoneFilter.rare_candy_base` で
  「進化ラインに合う 2 進化が手札にあるたね」だけを `legal_actions` の pool に残す。
  進化禁止「自分の最初の番」は commit `11ac5c5` で修正）。**bot は §5.4 RARE_CANDY を実装済**
  （`find_rare_candy`：炎+超 dreepy→dragapult-ex〔ドロンチ止め適用〕/ 既定 duskull→dusknoir、
  対象たねは `pending_rare_candy` 経由で offered pool から §5.4 優先選択）。12 シードの
  takeuchi 同士対戦で中断なしを確認（`dragapult_takeuchi_completes_across_seeds_with_rare_candy`）。
- **GAP-8（解決済）** 原文の優先度リストの「最優先」二重発火・ランク係り先不明は、
  **同ランク衝突を G3b（候補群からランダム）**で解決すると確定（2026-06-09、ユーザー判断「C」）。
  `poke-pad`/`ultra-ball` を「最優先 tier（衝突→ランダム）→ 次 tier（PICK-FIRST）→ 既定」の
  3層モデルに再構成済み。原文と差異が出たら原文照合で修正する。

---

## 10. カード事実テーブル（実 POL データ由来 / 実装で参照）

`crates/engine-core/data/cards/{slug}.yaml`（効果）+ master（HP/にげ/弱点）から抽出した、判断に必要な
事実。**ワザのコスト・威力・効果・特性・にげコストは StateDto に含まれない**（StateDto から取れるのは
slug / stage / hp_max / damage / 装着エネの slug / status / 特性使用済みフラグ）。

**実装方針**: 内蔵 BOT は in-process なので、本テーブルをハードコードせず **`CardRegistry` から
ライブで読む**。`CardDef` が `attacks` / `abilities` / `hp` / `weakness` / `retreat_cost` を保持して
おり、StateDto の slug を `registry.find_by_slug` で `CardDef` に解決すれば、コスト・威力・にげコスト
まで全て取得できる。本テーブルは**人間用の参照ドキュメント**として残す（実装の真値は registry）。

### ポケモンのワザ / 特性

| slug | 進化 | ワザ / 特性 | コスト | 効果 |
|---|---|---|---|---|
| `dragapult-ex` | 2進化 | ジェットヘッド | 無 | 70 |
| `dragapult-ex` | 2進化 | **ファントムダイブ** | **炎+超** | **200** + 相手ベンチに**ダメカン6個(=60)分配** |
| `drakloak` | 1進化 | 特性 **ていさつしれい** | — | activated・1/turn。山上2枚→1枚手札・残り1枚を山下（選択=G1ランダム） |
| `drakloak` | 1進化 | リューズヘッド | 炎+超 | 70（**戦略は drakloak のワザを使わない**） |
| `dreepy` | たね | ちょっとうらむ / かみつく | 超 / 炎+超 | 10 / 40（**戦略は dreepy のワザを使わない**） |
| `duskull` | たね | むかえにいく / つぶやく | 超 / 超超 | trash の「ヨマワル」をベンチへ / 30（戦略では未使用） |
| `dusclops` | 1進化 | 特性 **カースドボム** | — | activated・1/turn。対象1匹に**ダメカン5個(=50)**→**自分を気絶** |
| `dusclops` | 1進化 | おにび | 超超 | 50 |
| `dusknoir` | 2進化 | 特性 **カースドボム** | — | activated・1/turn。対象1匹に**ダメカン13個(=130)**→**自分を気絶** |
| `dusknoir` | 2進化 | かげしばり | 超超無 | 150 + 相手バトル場 にげられない（次相手番終了まで） |
| `budew` | たね | **むずむずかふん** | **無し(0)** | 10 + 相手は次の番グッズ使用不可（→ **スボミーはエネ0で常に攻撃可**） |
| `meowth-ex` | たね/ex | 特性 **おくのてキャッチ** | — | **triggered（ベンチ出し時に自動発火）**・「おくのて」名グループ1/turn。山からサポート1枚をサーチ→手札（公開）→シャッフル |
| `meowth-ex` | たね/ex | しっぽをまく | 無無無 | 60 + 自身を手札に戻す |
| `fezandipiti-ex` | たね/ex | 特性 さかてにとる | — | activated・1/turn。前の相手番に自ポケが KO されていれば3ドロー |
| `fezandipiti-ex` | たね/ex | クルーエルアロー | 無無無 | 相手1匹にダメカン10個(=100、弱抵抗無視) |

### 実装上の含意（戦略との差分・注意）

- **`ファントムダイブ` の `cost_met`** = 炎エネ1 + 超エネ1（ちょうど2個で要件充足）。`!energyOverflow`
  ルール（炎炎/超超回避）はこの「炎1超1」の自然な帰結。
- **`むずむずかふん` は 0 エネ** → `budew` はバトル場にいれば常に使える。原文「エネつけてにがしてスボミー」
  の"つけて"は**にげる側のにげエネ**。
- **`おくのてキャッチ` は triggered**: 「使う」＝ `meowth-ex` をベンチに出す → 発火 → サーチ prompt で
  対象サポート（§5.6 の優先度）を選ぶ。能動 `UseAbility` ではない。
- **`カースドボム` は使用後に自分が気絶する**（自滅）＝サイドを相手に渡す。原文の運用（ヨノワール進化後
  即使用・対象は相手 ex 優先）をそのまま実装するが、自滅コストは戦略の織り込み済みとして扱う。
  威力は サマヨール50 / ヨノワール130 で異なる（§5.7 の対象規則は共通）。
- **`fezandipiti-ex`（キチキギスex）** は S2 のバトル場候補に出てくるが、原文は能動運用を
  指定していない（さかてにとる＝KO 返しドローは自動条件）。ベンチ出しの優先には乗らない（ex 抑制）。
- **にげコスト不明**: 「にげエネが必要か」を判定する `RETREAT`/`ENERGY_ATTACH`(V4/V5) のために、
  各 slug のにげコストを別途採取してテーブル化する必要がある（master 参照、未採取）。
