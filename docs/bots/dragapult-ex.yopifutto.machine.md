# Dragapult ex BOT（よぴふっと博士版）— 機械可読戦略仕様（正規化版）

> **出典**: [`dragapult-ex.yopifutto.md`](dragapult-ex.yopifutto.md)（ユーザー提供の人間向け原文、よぴふっと博士版、2026-06-09 受領）。
> 本ファイルは原文（ノード ID 付き決定木）から**演出・口上・物理動作を除去**し、**意思決定のみを
> 決定的ステップ機械**として正規化したもの。実装は `crates/bots/src/dragapult_yopifutto.rs`
> （`DragapultYopifuttoBot`、`BotPolicy` 実装）にあり、この正規化版を直接の参照とする。
> bot 共通の枠組み（trait / レジストリ）は `crates/bots/src/lib.rs`、`ptcg-cli` からは
> `--bot dragapult-yopifutto` で選択する。
> 原文と本ファイルが食い違ったら**原文を正**とし、本ファイルを再正規化する。
> 人間の指示はバグ（対象未指定・条件漏れ・矛盾・ノード ID の重複/飛び）を含む前提。補完できない
> ものは §9 GAP に列挙し、勝手に意味を足さない。実行時の既定挙動は §2 のグローバル方針に従う。
>
> **クチート竹内版との関係**: デッキ（`decks/dragapult-ex.yaml`）・slug 表（§1）・グローバル方針
> （§2）・カード事実（§10）は両ペルソナ共通。差分はターン手順（§4〜§8）と GAP（§9）。共通部は
> [`dragapult-ex.takeuchi.machine.md`](dragapult-ex.takeuchi.machine.md) と同一内容を再掲する。

---

## 1. 用語 ↔ slug 対応（`decks/dragapult-ex.yaml`）

竹内版 §1 と同一（同じデッキ）。

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

関連特性・ワザ:
- `ていさつしれい`（`drakloak`）/ `おくのてキャッチ`（`meowth-ex`）/ `カースドボム`（`dusclops`/`dusknoir`）
- `さかてにとる`（`fezandipiti-ex`）/ `ファントムダイブ`（`dragapult-ex`）/ `むずむずかふん`（`budew`）

---

## 2. グローバル方針（全ステップ共通・最優先）

竹内版 §2 と同一（ペルソナ非依存）。

- **G1（対象未指定→ランダム）**: 行動が対象を取るのに戦略が一意に決めていない場合、条件に合致する
  合法候補からランダムに1つ選ぶ。候補0なら行動自体をスキップ。
- **G2（ステップ順次）**: 各ターンは STEP（=原文ノード ID）を番号順に評価。各 STEP は「実行可能なら
  実行、不可能ならスキップして次へ」。原文の「○○へ」は次ノードへの遷移＝順次評価で表現する。
- **G3（PICK-FIRST 優先度）**: 「A＞B＞C」は上から評価しガード条件を満たす最初の候補を選ぶ。
- **G3b（同ランク衝突→ランダム）**: 同ランク候補がガードを同時に満たすなら G1 でランダム選択。
- **G4（未規定→ランダム合法手）**: どの RULE にも当てはまらず STEP が「やること」を指定しない場合は
  合法手からランダム（最終手段。可能なら end_turn を含め番を進める方向）。
- **G5（演出除去）**: 口角・ため息・カメラピース・「勝ちました！」・無邪気に喜ぶ・物理位置指定
  （「一番右」「左側」等）は実装しない。位置指定が対象選択に化けている箇所は G1 でランダム化。
- **G6（ジャンケン）**: 勝ち→**先攻**を選択（※竹内版と逆。原文 A-a「勝ち＝先攻をとる」）/
  負け→相手が選ばなかったほうをとる。
- **G7（REPEAT 明示）**: 「すべて出す」「すべて進化させる」「○○ノードへ（自己ループ）」は対象が
  尽きるか合法でなくなるまで STEP 内で反復（`REPEAT` と明記）。原文の「X-a-n → X-a-n へ」という
  自己参照は「その種類のリソースが尽きるまで繰り返す」ループとして解釈する。

---

## 3. 述語（predicate）定義

- `cost_met(p, attack)` … `p` の装着エネが `attack` のコストを満たす（コストは当該 YAML を正）。
- `canPhantomDive(p)` ≡ `p.slug == dragapult-ex && cost_met(p, ファントムダイブ)`（＝炎1+超1）。
- `P_phantomReadyActive` ≡ バトル場が dragapult-ex かつ `canPhantomDive(バトル場)`。
- `P_phantomReadyField` ≡ 場（バトル場/ベンチ）に `canPhantomDive` な dragapult-ex がいて、かつ
  この番にバトル場へ出して攻撃できる見込みがある（原文 F-a-13「この番ファントムダイブを使用できる？」）。
- `P_itchyLastTurn` ≡ **前の相手の番に**バトル場が `むずむずかふん`（budew）を受けた
  （＝この番グッズ使用不可）。原文の「前の番にむずむずかふんを使われた？」「グッズはないものとして扱う」。
  **GAP-Y1（解決済 — engine 修正、2026-06-09）**: むずむずかふんのグッズロックは engine で
  `OpponentCannotPlayItem` timed effect として実装済だが、当初 `legal_actions` に反映されず（実行時
  `EffectBlocked` のみ）、StateDto にも timed_effects が無いため bot から不可視だった。**engine の
  `push_legal_play_card` を修正し、グッズロック中はアイテムを `legal_actions` から除外**（実行時
  ブロックと整合）。これにより原文「グッズはないものとして扱う」は**合法手から自動成立** — bot 側に
  `P_itchyLastTurn` 検出コードは不要（グッズ系 GOODS は合法手に出ないので素通り）。Supporter は
  ロック対象外なので従来どおり使える。
- `enemyExLeHP(h)` ≡ 相手の場に残り HP ≤ h のポケモンex がいる。
- `koByCursed(target, n)` ≡ `target` の残り HP が、カースドボムの n 個（dusclops=5/dusknoir=13）の
  ダメカンで 0 以下になる。
- `cursedReducesToLe(target, h, n)` ≡ カースドボム n 個で `target` の残り HP が h 以下になる。
- `evolvableDreepyOnField` ≡ 場に「この番進化できる（召喚酔いでない）`dreepy`」がいる。
- `notSupportedYet` / `notAttachedYet` ≡ この番まだサポート未使用 / まだ手札からエネ未装着。
- エネルギー種別優先: 原文は「炎＞超」と「超＞炎」を文脈で使い分ける。各 STEP の明示に従う
  （§5.5 ENERGY_ATTACH 参照）。`!energyOverflow`（同種2個＝炎炎/超超を作らない）を常に満たす。

---

## 4. セットアップ（A. 対戦準備）

- `S1` ジャンケン（A-a）: §G6（勝ち→先攻 / 負け→相手非選択側）。
- `S2` 手札評価（A-b）: 7枚引いてたねがいなければエンジン既定マリガン（引き直し回数分は
  相手にドロー機会＝エンジン処理）。原文 A-b-2 の「キープ基準」（ドラメシヤ/スボミー/ヨマワルが
  いて、かつ エネ or なかよしポフィン or ポケパッド or リーリエの決心がある）は**マリガン判断では
  なく初手キープの自己申告**であり、現行エンジンにマリガン選択の余地はない → **実装対象外**（GAP-Y2）。
- `S3` バトル場の最初の1体（PICK-FIRST、出せるたねから）:
  - **先攻（A-c-1）**: `duskull`(ヨマワル) → `dreepy`(ドラメシヤ) → `budew`(スボミー)
  - **後攻（A-c-2）**: `budew`(スボミー) → `dreepy`(ドラメシヤ) → `duskull`(ヨマワル)
  - **たね不在で ex のみ（A-F-a-3）**: `fezandipiti-ex`(キチキギスex) → `meowth-ex`(ニャースex)
  - **NOTE**: 竹内版 S2 と優先順が異なる（よぴふっと版は先攻ヨマワル最優先・後攻スボミー最優先）。
  - **GAP-Y3**: 原文 A-b の分岐は「A-c-1（エネ等あり）/ A-c-2（リーリエもない）」を分けるが、
    どちらも結局バトル場選択の優先順が違うだけ（c-1=ヨマワル/スボミー優先, c-2=スボミー優先）。
    現状エンジンの `ChooseInitialActive` は手札全体を見て1体選ぶだけなので、c-1/c-2 の差は
    「先攻=ヨマワル優先 / 後攻=スボミー優先」に集約して実装する（A-c-2 単独条件は近似）。
- `S4` ベンチ初期配置（`PlaceInitialBench`）: **何も置かない（0 枚）**。原文 A の対戦準備手順は
  「バトル場に1体出す（A-c-1/A-c-2/A-F-a-3）→ サイド6枚 → B.1回目の番へ」で**完結しており、
  ベンチ初期配置の動作自体が存在しない**。よって忠実にはセットアップでベンチには置かない。
  （旧版は「原文指定なし → G4ランダム」と記したが、原文に動作が無い以上ランダムで置くのは挙動の
  発明＝誤り。2026-06-10 ユーザー確定で訂正。「原文が定めない選択はランダム」の原則は、ペルソナが
  直面する選択に対するもので、原文に存在しない動作を足す根拠にはならない）。

---

## 5. 共通サブルーチン

各ターン STEP から呼ぶ。すべて「合法な範囲で・対象未指定は G1」。竹内版と共通の挙動が多いが、
よぴふっと版固有の優先順を明記する。

### 5.1 `PLACE_BASICS`（たねをベンチに出す）— REPEAT
原文の各番に共通する「ドラメシヤが N 匹以下・ヨマワルが1匹以下・スボミーが1匹以下の範囲内で
すべてのたねを出す」。上限を満たす範囲で手札のたねを出す。出す順は PICK-FIRST `dreepy` → 他
（番ごとの上限: §6〜§8 の各 STEP に明記）。**ex（`meowth-ex`/`fezandipiti-ex`）は条件付き**（各 STEP が
明示した時のみ）。

### 5.2 `EVOLVE_ALL`（通常進化）— REPEAT
原文 C-a-6 / F-a-5 に忠実。**使う進化カードは「サマヨール(dusclops) / ヨノワール(dusknoir) /
ドロンチ(drakloak)」のみ**で、**`dragapult-ex` は通常進化に含めない**（F-a-5「持っているサマヨール、
ヨノワール、ドロンチのみ場のポケモンたちをすべて進化させ」）。
- 通常進化の優先（PICK-FIRST）: `drakloak`（dreepy→drakloak）→ `dusclops`（duskull→dusclops）→
  `dusknoir`（dusclops→dusknoir）。エネ付き `dreepy`/`drakloak` を優先（F-a-9「エネルギーがついている
  ドラメシヤを優先して進化」）。
- `rare-candy`: 手札にふしぎなアメ + ヨノワールがあれば `duskull`→`dusknoir` 直行（サマヨール飛ばし、
  C-a-6 / F-a-5）。✅ engine の rare-candy 修正（GAP-9 解決）に伴い実装済（`find_rare_candy`、通常進化より優先）。
- **`drakloak`→`dragapult-ex` の進化は通常進化に含めない**。これは F-a-11/F-a-12 の
  **ファントムダイブ用アタッカー準備パス専用**（「場に `dragapult-ex` がいない」かつ「炎+超エネが
  ついた `drakloak` がいる」とき、その `drakloak` を **1匹** `dragapult-ex` に進化、または手札/
  `ultra-ball` の `dragapult-ex` で進化）。実装では別スライス（攻撃準備）で扱う。
- **GAP-Y4（解決済 — 原文から導出、2026-06-09）**: 当初「竹内版のドロンチ止めを当てる」と誤って
  既定したが、よぴふっと原文は通常進化に `dragapult-ex` を含めず、drakloak→dragapult-ex を
  F-a-11/F-a-12 のアタッカー準備でのみ行う構造になっている。他ペルソナとの比較ではなく、この原文
  構造をそのまま実装する（通常進化 = `EVOLVE_PRIORITY_BLANKET` = drakloak/dusclops/dusknoir）。

### 5.3 `GOODS`（ポケモンを加える/その他グッズ）
よぴふっと版のグッズ全体優先度（原文 B/C/F の各番先頭で繰り返される順）:
`buddy-buddy-poffin`(なかよしポフィン) → `poke-pad`(ポケパッド) → `ultra-ball`(ハイパーボール)。
（竹内版は poke-pad → poffin → ultra-ball → night-stretcher。**よぴふっと版は poffin が先頭**）。
`P_itchyLastTurn` のときは**この番グッズ全スキップ**（原文「グッズを使用する箇所はすべてナシ」）。

- **`buddy-buddy-poffin`**（たね限定。番ごとに目標盤面が違う）:
  - B（1番）: 場の `dreepy` が2匹以上になるように、加えて `duskull`1・`budew`1。不足時は dreepy 優先。
  - C（2番）: 場の `dreepy`（進化込み）が3匹・`duskull`1・`budew`1 に近づける。余れば budew＞duskull。
  - F（3番〜）: 場の dreepy 系が3匹・`duskull`1・`budew`1 に近づける。余れば budew1・duskull2 以上。
- **`poke-pad`**（PICK-FIRST、原文 B-a-2 / C-a-4 / F-a-4 共通骨子）:
  1. 場に dreepy 系 ≥2 かつ場に budew がいる → （手札 drakloak ≥2 かつ場にヨマワル）? `dusclops`(進化用) :
     （手札 drakloak ≥2 かつヨマワル不在）? `duskull` : `drakloak`
  2. 場に dreepy 系 ≥2（budew 不在）→ `budew`
  3. 既定 → 場の dreepy が2匹になるよう `dreepy`
  - 加えたポケモンは直後に出す/進化させる（自己ループ ...-へ）。
- **`ultra-ball`**（原文 B-a-3 / C-a-5 / F-a-10 等）:
  - トラッシュ対象（PICK-FIRST、原文の明示順。`drakloak` は決してトラッシュしない）:
    `night-stretcher` → スタジアム → 2枚以上被るポケモン（drakloak 除く）1枚 → `ultra-ball` →
    `rare-candy` → その他グッズ（G1ランダム）→ `boss-s-orders` → `crispin` → `lillie-s-determination` → エネ
    - C（2番）の順は: スタジアム → 被りポケ1 → ultra-ball → rare-candy → その他グッズ → boss → サポート → エネ
      （night-stretcher を最上位に置かない番もある＝原文差。各番の明示順を正とする）。
  - サーチ対象: 場に dreepy がいなければ（B-a-3 下段）`meowth-ex`（→おくのてキャッチ）or `dreepy`。
    場に dreepy 系 ≥2 + budew + 手札 drakloak ≥2 + ヨマワル → `dusclops`。既定 → `drakloak`。
    各番の明示分岐（§6〜§8）を正とする。

### 5.4 `ENERGY_ATTACH`（手張り）— `notAttachedYet` のときのみ
番ごとに付与先・タイプ優先が異なる（原文 B-a-4/B-a-7/C-a-7/F-a-6）:
- **B 先攻（B-a-4）**: バトル場が dreepy → バトル場 dreepy に（炎＞超）。else 場に dreepy → その
  dreepy に（炎＞超）。else バトル場に（炎＞超）。
- **B 後攻（B-a-7）**: バトル場が budew → ベンチ dreepy に（炎＞超）。else ベンチに budew → バトル場に
  （炎＞超）。else 場（左側）の dreepy に（炎＞超、「左側」は G5 で無視し G1）。
- **C（C-a-7）**: バトル場 budew → ベンチ `drakloak`＞`dreepy` が炎・超付きに近づくよう（炎＞超）。
  else ベンチに budew → バトル場に（超＞炎）。else 場の dreepy に（超＞炎）。
- **F（F-a-6）**: バトル場 budew → ベンチ `drakloak`＞`dreepy` を炎・超付きに近づける（炎＞超）。
  else バトル場 dragapult-ex → ベンチ `drakloak`＞`dreepy` を炎・超付きに近づける（タイプ無指定→G1で
  `!energyOverflow`）。else 場の `drakloak`/`dreepy` を炎・超付きに近づけ、超過分は付けない。
- 不変条件: `!energyOverflow`（炎炎/超超を作らない）。「炎・超付きに近づける」＝1匹が炎1超1を
  持つ状態を目標に、不足タイプを補う向きで付ける。

### 5.5 `SUPPORT`（サポート使用）— `notSupportedYet` のとき、各番の分岐に従う
- **アカマツ（`crispin`）**: 原文「場のポケモンについているほうのエネを手札に加え、ついていないほうを
  効果でつける」。アカマツは2枚を山札から手札に加える効果だが、原文運用は「不足タイプを供給する」
  目的。実装は当該カード効果（`crispin.yaml`）を正とし、付与先 PICK-FIRST は「エネ1個の dreepy 系
  ＞ エネ無し dreepy 系 ＞ バトル場」（C-a-8）/「エネ付き dreepy 系 ＞ エネ無し dreepy 系」（F-a-7、
  場のエネ総数 ≥3 なら使わない）。
- **リーリエの決心（`lillie-s-determination`）**: 各番の分岐で「使う/使わない」が指定（B-a-8 / C-a-9 /
  F-a-8）。使ったら手札補充後その番の盤面整備ノードへ戻る（自己ループ）。
- **ボスの指令（`boss-s-orders`）**: §7（F-a-13 / D 群）で詳述。`P_itchyLastTurn` でも**サポートは使える**
  （グッズ不可なだけ）。

### 5.6 `RECON`（特性ていさつしれい）— 全行動に優先（C-a-10 / F-a-9）
場に「ていさつしれい」を使える `drakloak` がいれば、使えなくなるまで使用（REPEAT, G7）。
加える1枚の優先度（原文 C-a-10 / F-a-9。場のポケモン数で分岐）:
- **場のポケモン ≤2**: たねポケモン → `buddy-buddy-poffin` → `poke-pad` → `lillie-s-determination`
  →（進化できる dreepy がいる場合に限り）`drakloak` →（同条件で）`dragapult-ex` → `unfair-stamp`
  → `special-red-card` → その他サポート。
- **場のポケモン >2**: `crispin` → `ultra-ball` → `lillie-s-determination` → `poke-pad` → `drakloak`
  → `dragapult-ex` → `psychic-energy`(超) → `fire-energy`(炎) → その他（優先つかず→G1）。
  - C（2番）の >2 順は: `lillie-s-determination` → `crispin` → `poke-pad` → `drakloak` → `dragapult-ex`
    → 超 → 炎 → その他（C-a-10）。F（3番〜）の順（F-a-9）とは差がある → 各番の明示順を正とする。
- 「2枚目を加える」等の物理指定は GAP-5（竹内版）と同様 G1。引く枚数・選択は `drakloak.yaml` を正。

### 5.7 `CURSED_BOMB`（`dusclops`/`dusknoir` カースドボム）
- **グローバル制約（原文冒頭・D 冒頭で2回明示）**: 相手のサイドが1枚以下のときは**使用しない**
  （相手サイド1枚なら「カースドボムの判定はいいえへ」）。
- 威力: `dusclops`=ダメカン5個(50) / `dusknoir`=ダメカン13個(130)。使用後**自分が気絶**（自滅）。
- 対象は各使用箇所が明示（F-a-12 / D-a-1 / D-a-2 / D-a-4 群）。原則「相手 ex（複数なら残り HP 最大）」
  または「カースドボムで HP がしきい値（200/60）以下に落ちる相手」。

### 5.8 `RETREAT_TO_BUDEW`（むずむずかふん運用）
バトル場が budew でなく、ベンチ/場に budew を出せる/いる場合（B-a-9 / C-a-11）:
バトル場をにがし（エネ2タイプ以上付くならトラッシュは手札にあるタイプを選択）、budew をバトル場へ
出して `むずむずかふん`。

### 5.9 `REFILL_ACTIVE`（E. バトルポケモンがきぜつした時の繰り出し）
PICK-FIRST: ベンチに budew → `budew`。else エネ付き `drakloak` → エネ付き `dreepy` → `dreepy`
→ `drakloak` → その他。繰り出し後は F（3番以降）の手順へ。

### 5.10 `KO_RECOVERY`（F-a-10 さかてにとる回収）
前の番に自分のポケモンがきぜつしていれば（`fezandipiti-ex` の「さかてにとる」=3ドロー）:
- 場に未使用 `fezandipiti-ex` → 使う。
- else 手札に `fezandipiti-ex` → ベンチ空きがあれば出して使う。
- else `ultra-ball` あり → サーチ（トラッシュ優先: night-stretcher/ultra-ball/poffin/budew/duskull/
  dusclops/dusknoir/サポート）で `fezandipiti-ex` を加え出して使う。
- else 相手手札 ≥4 かつ手札に `unfair-stamp` → 使う。

---

## 6. ターン: 自分の最初の番（B. 1回目の番）

STEP 順次（G2）。先攻/後攻で分岐（B-a）。

### 6.1 先攻
- `B1` ドロー（エンジン処理）。
- `B2` `PLACE_BASICS`（上限: dreepy ≤3・duskull ≤1・budew ≤1）。
- `B3`（B-a-1）`buddy-buddy-poffin`: 場 dreepy ≥2 なら dreepy ≥2・duskull1・budew1 を目標に。
  未満なら dreepy を2匹出す。REPEAT（ループ）。
- `B4`（B-a-2）`poke-pad`: §5.3 poke-pad 規則。REPEAT。
- `B5`（B-a-3）`ultra-ball`: 場に dreepy がいる → 手札に drakloak あり? 使わない : drakloak をサーチ。
  場に dreepy 不在 → 山に meowth-ex あり? meowth-ex を出し「おくのてキャッチ」で lillie 保持 :
  dreepy をサーチ。トラッシュは §5.3 の順。
- `B6`（B-a-4）`ENERGY_ATTACH`（§5.4 B 先攻）。
- `B7`（B-a-10）end_turn（先攻初手はワザ不可）。

### 6.2 後攻
- `B1'` ドロー。
- `B2'`（B-a-4）`PLACE_BASICS`（上限同上）。
- `B3'`（B-a-5）`buddy-buddy-poffin`: 場 dreepy ≥2 なら dreepy ≥2・duskull1・budew1。未満なら
  dreepy ≥2 を目標、余れば budew＞duskull。REPEAT。
- `B4'`（B-a-6）`poke-pad`: §5.3。REPEAT。
- `B5'`（B-a-7）`ENERGY_ATTACH`（§5.4 B 後攻）。
- `B6'`（B-a-8）`lillie-s-determination`: この番ポケパッドで drakloak を加えていれば使わない、else 使用
  → B-a-4（=盤面整備）へ戻る（REPEAT）。
- `B7'`（B-a-9）バトル場が budew → `むずむずかふん`。else 場に budew いてバトル場にエネ付き →
  `RETREAT_TO_BUDEW` → `むずむずかふん`。else end_turn。
- `B8'`（B-a-10）end_turn。

---

## 7. ターン: 2回目の番（C. 2回目の番）

冒頭注（C 冒頭の ※）: この番すでにサポート使用済ならサポート分岐は「いいえ」、すでに手張り済なら
エネ分岐は「いいえ」を通る（= `notSupportedYet`/`notAttachedYet` のガード）。

- `C0` ドロー。
- `C1`（C-a-1）`P_itchyLastTurn` 判定 → 真ならこの番グッズ全スキップ。
- `C2`（C-a-2）`PLACE_BASICS`（dreepy ≤3・duskull ≤1・budew ≤1）。REPEAT で C-a-3 へ。
- `C3`（C-a-3）`buddy-buddy-poffin`: 場 dreepy ≥3 なら dreepy3・duskull1・budew1。未満なら dreepy3 に
  近づけ余れば budew＞duskull。REPEAT。
- `C4`（C-a-4）`poke-pad`: §5.3（手札 drakloak ≥2 + ヨマワル → dusclops 進化 / ヨマワル不在 → duskull）。REPEAT。
- `C5`（C-a-5）`ultra-ball`: 場 dreepy 系 ≥2 + budew + 手札 drakloak ≥2 → ヨマワルいれば dusclops を
  サーチ進化、else drakloak。drakloak は絶対トラッシュしない。トラッシュ順は §5.3（C 番の順）。
- `C6`（C-a-6）`EVOLVE_ALL`（手札に rare-candy+dusknoir → dusclops 飛ばし。else 進化可能をすべて）。
- `C7`（C-a-7）`ENERGY_ATTACH`（§5.4 C）。
- `C8`（C-a-8）`crispin`（アカマツ）: §5.5。付与先 = エネ1個の dreepy 系 ＞ エネ無し dreepy 系 ＞ バトル場。
- `C9`（C-a-9）`lillie-s-determination`: あれば使用 → C-a-2 へ戻る（REPEAT）。
- `C10`（C-a-10）`RECON`（ていさつしれい、§5.6。場ポケ数で加える優先分岐）→ C-a-2 へ戻る。
  RECON 不可で「進化できる dreepy がいる」→ 手札 drakloak あり? 進化（G1）: poke-pad で drakloak
  サーチ進化 : C-a-11 へ。
- `C11`（C-a-11）バトル場 budew → `むずむずかふん`。else budew を出せる/ベンチにいる →
  `RETREAT_TO_BUDEW` → `むずむずかふん`。else C-a-12。
- `C12`（C-a-12）end_turn。

---

## 8. ターン: 3回目以降（F. 3回目以降の番）+ ボス判定（D）+ きぜつ（E）

冒頭注（F 冒頭の ※）: C と同じ（サポート/手張り済なら「いいえ」）。

- `F0` ドロー。
- `F-ENTRY` 自分のサイド ≤4 かつ `P_phantomReadyField` → **D（ボス判定）へ**。else `F-a-1` へ。

### 8.1 メインライン（F-a-1 〜 F-a-15）
- `F1`（F-a-1）`P_itchyLastTurn` → グッズ全スキップ。
- `F2`（F-a-2）`PLACE_BASICS`（dreepy 系 ≤3・duskull ≤1・budew ≤1）。REPEAT。
- `F3`（F-a-3）`buddy-buddy-poffin`: 場 dreepy 系 ≥3 なら3・duskull1・budew1。未満なら3に近づけ
  余れば budew1・duskull2 以上。REPEAT。
- `F4`（F-a-4）`poke-pad`: §5.3（dreepy 系 ≥2 + budew + 場 drakloak ≥2 → ヨマワルいれば dusclops 進化 /
  ヨマワル不在 → duskull / else drakloak 進化）。REPEAT。
- `F5`（F-a-5）`EVOLVE_ALL`（rare-candy+dusknoir → dusclops 飛ばし。else 持っている dusclops/dusknoir/
  drakloak で全進化）。
- `F6`（F-a-6）`ENERGY_ATTACH`（§5.4 F）。
- `F7`（F-a-7）`crispin`: 場のエネ総数 ≥3 なら使わない。else 使用、付与先 = エネ付き dreepy 系 ＞ エネ無し
  dreepy 系。
- `F8`（F-a-8）`lillie-s-determination`: すでに `canPhantomDive` な dragapult-ex + drakloak ≥2 が場にいる
  → 使わない。else 使用 → F-a-2 へ戻る（REPEAT）。
- `F9`（F-a-9）`RECON`（§5.6 F 番の優先分岐）→ F-a-2 へ戻る。RECON 不可で「進化できる dreepy」→
  手札 drakloak あり? エネ付き dreepy 優先で進化 : poke-pad で drakloak サーチ進化 : F-a-10。
- `F10`（F-a-10）`KO_RECOVERY`（§5.10。前番きぜつ時）。きぜつしていない → F-a-11。
- `F11`（F-a-11）**エネ準備/進化の中核**（原文最大の分岐）:
  - 場に dragapult-ex いる & にげられる/バトル場が dragapult-ex:
    - 炎+超 両方付き → F-a-12。
    - どちらか1つ付き → 手札に不足タイプエネ? `notAttachedYet` なら手張り→F-a-6 :
      手札 crispin? アカマツで補給(F-a-5-①)→F-a-6 : 手札 meowth-ex? ベンチ出し→おくのてキャッチで
      crispin 加え→F-a-5-① : lillie あれば使用→F-a-6。
    - エネ無し（ベンチ4匹埋まり）→ ベンチの「この番より前から出ている」ヨマワル/サマヨールを
      サマヨール/ヨノワールへ進化（手札 or poke-pad）→ F-a-6。ベンチ展開（poffin/poke-pad で
      dreepy3・ヨマワル1）→ lillie 使用→F-a-5。（原文 F-a-11 上段の深い入れ子。§GAP-Y5）
  - 場に dragapult-ex いない:
    - 炎+超 付き drakloak いる → 手札 dragapult-ex? 進化→F-a-12 : 手札 ultra-ball? dragapult-ex を
      サーチ進化（トラッシュ順 グッズ＞ポケモン＞サポート＞エネ）→F-a-12 : F-a-12。
    - エネ1個 drakloak いる → 手札エネあり? 違うタイプを「エネ付き drakloak＞dreepy」/同タイプを
      「エネ無し drakloak＞dreepy」に付け→F-a-5② : 手札 crispin? アカマツで不足タイプ補給→F-a-6 :
      手札 meowth-ex? ベンチ出し→crispin 加え→F-a-6 : F-a-6。
    - else F-a-12。
- `F12`（F-a-12）`CURSED_BOMB` 事前付与: バトル場に dragapult-ex を出せる/いて、場に使える dusknoir +
  相手に ex → カースドボム（相手 ex、複数なら残り HP 最大）にダメカン13個。→ F-a-13。
- `F13`（F-a-13）この番 `ファントムダイブ` 使用可？:
  - 可 & 手札 boss & `notSupportedYet` → 以下のときのみ `boss-s-orders`:
    ①相手に dragapult-ex 不在 & 相手ベンチに残り HP ≤200 の `fezandipiti-ex`/`meowth-ex` → 呼ぶ
    （fezandipiti-ex＞meowth-ex）。②相手に dragapult-ex 不在 & 相手 drakloak が1匹のみ → drakloak を呼ぶ。
    → F-a-14。
  - 不可 → 相手サイド ≤3 & この番 unfair-stamp 未使用なら `special-red-card` → F-a-15。
- `F14`（F-a-14）`ファントムダイブ`（バトル場に dragapult-ex を出して）使用。「出して」= **バトル場を
  にがしてベンチの撃てる (炎+超) dragapult-ex を繰り出す**。**バトル場が budew でも同じ**: 撃てる
  dragapult-ex がベンチにいれば、budew をにがして繰り出す（むずむずかふん 10 ではなくファントムダイブ
  200。F-a-15 チップは「撃てる dragapult-ex が無い」=ファントムダイブ不可のときの専用フォールバック）。
  ※旧実装は `find_retreat` が budew を無条件にげ不要とし、撃てる dragapult-ex がいても繰り出さず
  budew で居座る F-a-14 違反だった（2026-06-10 修正）。ダメカン配分 PICK-FIRST:
  ①drakloak を倒せる → 倒し、余りを dreepy＞drakloak＞残 HP≤80 ex＞ex＞他。
  ②dreepy を倒せる → 同様。③duskull を倒せる → 同様。
  ④相手に `fezandipiti-ex` → それに1個、他は dreepy＞drakloak＞残 HP≤80 ex＞ex＞他。
  ⑤上記外 → エネ無し dreepy 系＞エネ付き dreepy＞エネ付き drakloak＞他 に3個ずつ。
- `F15`（F-a-15）ポケモンをにがす等して `むずむずかふん`。**原文自認の暫定ノード**（「適当」）→
  実装は `RETREAT_TO_BUDEW`→`むずむずかふん`、不能なら end_turn（G4）。

### 8.2 ボス判定（D. ボスの指令を使うタイミング）
`F-ENTRY` から分岐。自分のサイド枚数で D-a-1〜D-a-4 に振り分け（D-1）。
- グローバル制約（D 冒頭、§5.7 と同じ）: 相手サイド ≤1 ではカースドボム不可（判定「いいえ」へ）。
  `P_itchyLastTurn` なら手札グッズは無いものとして扱う。
- `D-a-1`（自分サイド4）: 相手に残 HP ≤200 ex + (合計 HP ≤60 の非ルールポケ2匹 or ex1匹) → 手札 boss?
  boss で ex を呼び `ファントムダイブ`（残 HP60 にダメカン）→ **D-b-1（勝利）** : meowth-ex で boss
  サーチ→D-a-1 ループ : F-a-1。
  / 相手に残 HP ≤330 ex + 同条件 → 自場に dusknoir + 手札 boss? カースドボム13個→boss 呼び→
  `ファントムダイブ`（HP≤60 の2匹 or ex1匹に6個）: meowth-ex で boss サーチ→ループ : F-a-1。else F-a-1。
- `D-a-2`（サイド3）: 相手に残 HP ≤200 ex + 合計 HP ≤60 非ルールポケ → boss で ex（複数は自分から
  見て右端＝G5/G1）呼び `ファントムダイブ`→D-b-1 / meowth-ex/ultra-ball で boss 補充→ループ。
  else ベンチに使える dusclops/dusknoir + カースドボムで相手 HP ≤60 → 使う→ループ。
  else カースドボムで相手 ex を HP ≤200 に → 使う→ループ。else F-a-1。
- `D-a-3-①`（サイド2）: 相手バトル場に残 HP ≤200 ex → `ファントムダイブ`。else 相手ベンチに残 HP
  ≤200 ex → boss で呼び `ファントムダイブ`→D-b-1 / meowth-ex/ultra-ball で boss 補充→ループ : D-a-3-②。
- `D-a-3-②`: 相手に HP ≤60 と HP ≤200 のポケ → boss で HP 高いほうを呼び `ファントムダイブ`→D-b-1 /
  boss 補充ループ : F-a-1。
- `D-a-4-①`（サイド1）: 相手バトル場 HP ≤200 or 相手ベンチに HP ≤60 → `ファントムダイブ`（HP≤60 に
  6個、バトル場 HP≤200 ならのせ先任意）。else D-a-4-②。
- `D-a-4-②`: 相手に カースドボムで倒せるポケ → カースドボムで倒し D-b-1。else D-a-4-③。
- `D-a-4-③`: 相手に HP ≤200 → boss で呼び `ファントムダイブ`→D-b-1 / meowth-ex/ultra-ball で boss
  補充→ループ : D-a-4-④。
- `D-a-4-④`: カースドボムで相手 HP ≤200 or ≤60 になるポケ → バトル場なら使う / ベンチでも boss か
  meowth-ex を持つなら使う→ループ : F-a-1。
- `D-b-1`: 勝利（演出のみ＝G5。実装上は最後の攻撃で終局判定にまかせる）。

### 8.3 きぜつ時（E. 自分のバトルポケモンがきぜつ）
`REFILL_ACTIVE`（§5.9）。

---

## 9. GAP / 曖昧点（実装前に要判断・勝手に補完しない）

竹内版と共通の GAP は同じ解決（G1〜G7・演出除去・rare-candy ブロッカー）を引き継ぐ。以下はよぴふっと
版固有、または再確認が要るもの。

- **GAP-Y1（解決済 — engine 修正、2026-06-09）** むずむずかふんのグッズロックは engine で
  `OpponentCannotPlayItem` timed effect として実装済だったが、`legal_actions` に反映されず（実行時
  `EffectBlocked` のみ）、StateDto にも timed_effects 非公開で bot から不可視だった。**engine の
  `actions::push_legal_play_card` の Item 分岐に `opponent_blocks_own_item_play` guard を追加**し、
  グッズロック中はアイテムを `legal_actions` から除外（実行時ブロックと整合する正しさ修正）。
  回帰テスト `legal_actions_excludes_item_when_opponent_blocks_item_play`。結果、原文「グッズは
  ないものとして扱う」は**合法手から自動成立**し、bot 側に検出コードは不要。GOODS スライスは
  「合法手にあるグッズを使う」だけでよい（ロック中は候補が出ない）。
- **GAP-Y2（実装対象外）** A-b の初手キープ基準はマリガン選択肢が無い現行エンジンでは表現できない。
  S2 はエンジン既定マリガンに委ねる。
- **GAP-Y3（近似）** A-c-1 / A-c-2 の差（エネ等の有無で先攻の active 優先順を変える）は、現行
  `ChooseInitialActive`（手札全体から1体）では「先攻=ヨマワル優先 / 後攻=スボミー優先」に集約。
  A-c-2 単独条件（リーリエもエネもポフィンもポケパッドもない）の分岐は near-equivalent として落とす。
- **GAP-Y4（要判断）** よぴふっと版は drakloak→dragapult-ex 進化に積極的（F-a-11 で「炎+超付き
  drakloak を dragapult-ex に進化」を明示）。竹内版流の「ドロンチ止め」をどこまで効かせるか
  （= 場に dragapult-ex が既にいるとき2匹目を作るか）を要確認。**暫定**: §5.2 のドロンチ止めを
  既定とし、F-a-11/F-a-12 の明示進化はそのガードに優先（攻撃用 dragapult-ex を1匹確保するまでは進化、
  確保後は止める）。
- **GAP-Y5（深い入れ子の近似）** F-a-11 上段「エネ無し・ベンチ4匹埋まり」の多段入れ子（ヨマワル→
  サマヨール→ヨノワールの順次進化＋ベンチ補充＋リーリエ）は、盤面前提が極めて限定的。実装は
  「進化できるなら進化（§5.2）→ できるグッズ/サポートで盤面整備（G4 寄り）」に圧縮する近似で開始し、
  原文との差が出たら個別ノードを起こす。
- **GAP-Y6（ノード ID 不整合）** 原文に重複/飛びがある（A-b-2 の「いる」分岐が2連、B 後攻が C と同じ
  B-a-4 から再採番、F-a-10 内が F-a-3-①/F-a-4 を指す、F-a-5-① / F-a-5② / F-a-3-① 等の枝番）。
  本正規化は**遷移先の意味**（盤面整備に戻る/進化に戻る/エネに戻る）で解釈し、番号の字面には
  依存しない。差異が出たら原文の文意で再正規化する。
- **GAP-Y7（解決済 — 実装、2026-06-10）** D 群の「合計 HP ≤60 の非ルールポケモン2匹 / 残HP≤60 ex1匹」
  判定と、それに対応するファントムダイブ 6 カウンタの割り振りを実装。`find_d_a_1_2`（サイド4/3）が
  本命 ex（≤200、必要なら ≤330 をカースドボム軟化）+ `find_extra_prize_targets`（残りサイド分の
  合計≤60 対象）の複合条件を判定し、満たせば「カースドボム→ボス→ファントムダイブ（pending_distribute で
  6 カウンタを合計≤60 対象に乗せて KO）」を 1 アクションずつ実行、満たさねば F-a-1（None）。ex 判定は
  `slug_is_ex`（registry の prize_value）。サイド1/2 は D-a-3/D-a-4 の generic ボス。残るのは補充ループの
  細かい順序など枝葉のみ。
- **GAP-Y8（演出ノード）** A-c-1「口角3mm」/ A-F-a-3「ため息」/ C-a-1「無邪気に喜ぶ」/ D-b-1「ピース」/
  F-a-15「適当」自認 は G5 で実装対象外。
- **GAP-9 引き継ぎ（解決済、2026-06-09 engine 修正）** 旧 `rare-candy` POL stub はマッチ中断しうった
  （竹内版 §9 GAP-9）が、engine 側で修正済み（commit `3bded44`：進化ライン registry 検証 +
  `rare_candy_base` legal gate / `11ac5c5`：最初の番の進化禁止）。**bot は C-a-6/F-a-5 の rare-candy を
  実装済**（`find_rare_candy`：手札に rare-candy + dusknoir があり場に duskull → duskull→dusknoir 直行
  〔dusclops 飛ばし〕、通常進化より優先。対象たねは `pending_rare_candy` 経由で offered pool から
  duskull を G1 選択）。12 シードの yopifutto 同士対戦で中断なしを確認
  （`dragapult_yopifutto_completes_across_seeds_with_rare_candy`）。

---

## 10. カード事実テーブル

竹内版 §10 と同一（同じデッキ・同じカード）。実装は本テーブルをハードコードせず `CardRegistry` から
ライブで読む（`find_by_slug` → `CardDef` の attacks/abilities/hp/weakness/retreat_cost）。詳細は
[`dragapult-ex.takeuchi.machine.md`](dragapult-ex.takeuchi.machine.md) §10 を参照。

よぴふっと版で特に効くカード事実:
- **`ファントムダイブ`** = 炎1+超1、200 + 相手ベンチにダメカン6個(=60)分配。F-a-14 の配分規則が中核。
- **`カースドボム`** = dusclops:5個(50) / dusknoir:13個(130)、使用後自滅。相手サイド ≤1 で使用不可（§5.7）。
- **`むずむずかふん`**（budew）= 0エネ・10 + 相手次番グッズ不可。`P_itchyLastTurn` の発生源。
- **`おくのてキャッチ`**（meowth-ex）= triggered（ベンチ出しで発火）でサポート（主に boss/crispin/lillie）サーチ。
- **`さかてにとる`**（fezandipiti-ex）= 前番に自ポケ KO されていれば3ドロー（F-a-10）。
- **`ていさつしれい`**（drakloak）= 1/turn・山上2枚→1枚手札（§5.6 RECON）。
