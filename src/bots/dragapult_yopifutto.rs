//! `DragapultYopifuttoBot` — `decks/dragapult-ex.yaml` 固定デッキの決め打ち戦略 bot
//! (よぴふっと博士版ペルソナ)。
//!
//! 仕様の真値は `docs/bots/dragapult-ex.yopifutto.machine.md` (機械可読正規化版)。
//! 戦略ロジックはスライス単位で積み、未実装の判断は [`RandomPolicy`] に委譲する。
//!
//! 竹内版 ([`super::dragapult_takeuchi`]) と盤面読みヘルパー (slug 解決 / 進化 + ドロンチ止め /
//! ていさつしれい検出 / たね展開) を共有し (mod.rs)、ペルソナ差分 (ジャンケン勝ち→先攻 /
//! 初期 active 優先順 等) のみをここに持つ。判断に必要なカード事実は [`CardFacts`] から引く。

use crate::cards::CardFacts;
use crate::transport::TransportError;
use crate::wire::action::{ActionDto, ActionTarget};
use crate::wire::protocol::{PromptDto, PromptMsg, RequestMsg};
use crate::wire::state::{EntityDto, PokemonInPlayDto, StateDto};

use rand::Rng;
use rand_chacha::ChaCha20Rng;

use super::{
    count_in_hand, count_in_play, find_basic_placement, find_evolution, find_recon_directive,
    is_hand_discard, my_slug_of, my_turn_number, pick_by_slug_priority, pick_from_zone,
    play_card_slug, shuffle_strs, BotPolicy, PromptChoice, RandomPolicy,
};

/// S3: バトル場の最初の1体の優先度 (先攻、A-c-1: ヨマワル＞ドラメシヤ＞スボミー)。仕様 §4。
const ACTIVE_PRIORITY_FIRST: &[&str] =
    &["duskull", "dreepy", "budew", "fezandipiti-ex", "meowth-ex"];
/// S3: バトル場の最初の1体の優先度 (後攻、A-c-2: スボミー＞ドラメシヤ＞ヨマワル)。仕様 §4。
const ACTIVE_PRIORITY_SECOND: &[&str] =
    &["budew", "dreepy", "duskull", "fezandipiti-ex", "meowth-ex"];

/// 通常進化 (blanket) の優先度。原文 F-a-5「持っているサマヨール・ヨノワール・ドロンチのみ
/// 場のポケモンたちをすべて進化させ」に忠実 — **ドラパルトex は含めない**。
/// drakloak→dragapult-ex は F-a-11/F-a-12 のファントムダイブ用アタッカー準備パス専用 (後続スライス)。
const EVOLVE_PRIORITY_BLANKET: &[&str] = &["drakloak", "dusclops", "dusknoir"];
/// たね展開の優先度 (PLACE_BASICS、ドラメシヤ最優先。ex は条件付きで除外、仕様 §5.1)。
const BENCH_BASIC_PRIORITY: &[&str] = &["dreepy", "duskull", "budew"];

/// dragapult-ex 固定デッキの決め打ち戦略 bot (よぴふっと博士版)。
pub struct DragapultYopifuttoBot {
    /// startup に clone した registry (orchestrator に move される本体とは別実体)。
    /// ワザ名→index 解決などカード事実の参照に使う。
    registry: CardFacts,
    /// 未実装の判断のフォールバック先。
    fallback: RandomPolicy,
    /// 直前のアクション (ボスの指令 / カースドボム) で決めた対象 entity。直後の
    /// `ChooseTargetPokemon` でこれを使う (原文が的を指定する場面の橋渡し)。take 後にクリア。
    pending_target: Option<u32>,
    /// 直前に出した探索カードの slug (poffin / poke-pad / ultra-ball / meowth-ex / crispin)。
    /// 直後の `ChooseFromZone` でカード別・番別の対象選択に使う。take 後にクリア。
    pending_search: Option<String>,
    /// 番内フラグの基準ターン (変わったらリセット)。
    current_turn: Option<u32>,
    /// この番にアンフェアスタンプを使ったか (スペシャルレッドカードの F-a-13 条件用)。
    unfair_stamp_used_this_turn: bool,
    /// D-a-1/D-a-2 の詰めで、ファントムダイブのダメカン 6 個を乗せて倒す相手 entity 群
    /// (合計HP≤60 の非ルールポケ2匹 / 残HP≤60 ex1匹)。直後の DistributeDamage で使う。take 後クリア。
    pending_distribute: Option<Vec<u32>>,
    /// C-a-6 / F-a-5 ふしぎなアメを出した直後か (対象たね = duskull を選ぶ)。
    /// 直後の `ChooseTargetPokemon` で消費。
    pending_rare_candy: bool,
}

impl DragapultYopifuttoBot {
    #[must_use]
    pub fn new(registry: CardFacts) -> Self {
        Self {
            registry,
            fallback: RandomPolicy,
            pending_target: None,
            pending_search: None,
            current_turn: None,
            unfair_stamp_used_this_turn: false,
            pending_distribute: None,
            pending_rare_candy: false,
        }
    }
}

impl BotPolicy for DragapultYopifuttoBot {
    fn choose_action(
        &mut self,
        req: &RequestMsg,
        rng: &mut ChaCha20Rng,
    ) -> Result<ActionDto, TransportError> {
        let state = &req.state;
        let legal = &req.legal_actions;
        // 番が変わったら番内フラグをリセット。
        if self.current_turn != Some(state.turn) {
            self.current_turn = Some(state.turn);
            self.unfair_stamp_used_this_turn = false;
            self.pending_rare_candy = false;
        }
        // 全行動に優先して特性「ていさつしれい」(drakloak) を使う (仕様 §5.6 RECON / C-a-10 / F-a-9)。
        if let Some(a) = find_recon_directive(state, legal) {
            // 加える1枚は recon 専用の優先 (場ポケ数 + 番別、C-a-10/F-a-9) で選ぶ。
            self.pending_search = Some("recon".to_string());
            return Ok(a);
        }
        // さかてにとる (§F-a-10): 前の番に自ポケがきぜつしていれば、場の fezandipiti-ex で
        // 3 ドロー (UseAbility は engine が KO 条件でゲート)。場にいなければ手札から出して補充。
        if let Some(a) = find_take_advantage(state, legal) {
            return Ok(a);
        }
        if let Some(a) = find_fezandipiti_setup(state, legal) {
            return Ok(a);
        }
        // C-a-6 / F-a-5 ふしぎなアメ: 手札に rare-candy + dusknoir があれば duskull→dusknoir 直行
        // (dusclops 飛ばし)。通常進化 (duskull→dusclops) より優先するため find_evolution の前。
        if let Some(a) = self.find_rare_candy(state, legal) {
            return Ok(a);
        }
        // 通常進化 (drakloak/dusclops/dusknoir、ドラパルトex は含めない) → たね展開 (§5.1 / §5.2)。
        if let Some(a) = find_evolution(state, legal, EVOLVE_PRIORITY_BLANKET) {
            return Ok(a);
        }
        // アタッカー準備 (F-a-11/F-a-12): 場に dragapult-ex がいないとき、炎+超 が付いた drakloak を
        // dragapult-ex に進化させてファントムダイブ可能にする (通常進化には含めない専用パス)。
        if let Some(a) = find_attacker_evolution(state, legal) {
            return Ok(a);
        }
        if let Some(a) = find_basic_placement(state, legal, BENCH_BASIC_PRIORITY) {
            return Ok(a);
        }
        // GOODS (§5.3): なかよしポフィン → ポケパッド → ハイパーボール。グッズロック中は
        // engine が legal_actions から除外する (むずむずかふん修正済) ので「合法なら使う」でよい。
        if let Some(a) = find_goods_play(state, legal) {
            // 直後の探索 prompt 用に、出したグッズ slug を記録 (カード別の対象選択)。
            self.pending_search = play_card_slug(state, &a).map(str::to_string);
            return Ok(a);
        }
        // エネ付与 (§5.4): ドラパルトライン (drakloak>dragapult-ex>dreepy) を炎+超に近づける。
        // 炎炎/超超は作らない・充足済みは飛ばす・ベンチ優先。手張りは番1回 (engine が legal で制御)。
        if let Some(a) = find_energy_attach(state, legal) {
            return Ok(a);
        }
        // ボス判定 D 群 (F-ENTRY: 自分サイド≤4 かつファントムダイブ可能 → D-1)。サイド枚数別に
        // 「詰め」アクション (ボスで KO 対象を呼ぶ / ボス補充の ニャースex / カースドボム詰め) を返す。
        // 該当しなければ F-a 通常路へ。F-a-13 ボスより優先 (低サイドの詰めが最優先)。
        if let Some(a) = self.find_d_group_action(state, legal, rng) {
            return Ok(a);
        }
        // ボスの指令 (F-a-13 の①②条件のみ、サイド>4 等で D 群に入らない場合)。
        if let Some(a) = self.find_boss_fa13(state, legal) {
            return Ok(a);
        }
        // サポート (§5.5): アカマツ (エネ加速) → リーリエ (ドロー)。サポートは番1回。
        if let Some(a) = find_supporter(state, legal) {
            // アカマツは基本エネサーチ prompt を出す (リーリエは出さない)。
            self.pending_search = play_card_slug(state, &a).map(str::to_string);
            return Ok(a);
        }
        // カースドボム (ヨノワール、自滅して相手にダメカン13): 相手サイド>1 かつ 130 で倒せる ex が
        // いるときだけ使う (1 prize 自滅 → 2 prize KO の純益)。的は F-a-12「残りHPの大きい方」の ex。
        if let Some(a) = self.find_cursed_bomb(state, legal) {
            return Ok(a);
        }
        // 手札妨害 (F-a-3-① アンフェアスタンプ / F-a-13 いいえ スペシャルレッドカード)。
        if let Some(a) = find_disruption(state, legal, self.unfair_stamp_used_this_turn) {
            if play_card_slug(state, &a) == Some("unfair-stamp") {
                self.unfair_stamp_used_this_turn = true;
            }
            return Ok(a);
        }
        // 退避 (§5.8): バトル場がアタッカーでないとき、ファントムダイブ可能な dragapult-ex >
        // budew をバトル場へ出すためにげる。にげが合法 (にげエネ充足) なときのみ。にげ後は
        // 同じ番に新 active で find_attack が発火する (にげは番1回、engine が legal で制御)。
        if let Some(a) = find_retreat(state, legal) {
            return Ok(a);
        }
        // ワザ: バトル場が dragapult-ex なら ファントムダイブ、budew なら むずむずかふん。
        // (原文はジェットヘッドを使わない。dreepy/drakloak のワザも使わない)。ダメカン配分は
        // choose_prompt の DistributeDamage で HP 低い順に処理 (§F-a-14 の近似)。
        if let Some(a) = self.find_attack(state, legal) {
            return Ok(a);
        }
        // TODO(後続スライス): サポート (リーリエ/アカマツ/おくのてキャッチ補充) / ボス判定 (D 群) /
        // カースドボム。未実装局面は「番を終える」を既定とする (random にすると未実装トレーナーズ・
        // rare-candy stub を誤爆するため)。
        if let Some(end) = legal.iter().find(|a| matches!(a, ActionDto::EndTurn)) {
            return Ok(end.clone());
        }
        self.fallback.choose_action(req, rng)
    }

    fn choose_prompt(&mut self, p: &PromptMsg, rng: &mut ChaCha20Rng) -> PromptChoice {
        match &p.kind {
            // G6: ジャンケン (コイン) に勝ったら先攻を選ぶ (yes=true → 自分が先攻)。
            // ※竹内版は後攻。よぴふっと版は原文 A-a「勝ち＝先攻をとる」。
            PromptDto::ChooseFirstOrSecond => PromptChoice {
                selected: vec![],
                counts: vec![],
                yes: Some(true),
                branch_index: None,
            },
            // S3: バトル場の最初の1体を先攻/後攻別の優先度で選ぶ。
            PromptDto::ChooseInitialActive { eligible } => self
                .pick_initial_active(p, eligible)
                .unwrap_or_else(|| self.fallback.choose_prompt(p, rng)),
            // §4 S4: 原文の対戦準備手順は「バトル場に1体出す → サイド6枚 → 1回目の番へ」で
            // 終わっており、ベンチ初期配置のステップが存在しない。よって **0 枚 (何も置かない)**。
            // (旧実装は「原文指定なし → ランダム」と発明していたが、原文には動作自体が無いため誤り。
            // 2026-06-10 ユーザー確定で訂正)。
            PromptDto::PlaceInitialBench { .. } => PromptChoice {
                selected: vec![],
                counts: vec![],
                yes: None,
                branch_index: None,
            },
            // 対象 1 匹選択: ふしぎなアメ (C-a-6/F-a-5) の直後は対象たね = duskull (複数は G1)。
            // それ以外は pending_target (ボス/カースドボムの的)。原文が的を指定しない場面・pending が
            // 無効なら eligible から無作為 (発明しない)。
            PromptDto::ChooseTargetPokemon { targets } => {
                let chosen = if std::mem::take(&mut self.pending_rare_candy) {
                    pick_rare_candy_target(p.state.as_ref(), targets, rng)
                } else {
                    self.pending_target
                        .take()
                        .filter(|t| targets.contains(t))
                        .or_else(|| {
                            (!targets.is_empty()).then(|| targets[rng.gen_range(0..targets.len())])
                        })
                };
                match chosen {
                    Some(t) => PromptChoice {
                        selected: vec![t],
                        counts: vec![],
                        yes: None,
                        branch_index: None,
                    },
                    None => self.fallback.choose_prompt(p, rng),
                }
            }
            // ファントムダイブのダメカン配分 (§F-a-14): ①ドロンチ②ドラメシヤ③ヨマワルが倒せれば
            // 倒し、余りを [ドラメシヤ>ドロンチ>残HP80以下ex>ex>他] に。④キチキギスexに1個。
            // ⑤該当無しは [エネ無しドラメシヤ>エネ付きドラメシヤ>エネ付きドロンチ>他] に3個ずつ。
            PromptDto::DistributeDamage {
                eligible,
                total,
                per_target_max,
            } => {
                // D-a-1/D-a-2 の詰め: pending_distribute (合計HP≤60 対象) を倒すよう乗せる。
                // 無ければ F-a-14 の汎用配分。
                let planned = self
                    .pending_distribute
                    .take()
                    .and_then(|t| distribute_to_targets(p, eligible, &t, *total, *per_target_max));
                planned
                    .or_else(|| self.distribute_phantom_dive(p, eligible, *total, *per_target_max))
                    .unwrap_or_else(|| self.fallback.choose_prompt(p, rng))
            }
            // ChooseFromZone は (a) 手札トラッシュ cost (ハイパーボール) と (b) デッキ探索の
            // 両方に使われる。候補が自分の手札にあるかで判別する (zone は engine 固定のため)。
            PromptDto::ChooseFromZone { options, .. } => {
                if is_hand_discard(p, options) {
                    // (a) トラッシュ cost: 原文 B-a-3 の順 (無作為 tier は rng) で max 枚 (drakloak 温存)。
                    PromptChoice {
                        selected: pick_trash_targets(options, usize::from(p.max), rng),
                        counts: vec![],
                        yes: None,
                        branch_index: None,
                    }
                } else {
                    // (b) デッキ探索: 直前に出したカード (pending_search) + 番 + 盤面で対象を選ぶ (§5.3)。
                    let source = self.pending_search.take();
                    let pri = p
                        .state
                        .as_ref()
                        .map(|s| search_priority(s, source.as_deref(), rng));
                    let pri_ref: &[&str] = pri.as_deref().unwrap_or(DEFAULT_FETCH_PRIORITY);
                    pick_from_zone(options, pri_ref, p.min, p.max)
                }
            }
            _ => self.fallback.choose_prompt(p, rng),
        }
    }
}

impl DragapultYopifuttoBot {
    /// S3: バトル場の最初の1体を選ぶ。`state` が無い場合は `None` (呼び出し側で random)。
    #[allow(clippy::unused_self)]
    fn pick_initial_active(&self, p: &PromptMsg, eligible: &[u32]) -> Option<PromptChoice> {
        let state = p.state.as_ref()?;
        // active_player == "me" ⟺ 自分が先攻 (mask は viewer 視点で "me"/"opp" を出す)。
        let is_first = state.active_player == "me";
        let priority = if is_first {
            ACTIVE_PRIORITY_FIRST
        } else {
            ACTIVE_PRIORITY_SECOND
        };
        let chosen = pick_by_slug_priority(state, eligible, priority)?;
        Some(PromptChoice {
            selected: vec![chosen],
            counts: vec![],
            yes: None,
            branch_index: None,
        })
    }

    /// C-a-6 / F-a-5 ふしぎなアメ: 手札に rare-candy + dusknoir があり 場に duskull がいれば、
    /// rare-candy を出して duskull→dusknoir 直行 (dusclops 飛ばし)。対象たねは pending_rare_candy
    /// 経由で ChooseTargetPokemon の offered pool (rare_candy_base filter 済) から duskull を選ぶ。
    fn find_rare_candy(&mut self, state: &StateDto, legal: &[ActionDto]) -> Option<ActionDto> {
        let rc = legal
            .iter()
            .find(|a| play_card_slug(state, a) == Some("rare-candy"))?;
        if count_in_hand(state, "dusknoir") < 1 || count_in_play(state, "duskull") < 1 {
            return None;
        }
        self.pending_rare_candy = true;
        Some(rc.clone())
    }

    /// ワザ選択: バトル場が dragapult-ex なら ファントムダイブ、budew なら むずむずかふん。
    /// 原文はジェットヘッド・dreepy/drakloak のワザを使わない。合法な (= コスト充足) UseAttack のみ返す。
    fn find_attack(&self, state: &StateDto, legal: &[ActionDto]) -> Option<ActionDto> {
        let slug = state.me.active.as_ref()?.card.as_deref()?;
        let prefs: &[&str] = match slug {
            "dragapult-ex" => &["ファントムダイブ"],
            "budew" => &["むずむずかふん"],
            _ => return None,
        };
        for name in prefs {
            let Some(idx) = self.attack_index_by_name(slug, name) else {
                continue;
            };
            if legal
                .iter()
                .any(|a| matches!(a, ActionDto::UseAttack { attack_index } if *attack_index == idx))
            {
                return Some(ActionDto::UseAttack { attack_index: idx });
            }
        }
        None
    }

    /// ボスの指令 (F-a-13): ファントムダイブを使えて、ボスがあり、原文の①②に当てはまる場合のみ使う。
    /// ①相手 dragapult-ex 不在 & 相手ベンチに残 HP≤200 の キチキギスex/ニャースex → キチキギスex優先で呼ぶ。
    /// ②相手 dragapult-ex 不在 & 相手ドロンチが場に1匹のみ → その(ベンチの)ドロンチを呼ぶ。
    /// 呼ぶ対象は pending_target に置く。①②外では使わない。
    fn find_boss_fa13(&mut self, state: &StateDto, legal: &[ActionDto]) -> Option<ActionDto> {
        if !has_phantom_ready_dragapult(state) {
            return None;
        }
        let boss = legal
            .iter()
            .find(|a| play_card_slug(state, a) == Some("boss-s-orders"))?;
        // ①② とも前提: 相手の場 (バトル場+ベンチ) に dragapult-ex がいない。
        let opp_has_dragapult = state
            .opp
            .active
            .iter()
            .chain(state.opp.bench.iter())
            .any(|p| p.card.as_deref() == Some("dragapult-ex"));
        if opp_has_dragapult {
            return None;
        }
        // ① 相手ベンチに残 HP≤200 の キチキギスex/ニャースex (fezandipiti-ex 優先)。
        for want in ["fezandipiti-ex", "meowth-ex"] {
            if let Some(p) = state.opp.bench.iter().find(|p| {
                p.card.as_deref() == Some(want) && p.hp_max.saturating_sub(p.damage) <= 200
            }) {
                self.pending_target = Some(p.entity_id);
                return Some(boss.clone());
            }
        }
        // ② 相手ドロンチが場に1匹のみ → ベンチにいればそれを呼ぶ。
        let opp_drakloak_total = state
            .opp
            .active
            .iter()
            .chain(state.opp.bench.iter())
            .filter(|p| p.card.as_deref() == Some("drakloak"))
            .count();
        if opp_drakloak_total == 1 {
            if let Some(p) = state
                .opp
                .bench
                .iter()
                .find(|p| p.card.as_deref() == Some("drakloak"))
            {
                self.pending_target = Some(p.entity_id);
                return Some(boss.clone());
            }
        }
        None
    }

    /// ボス判定 D 群 (F-ENTRY + D-1〜D-a-4)。自分のサイド ≤4 かつファントムダイブ可能なときのみ。
    /// (a) サイド4/3 (D-a-1/D-a-2): 本命 ex + 合計HP≤60 対象で取り切る複合条件の詰めのみ。満たさねば
    /// None (= 原文の「F-a-1 へ」。generic ボスには落とさない)。
    /// (b) サイド1 (D-a-4②): カースドボム finisher。
    /// (c) サイド1/2 (D-a-3-②/D-a-4③): 相手ベンチの KO 可能対象 (残HP≤200) をボスで呼ぶ。ボスが
    /// 無ければ ニャースex で補充。
    /// 深い枝 (補充の細かい順序等) は近似。
    fn find_d_group_action(
        &mut self,
        state: &StateDto,
        legal: &[ActionDto],
        rng: &mut ChaCha20Rng,
    ) -> Option<ActionDto> {
        let sides = state.me.prizes.len();
        if sides > 4 || !has_phantom_ready_dragapult(state) {
            return None;
        }
        // サイド4/3 (D-a-1 / D-a-2): 複合条件の詰めのみ。満たさねば None (F-a-1 へ。generic に落とさない)。
        if sides >= 3 {
            return self.find_d_a_1_2(state, legal);
        }
        // 以下サイド1/2 (D-a-3 / D-a-4)。
        // サイド1 (D-a-4②): カースドボムで直接倒せる相手がいれば倒して詰める。
        if sides == 1 {
            if let Some(a) = self.find_cursed_bomb_finisher(state, legal) {
                return Some(a);
            }
        }
        // ボスで引っ張る KO 可能対象 (相手ベンチ。ファントムダイブ 200 で残 HP≤200 を KO)。
        // ex は全サイドで対象 (D-a-1〜4)。非ex はサイド≤2 (D-a-3-②/D-a-4③) で対象。
        // (相手バトル場の KO はボスでなく退避→ファントムダイブが処理するためベンチのみ走査)。
        let ko_targets: Vec<(u32, u16)> = state
            .opp
            .bench
            .iter()
            .filter_map(|p| {
                let rem = p.hp_max.saturating_sub(p.damage);
                let is_ex = p.card.as_deref().is_some_and(|s| self.slug_is_ex(s));
                let ko_able = rem <= 200 && (is_ex || sides <= 2);
                ko_able.then_some((p.entity_id, rem))
            })
            .collect();
        if ko_targets.is_empty() {
            return None;
        }
        // 対象選択: サイド2 (D-a-3-②) は原文「HP が高い方」を呼ぶ。それ以外は原文が一意に
        // 定めない (D-a-2「一番右」は物理指定) ので無作為。
        let target = if sides == 2 {
            ko_targets
                .iter()
                .max_by_key(|(_, rem)| *rem)
                .map_or(ko_targets[0].0, |(e, _)| *e)
        } else {
            ko_targets[rng.gen_range(0..ko_targets.len())].0
        };
        // ボスがあれば呼ぶ。無ければ ニャースex で補充 (おくのてキャッチでボスを手札へ)。
        if let Some(boss) = legal
            .iter()
            .find(|a| play_card_slug(state, a) == Some("boss-s-orders"))
        {
            self.pending_target = Some(target);
            return Some(boss.clone());
        }
        if let Some(meowth) = legal
            .iter()
            .find(|a| play_card_slug(state, a) == Some("meowth-ex"))
        {
            // おくのてキャッチで「ボスの指令」を取りにいく。
            self.pending_search = Some("okunote-boss".to_string());
            return Some(meowth.clone());
        }
        None
    }

    /// D-a-1 / D-a-2 (サイド4/3) の詰め: 本命 ex (ファントムダイブ 200、必要ならカースドボム 130 で
    /// 軟化) を倒し、残りサイドを 6 カウンタ (合計HP≤60 の非ルールポケ2匹 / 残HP≤60 ex1匹) で取り切る。
    /// 手順を 1 アクションずつ返す: ①軟化が要るなら カースドボム → ②本命 ex をボスで前面へ
    /// (pending_distribute に 6 カウンタ先を記録) → ③本命が前面なら None (find_attack でファントム
    /// ダイブ、配分は pending_distribute)。ボスが手札に無ければ ニャースex で補充。条件を満たさねば None。
    fn find_d_a_1_2(&mut self, state: &StateDto, legal: &[ActionDto]) -> Option<ActionDto> {
        let sides = state.me.prizes.len();
        let boss_in_hand = count_in_hand(state, "boss-s-orders") >= 1;
        let dusknoir_legal = legal.iter().any(|a| {
            matches!(a, ActionDto::UseAbility { entity_id, .. }
                if my_slug_of(state, entity_id.0) == Some("dusknoir"))
        });
        // カースドボムで軟化できるのは「dusknoir 起動可 + 相手サイド>1 + ボス手札」(D-a-1 b)。
        let can_cursed = dusknoir_legal && state.opp.prizes.len() > 1 && boss_in_hand;
        let max_main_hp: u16 = if can_cursed { 330 } else { 200 };
        // 本命 ex: まず ≤200 (軟化不要)、無ければ ≤max_main_hp の ex。残HP昇順 (倒しやすい順)。
        let main = self.pick_main_ex(state, max_main_hp)?;
        // 残りサイド分の「6 カウンタで倒す対象」(本命 ex を除く)。
        let extra = self.find_extra_prize_targets(state, sides.saturating_sub(2), main.0)?;
        // ボスが無ければ ニャースex で補充 (おくのてキャッチでボスを取りにいく)。
        if !boss_in_hand {
            if let Some(m) = legal
                .iter()
                .find(|a| play_card_slug(state, a) == Some("meowth-ex"))
            {
                self.pending_search = Some("okunote-boss".to_string());
                return Some(m.clone());
            }
            return None;
        }
        // ① 本命 ex が残HP>200 → カースドボムで軟化 (max_main_hp=330 は can_cursed のときのみ)。
        if main.1 > 200 {
            if let Some(bomb) = legal.iter().find(|a| {
                matches!(a, ActionDto::UseAbility { entity_id, .. }
                    if my_slug_of(state, entity_id.0) == Some("dusknoir"))
            }) {
                self.pending_target = Some(main.0);
                return Some(bomb.clone());
            }
            return None;
        }
        // ② 本命 ex が前面でなければボスで呼ぶ。6 カウンタ先を記録。
        let main_is_active = state
            .opp
            .active
            .as_ref()
            .is_some_and(|p| p.entity_id == main.0);
        if !main_is_active {
            if let Some(boss) = legal
                .iter()
                .find(|a| play_card_slug(state, a) == Some("boss-s-orders"))
            {
                self.pending_target = Some(main.0);
                self.pending_distribute = Some(extra);
                return Some(boss.clone());
            }
            return None;
        }
        // ③ 本命 ex が前面 → ファントムダイブは find_attack に委ね、配分先だけ記録して None。
        self.pending_distribute = Some(extra);
        None
    }

    /// 本命 ex を選ぶ: 残HP ≤ `max_hp` の相手 ex のうち、まず ≤200 (軟化不要)、無ければ ≤max_hp、
    /// その中で残HP昇順 (倒しやすい順)。`(entity_id, 残HP)`。
    fn pick_main_ex(&self, state: &StateDto, max_hp: u16) -> Option<(u32, u16)> {
        let exes: Vec<(u32, u16)> = state
            .opp
            .active
            .iter()
            .chain(state.opp.bench.iter())
            .filter(|p| p.card.as_deref().is_some_and(|s| self.slug_is_ex(s)))
            .map(|p| (p.entity_id, p.hp_max.saturating_sub(p.damage)))
            .filter(|(_, rem)| *rem <= max_hp)
            .collect();
        // ≤200 を優先 (軟化不要)、その中で残HP最小。無ければ全体で残HP最小。
        exes.iter()
            .filter(|(_, rem)| *rem <= 200)
            .min_by_key(|(_, rem)| *rem)
            .or_else(|| exes.iter().min_by_key(|(_, rem)| *rem))
            .copied()
    }

    /// 残り `prizes_needed` サイドを 6 カウンタ (60 ダメージ) で取り切る対象 (本命 `exclude` を除く)。
    /// 2 サイド: 残HP≤60 の ex1匹 / 合計残HP≤60 の非ルールポケ2匹。1 サイド: 残HP≤60 の非ルールポケ1匹。
    /// 0 サイド: 空。取れなければ None (D-a-1/2 の複合条件不成立 → F-a-1 へ)。
    fn find_extra_prize_targets(
        &self,
        state: &StateDto,
        prizes_needed: usize,
        exclude: u32,
    ) -> Option<Vec<u32>> {
        if prizes_needed == 0 {
            return Some(vec![]);
        }
        // 本命を除く相手ポケ (id, 残HP, ex か)。
        let pokes: Vec<(u32, u16, bool)> = state
            .opp
            .active
            .iter()
            .chain(state.opp.bench.iter())
            .filter(|p| p.entity_id != exclude)
            .map(|p| {
                (
                    p.entity_id,
                    p.hp_max.saturating_sub(p.damage),
                    p.card.as_deref().is_some_and(|s| self.slug_is_ex(s)),
                )
            })
            .collect();
        match prizes_needed {
            2 => {
                // ex1匹 (残HP≤60)。
                if let Some((id, _, _)) = pokes.iter().find(|(_, rem, ex)| *ex && *rem <= 60) {
                    return Some(vec![*id]);
                }
                // 非ルールポケ2匹で合計残HP≤60。
                let nr: Vec<(u32, u16)> = pokes
                    .iter()
                    .filter(|(_, _, ex)| !ex)
                    .map(|(id, rem, _)| (*id, *rem))
                    .collect();
                for i in 0..nr.len() {
                    for j in (i + 1)..nr.len() {
                        if nr[i].1.saturating_add(nr[j].1) <= 60 {
                            return Some(vec![nr[i].0, nr[j].0]);
                        }
                    }
                }
                None
            }
            1 => pokes
                .iter()
                .find(|(_, rem, ex)| !*ex && *rem <= 60)
                .map(|(id, _, _)| vec![*id]),
            _ => None,
        }
    }

    /// カースドボム詰め (D-a-4②): カースドボム (dusknoir=130) で直接倒せる相手 (残 HP≤130) が
    /// いれば使う。グローバル制約: 相手サイド ≤1 では使わない (= D 群サイド1 では相手サイドを確認)。
    /// 的 (残 HP≤130 の中で最大) を pending_target に置く。
    fn find_cursed_bomb_finisher(
        &mut self,
        state: &StateDto,
        legal: &[ActionDto],
    ) -> Option<ActionDto> {
        if state.opp.prizes.len() <= 1 {
            return None;
        }
        let bomb = legal.iter().find(|a| {
            matches!(a,
                ActionDto::UseAbility { entity_id, .. }
                    if my_slug_of(state, entity_id.0) == Some("dusknoir"))
        })?;
        let target = state
            .opp
            .active
            .iter()
            .chain(state.opp.bench.iter())
            .filter(|p| p.hp_max.saturating_sub(p.damage) <= 130)
            .max_by_key(|p| p.hp_max.saturating_sub(p.damage))?;
        self.pending_target = Some(target.entity_id);
        Some(bomb.clone())
    }

    /// カースドボム (ヨノワール、§F-a-12): ファントムダイブを使える dragapult-ex が場にいて、
    /// カースドボムが使える dusknoir がいて、相手の場にポケモンex がいるとき、相手 ex
    /// (複数なら残りHPの大きい方) に 13 個のせる (ファントムダイブ前の軟化、KO 不問)。
    /// グローバル制約: 相手サイド ≤ 1 では使わない (自滅で最後のサイドを渡すため)。
    fn find_cursed_bomb(&mut self, state: &StateDto, legal: &[ActionDto]) -> Option<ActionDto> {
        if state.opp.prizes.len() <= 1 || !has_phantom_ready_dragapult(state) {
            return None;
        }
        let bomb = legal.iter().find(|a| {
            matches!(a,
                ActionDto::UseAbility { entity_id, .. }
                    if my_slug_of(state, entity_id.0) == Some("dusknoir"))
        })?;
        // 相手の場の ex のうち残り HP 最大 (F-a-12「残りHPの大きい方」)。
        let target = state
            .opp
            .active
            .iter()
            .chain(state.opp.bench.iter())
            .filter(|p| p.card.as_deref().is_some_and(|s| self.slug_is_ex(s)))
            .max_by_key(|p| p.hp_max.saturating_sub(p.damage))?;
        self.pending_target = Some(target.entity_id);
        Some(bomb.clone())
    }

    /// ファントムダイブのダメカン配分 (§F-a-14)。`eligible` は相手ベンチ。
    /// ①ドロンチ②ドラメシヤ③ヨマワルが倒せれば 1 匹倒し、余りを [ドラメシヤ>ドロンチ>残HP80以下
    /// ex>ex>他] に。④該当無し+キチキギスex なら 1 個。⑤さらに該当無しは
    /// [エネ無しドラメシヤ>エネ付きドラメシヤ>エネ付きドロンチ>他] に 3 個ずつ。`cap`/`total` 厳守。
    #[allow(clippy::too_many_lines)]
    fn distribute_phantom_dive(
        &self,
        p: &PromptMsg,
        eligible: &[u32],
        total: u8,
        per_target_max: Option<u8>,
    ) -> Option<PromptChoice> {
        let state = p.state.as_ref()?;
        if total == 0 || eligible.is_empty() {
            return None;
        }
        let cap = per_target_max.unwrap_or(total);
        if cap == 0 {
            return None;
        }
        // (entity, slug, 残り HP, エネ付き, ex か)
        let ts: Vec<(u32, String, u16, bool, bool)> = eligible
            .iter()
            .filter_map(|&e| {
                let mon = opp_in_play(state, e)?;
                let slug = mon.card.as_deref().unwrap_or("").to_string();
                let remaining = mon.hp_max.saturating_sub(mon.damage);
                let is_ex = self.slug_is_ex(&slug);
                Some((e, slug, remaining, !mon.energy_attached.is_empty(), is_ex))
            })
            .collect();
        if ts.is_empty() {
            return None;
        }
        let ko_cost = |remaining: u16| -> u8 {
            u8::try_from(remaining.div_ceil(10))
                .unwrap_or(u8::MAX)
                .max(1)
        };
        let mut counts: std::collections::HashMap<u32, u8> = std::collections::HashMap::new();
        let mut left = total;
        // Phase 1: ①ドロンチ ②ドラメシヤ ③ヨマワル を 1 匹 KO (倒せる = カウンタで残 HP を削り切れる)。
        // primary = KO した / キチキギスex に 1 個のせた対象。余り配分からは除外する
        // (原文「ドロンチにのせて倒す。余りを…」「キチキギスexに1個。ほかは…」)。
        let mut primary: Option<u32> = None;
        for ko_slug in ["drakloak", "dreepy", "duskull"] {
            if let Some(t) = ts
                .iter()
                .find(|t| t.1 == ko_slug && ko_cost(t.2) <= left && ko_cost(t.2) <= cap)
            {
                *counts.entry(t.0).or_default() += ko_cost(t.2);
                left -= ko_cost(t.2);
                primary = Some(t.0);
                break;
            }
        }
        // ④ キチキギスex に 1 個 (①②③ が無いとき)。
        if primary.is_none() {
            if let Some(t) = ts.iter().find(|t| t.1 == "fezandipiti-ex") {
                if left >= 1 && cap >= 1 {
                    *counts.entry(t.0).or_default() += 1;
                    left -= 1;
                    primary = Some(t.0);
                }
            }
        }
        let did_primary = primary.is_some();
        // Phase 2: 余り配分。primary (KO 済 / kiki) は除外。
        let mut order: Vec<&(u32, String, u16, bool, bool)> =
            ts.iter().filter(|t| Some(t.0) != primary).collect();
        if did_primary {
            // 余り: ドラメシヤ > ドロンチ > 残HP80以下 ex > ex > 他 (上位から cap まで詰める)。
            let rank = |t: &(u32, String, u16, bool, bool)| -> u8 {
                if t.1 == "dreepy" {
                    0
                } else if t.1 == "drakloak" {
                    1
                } else if t.4 && t.2 <= 80 {
                    2
                } else if t.4 {
                    3
                } else {
                    4
                }
            };
            order.sort_by_key(|t| (rank(t), t.0));
            for t in &order {
                while left > 0 && *counts.get(&t.0).unwrap_or(&0) < cap {
                    *counts.entry(t.0).or_default() += 1;
                    left -= 1;
                }
            }
        } else {
            // ⑤: エネ無しドラメシヤ > エネ付きドラメシヤ > エネ付きドロンチ > 他 に 3 個ずつ。
            let rank = |t: &(u32, String, u16, bool, bool)| -> u8 {
                if t.1 == "dreepy" && !t.3 {
                    0
                } else if t.1 == "dreepy" {
                    1
                } else if t.1 == "drakloak" && t.3 {
                    2
                } else {
                    3
                }
            };
            order.sort_by_key(|t| (rank(t), t.0));
            let per = 3u8.min(cap);
            for t in &order {
                if left == 0 {
                    break;
                }
                let give = per.min(left);
                *counts.entry(t.0).or_default() += give;
                left -= give;
            }
            // 端数が残れば上位から cap まで。
            for t in &order {
                while left > 0 && *counts.get(&t.0).unwrap_or(&0) < cap {
                    *counts.entry(t.0).or_default() += 1;
                    left -= 1;
                }
            }
        }
        // フォールバック: まだ余りがあれば、cap を超えない範囲で eligible 全体 (primary 含む) に
        // 端数を詰める。これで配分合計が必ず total に届く (engine の DistributionMismatch を防ぐ)。
        // 例: 対象が primary 1 体だけのとき、Phase 2 が primary を除外して残りカウンタを置けず
        // 合計不足になる問題の修正 (ファントムダイブは per_target_max なし=cap=total なので必ず収まる)。
        for t in &ts {
            while left > 0 && *counts.get(&t.0).unwrap_or(&0) < cap {
                *counts.entry(t.0).or_default() += 1;
                left -= 1;
            }
        }
        // counts > 0 の entity を eligible 順で出力 (selected と counts を対応させる)。
        let selected: Vec<u32> = ts
            .iter()
            .map(|t| t.0)
            .filter(|e| counts.get(e).copied().unwrap_or(0) > 0)
            .collect();
        if selected.is_empty() {
            return None;
        }
        let cnts: Vec<u8> = selected.iter().map(|e| counts[e]).collect();
        Some(PromptChoice {
            selected,
            counts: cnts,
            yes: None,
            branch_index: None,
        })
    }

    /// slug が「ルールを持つ (ex 等、サイド 2 枚)」ポケモンか。共有実装 (`super::slug_is_ex`) に委譲。
    fn slug_is_ex(&self, slug: &str) -> bool {
        super::slug_is_ex(&self.registry, slug)
    }

    /// カード slug のワザ一覧 (POL `CardEffectDef.attacks`、YAML 順 = engine の
    /// `attack_index` 基準) から、名前一致するワザの index を引く。
    fn attack_index_by_name(&self, slug: &str, attack_name: &str) -> Option<u8> {
        self.registry.attack_index(slug, attack_name)
    }
}

/// D-a-1/D-a-2 の詰め配分: `targets`(合計HP≤60 で倒す対象) を、各 KO に必要な分だけ乗せて倒す。
/// `eligible` に含まれ・`total`/`cap` を超えない範囲で。1 つも乗せられなければ None。
fn distribute_to_targets(
    p: &PromptMsg,
    eligible: &[u32],
    targets: &[u32],
    total: u8,
    per_target_max: Option<u8>,
) -> Option<PromptChoice> {
    let state = p.state.as_ref()?;
    if total == 0 {
        return None;
    }
    let cap = per_target_max.unwrap_or(total);
    let mut selected: Vec<u32> = Vec::new();
    let mut counts: Vec<u8> = Vec::new();
    let mut left = total;
    for &t in targets {
        if !eligible.contains(&t) {
            continue;
        }
        let Some(mon) = opp_in_play(state, t) else {
            continue;
        };
        let remaining = mon.hp_max.saturating_sub(mon.damage);
        let need = u8::try_from(remaining.div_ceil(10))
            .unwrap_or(u8::MAX)
            .max(1);
        let give = need.min(cap).min(left);
        if give == 0 {
            continue;
        }
        selected.push(t);
        counts.push(give);
        left -= give;
    }
    if selected.is_empty() {
        return None;
    }
    Some(PromptChoice {
        selected,
        counts,
        yes: None,
        branch_index: None,
    })
}

/// C-a-6 / F-a-5 ふしぎなアメの対象たね選択。offered pool (rare_candy_base filter 済) から
/// duskull を G1 (複数なら無作為) で選ぶ。duskull が無ければ pool から無作為 (発明しない)。
fn pick_rare_candy_target(
    state: Option<&StateDto>,
    targets: &[u32],
    rng: &mut ChaCha20Rng,
) -> Option<u32> {
    let state = state?;
    let duskulls: Vec<u32> = targets
        .iter()
        .copied()
        .filter(|&t| my_slug_of(state, t) == Some("duskull"))
        .collect();
    if !duskulls.is_empty() {
        return Some(duskulls[rng.gen_range(0..duskulls.len())]);
    }
    (!targets.is_empty()).then(|| targets[rng.gen_range(0..targets.len())])
}

/// 相手の場 (バトル場 + ベンチ) から entity を引く。
fn opp_in_play(state: &StateDto, entity_id: u32) -> Option<&PokemonInPlayDto> {
    if let Some(a) = &state.opp.active {
        if a.entity_id == entity_id {
            return Some(a);
        }
    }
    state.opp.bench.iter().find(|b| b.entity_id == entity_id)
}

/// GOODS の使用優先度 (§5.3: なかよしポフィン → ポケパッド → ハイパーボール)。
const GOODS_PRIORITY: &[&str] = &["buddy-buddy-poffin", "poke-pad", "ultra-ball"];

/// トラッシュの「その他のグッズ」(原文 B-a-3 の明示カテゴリに無いグッズ。原文「無作為」)。
const TRASH_OTHER_GOODS: &[&str] = &[
    "buddy-buddy-poffin",
    "poke-pad",
    "special-red-card",
    "unfair-stamp",
];
/// このデッキのポケモン slug (トラッシュの「2枚以上被っているポケモン」判定用)。
const DECK_POKEMON: &[&str] = &[
    "dreepy",
    "drakloak",
    "dragapult-ex",
    "duskull",
    "dusclops",
    "dusknoir",
    "meowth-ex",
    "fezandipiti-ex",
    "budew",
];

/// 盤面情報が無いとき (prompt に state 同梱なし) のデッキ探索フォールバック優先度。
/// 末尾の基本エネはアカマツの基本エネサーチ、サポートはおくのてキャッチの「サポート1枚」サーチで拾う
/// (各カードはそれぞれのタイプの候補しか出さないので混線しない)。
const DEFAULT_FETCH_PRIORITY: &[&str] = &[
    "dreepy",
    "budew",
    "drakloak",
    "dusclops",
    "dusknoir",
    "duskull",
    "fire-energy",
    "psychic-energy",
    // おくのてキャッチ (サポート 1 枚サーチ) 用。リーリエ (ドロー源) 優先 (B-a-3)。
    "lillie-s-determination",
    "crispin",
    "boss-s-orders",
];

/// 直前に出した探索カード (`source`) に応じた、デッキ探索の対象優先度。
/// なかよしポフィンは番別のドラメシヤ目標 (B=2 / C・F=3) でたねを詰める。ポケパッド / ハイパーボール
/// は盤面+番駆動 (B-a-2/C-a-4)。アカマツ (基本エネ) / おくのてキャッチ (サポート) は候補が
/// そのタイプしか出ないので desired_fetch_priority の末尾で拾う。
fn search_priority(
    state: &StateDto,
    source: Option<&str>,
    rng: &mut ChaCha20Rng,
) -> Vec<&'static str> {
    match source {
        Some("buddy-buddy-poffin") => poffin_fetch_priority(state),
        Some("recon") => recon_priority(state, rng),
        // D 群の ニャースex 補充: おくのてキャッチで「ボスの指令」を取りにいく。
        Some("okunote-boss") => vec!["boss-s-orders", "lillie-s-determination", "crispin"],
        _ => desired_fetch_priority(state),
    }
}

/// ていさつしれいで「加える1枚」の優先 (C-a-10 / F-a-9)。自分の場のポケモン数 (≤2 / >2) と
/// 番 (C=2番目 / F=3番目以降) で別リスト。原文どおり。原文が順を定めないカテゴリ
/// (「たねポケモン」「その他サポート」) は無作為順。優先表に無いカードは pick_from_zone の
/// min 充足 (= 残り候補) に委ねる (原文「その他は無作為」相当)。
fn recon_priority(state: &StateDto, rng: &mut ChaCha20Rng) -> Vec<&'static str> {
    let field_pokemon = usize::from(state.me.active.is_some()) + state.me.bench.len();
    let is_f = my_turn_number(state) >= 3;
    // 「場に進化できるドラメシヤがいる場合に限りドロンチ/ドラパルトex」(近似: dreepy が場にいる)。
    let evolvable_dreepy = count_in_play(state, "dreepy") >= 1;
    let mut pri: Vec<&'static str> = Vec::new();
    if field_pokemon <= 2 {
        // F-a-9 ≤2 は先頭が「たねポケモン」(原文はたね内の順を定めない → 無作為)。
        if is_f {
            pri.extend(shuffle_strs(&["dreepy", "duskull", "budew"], rng));
        }
        pri.extend_from_slice(&["buddy-buddy-poffin", "poke-pad", "lillie-s-determination"]);
        if evolvable_dreepy {
            pri.push("drakloak");
            if is_f {
                pri.push("dragapult-ex");
            }
        }
        pri.push("unfair-stamp");
        if is_f {
            pri.push("special-red-card");
        }
        // その他サポート (原文は順を定めない → 無作為)。
        pri.extend(shuffle_strs(&["crispin", "boss-s-orders"], rng));
    } else if is_f {
        // F-a-9 >2: アカマツ＞ハイパーボール＞リーリエ＞ポケパッド＞ドロンチ＞ドラパルトex＞超＞炎。
        pri.extend_from_slice(&[
            "crispin",
            "ultra-ball",
            "lillie-s-determination",
            "poke-pad",
            "drakloak",
            "dragapult-ex",
            "psychic-energy",
            "fire-energy",
        ]);
    } else {
        // C-a-10 >2: リーリエ＞アカマツ＞ポケパッド＞ドロンチ＞ドラパルトex＞超＞炎。
        pri.extend_from_slice(&[
            "lillie-s-determination",
            "crispin",
            "poke-pad",
            "drakloak",
            "dragapult-ex",
            "psychic-energy",
            "fire-energy",
        ]);
    }
    pri
}

/// なかよしポフィンのたねサーチ優先度: 番別のドラメシヤ目標 (1番目=2 / 2番目以降=3) まで
/// ドラメシヤを詰め、スボミー不在ならスボミー、以降ヨマワル。原文 B-a-1/B-a-5/C-a-3/F-a-3。
fn poffin_fetch_priority(state: &StateDto) -> Vec<&'static str> {
    let target = if my_turn_number(state) >= 2 { 3 } else { 2 };
    let line = count_in_play(state, "dreepy")
        + count_in_play(state, "drakloak")
        + count_in_play(state, "dragapult-ex");
    let mut pri: Vec<&'static str> = Vec::new();
    if line < target {
        pri.push("dreepy");
    }
    if count_in_play(state, "budew") < 1 {
        pri.push("budew");
    }
    // 目標達成後はヨマワル1・スボミー1 を埋める (B-a-1: ドラメシヤ達成 → ヨマワル/スボミー)。
    // ドラメシヤは目標未達のとき (上の conditional) だけ先頭。fallback では最後。
    pri.extend_from_slice(&["duskull", "budew", "dreepy"]);
    pri
}

/// 盤面 + 番 + 手札から「今サーチで一番欲しいカード」の優先順を導く。
/// 原文 B-a-2 / C-a-4 / F-a-4 (ポケパッド) の再帰パターン:
/// ①ドラパルトライン (dreepy/drakloak/dragapult-ex) < 2 → ドラメシヤ。
/// ②ライン≥2 かつスボミー不在 → スボミー (むずむずかふん用)。
/// ③ライン≥2 + スボミー有: 2番目以降(C/F)で手札ドロンチ≥2 なら進化先 (場にヨマワル→サマヨール、
///   いなければヨマワル) を先に、それ以外はドロンチ。以降は進化系→ヨマワル→エネ→サポートの fallback。
fn desired_fetch_priority(state: &StateDto) -> Vec<&'static str> {
    let line = count_in_play(state, "dreepy")
        + count_in_play(state, "drakloak")
        + count_in_play(state, "dragapult-ex");
    let has_budew = count_in_play(state, "budew") >= 1;
    let mut pri: Vec<&'static str> = Vec::new();
    // F-a-11: 炎+超 付き drakloak がいて場に dragapult-ex がいないなら、ハイパーボールで
    // dragapult-ex を取りにいく (進化させてファントムダイブ可能にするため)。poke-pad は
    // ルール持ちを出さないので候補に dragapult-ex が無く影響しない。
    if has_charged_drakloak(state) && count_in_play(state, "dragapult-ex") == 0 {
        pri.push("dragapult-ex");
    }
    if line < 2 {
        pri.push("dreepy");
    }
    if !has_budew {
        pri.push("budew");
    }
    // C-a-4 / F-a-4: 2番目以降の手番で手札にドロンチが2枚以上あるなら、ドロンチより先に進化先を取る
    // (場にヨマワルがいればサマヨール、いなければヨマワル)。1番目(B-a-2)はこの分岐が無い。
    if my_turn_number(state) >= 2 && count_in_hand(state, "drakloak") >= 2 {
        if count_in_play(state, "duskull") >= 1 {
            pri.push("dusclops");
        } else {
            pri.push("duskull");
        }
    }
    pri.extend_from_slice(&[
        "drakloak",
        "dusclops",
        "dusknoir",
        "duskull",
        "dreepy",
        "budew",
        // アカマツ等の基本エネサーチで拾う (ポケモン候補が尽きた末尾)。
        "fire-energy",
        "psychic-energy",
        // おくのてキャッチのサポート 1 枚サーチ用 (リーリエ優先、B-a-3)。
        "lillie-s-determination",
        "crispin",
        "boss-s-orders",
    ]);
    pri
}

/// 合法手から `GOODS_PRIORITY` 順に最初に使えるグッズの `PlayCard` を返す。
/// グッズロック中は engine が legal_actions から除外するので、ここに来ない。
fn find_goods_play(state: &StateDto, legal: &[ActionDto]) -> Option<ActionDto> {
    for want in GOODS_PRIORITY {
        // B-a-3: 手札にドロンチがあるならハイパーボールは使わない (温存)。
        if *want == "ultra-ball" && count_in_hand(state, "drakloak") >= 1 {
            continue;
        }
        if let Some(a) = legal
            .iter()
            .find(|a| play_card_slug(state, a) == Some(*want))
        {
            return Some(a.clone());
        }
    }
    None
}

/// さかてにとる (§F-a-10): 場の fezandipiti-ex の起動特性 (UseAbility) を使う。fezandipiti-ex の
/// 起動特性はさかてにとるのみで、engine が「前番に自ポケ KO」条件でゲートするので legal なら使う
/// (3 ドロー)。
fn find_take_advantage(state: &StateDto, legal: &[ActionDto]) -> Option<ActionDto> {
    legal
        .iter()
        .find(|a| {
            matches!(a,
                ActionDto::UseAbility { entity_id, .. }
                    if my_slug_of(state, entity_id.0) == Some("fezandipiti-ex"))
        })
        .cloned()
}

/// さかてにとる用の補充 (§F-a-10): 前の番に自ポケがきぜつしており、場に fezandipiti-ex が
/// いないとき、手札の fezandipiti-ex をベンチに出す (legal = ベンチ空き有)。次の手番で
/// find_take_advantage が さかてにとるを使う。KO していない番は ex を場に晒さない。
fn find_fezandipiti_setup(state: &StateDto, legal: &[ActionDto]) -> Option<ActionDto> {
    if !state.me.had_ko_last_turn || count_in_play(state, "fezandipiti-ex") >= 1 {
        return None;
    }
    legal
        .iter()
        .find(|a| play_card_slug(state, a) == Some("fezandipiti-ex"))
        .cloned()
}

/// 手札妨害カードの能動使用:
/// - アンフェアスタンプ (F-a-3-①): 前の番に自ポケがきぜつ + 相手の手札が 4 枚以上 → 使う。
/// - スペシャルレッドカード (F-a-13 いいえ): ファントムダイブが使えず相手サイド残り ≤3
///   かつ**この番アンフェアスタンプ未使用** → 使う。
fn find_disruption(
    state: &StateDto,
    legal: &[ActionDto],
    unfair_used_this_turn: bool,
) -> Option<ActionDto> {
    if state.me.had_ko_last_turn && state.opp.hand.len() >= 4 {
        if let Some(a) = legal
            .iter()
            .find(|a| play_card_slug(state, a) == Some("unfair-stamp"))
        {
            return Some(a.clone());
        }
    }
    if !has_phantom_ready_dragapult(state) && state.opp.prizes.len() <= 3 && !unfair_used_this_turn
    {
        if let Some(a) = legal
            .iter()
            .find(|a| play_card_slug(state, a) == Some("special-red-card"))
        {
            return Some(a.clone());
        }
    }
    None
}

/// 退避 (§5.8): バトル場がアタッカー (dragapult-ex / budew) でないとき、ファントムダイブ可能な
/// dragapult-ex (炎+超) > budew をバトル場に出すためにげる Retreat を返す。該当ベンチ index への
/// にげが legal (にげエネ充足) なときのみ。engine が energy_to_discard を pre-pick 済みの Retreat を
/// そのまま採用する。
fn find_retreat(state: &StateDto, legal: &[ActionDto]) -> Option<ActionDto> {
    let active_slug = state.me.active.as_ref()?.card.as_deref()?;
    // 既にアタッカー (dragapult-ex) がバトル場 → にげ不要 (find_attack がファントムダイブを処理)。
    if active_slug == "dragapult-ex" {
        return None;
    }
    // 撃てる (炎+超) dragapult-ex がベンチにいるか。
    let ready_dragapult = bench_index_where(state, |p| {
        p.card.as_deref() == Some("dragapult-ex")
            && mon_has_energy(p, "fire-energy")
            && mon_has_energy(p, "psychic-energy")
    });
    // にげ先決定 (原文 F-a-13/14/15)。
    // - バトル場が budew: ファントムダイブが使える (= 撃てる dragapult-ex がベンチにいる) ときだけ
    //   にげて繰り出す (F-a-14)。無ければ budew の むずむずかふんで chip するのでにげない (F-a-15)。
    //   ※旧実装は budew を無条件にげ不要にしていたが、撃てる dragapult-ex がいても繰り出さず
    //     budew チップで居座る誤り (F-a-14 違反) だった。
    // - それ以外がバトル場 (退避 §5.8): 撃てる dragapult-ex → budew の順で繰り出す。
    let target_index = if active_slug == "budew" {
        ready_dragapult?
    } else {
        ready_dragapult
            .or_else(|| bench_index_where(state, |p| p.card.as_deref() == Some("budew")))?
    };
    // 該当 index へのにげが合法 (にげエネ充足) なら採用。
    legal
        .iter()
        .find(|a| {
            matches!(a, ActionDto::Retreat { to_bench_index, .. } if *to_bench_index == target_index)
        })
        .cloned()
}

/// 述語に一致する最初のベンチ entity の index を返す。
fn bench_index_where<F: Fn(&PokemonInPlayDto) -> bool>(state: &StateDto, pred: F) -> Option<u8> {
    state
        .me
        .bench
        .iter()
        .position(pred)
        .and_then(|i| u8::try_from(i).ok())
}

/// サポート使用 (§5.5): アカマツ (場のエネ総数 < 3 でエネ加速) を優先し、無ければ
/// リーリエの決心 (ドロー)。ただしリーリエは盤面が完成 (炎+超 付き dragapult-ex + drakloak 2匹)
/// していれば手札温存のため使わない (F-a-8)。ボスの指令は判定木スライスで別途。
fn find_supporter(state: &StateDto, legal: &[ActionDto]) -> Option<ActionDto> {
    // ボスの指令は find_boss_ko (choose_action 側) で先に処理する。ここはアカマツ/リーリエのみ。
    // アカマツ: 番別 — 1番目(B)は使わない / 2番目(C-a-8)は無条件 / 3番目以降(F-a-7)は場のエネ総数<3 のみ。
    let use_crispin = match my_turn_number(state) {
        1 => false,
        2 => true,
        _ => field_energy_total(state) < 3,
    };
    if use_crispin {
        if let Some(a) = legal
            .iter()
            .find(|a| play_card_slug(state, a) == Some("crispin"))
        {
            return Some(a.clone());
        }
    }
    // リーリエの決心: 盤面完成時 (F-a-8) は温存、それ以外はドロー源として使う。
    if !board_is_phantom_ready(state) {
        if let Some(a) = legal
            .iter()
            .find(|a| play_card_slug(state, a) == Some("lillie-s-determination"))
        {
            return Some(a.clone());
        }
    }
    None
}

/// 自分の場 (バトル場 + ベンチ) に付いている基本エネルギーの総数。
fn field_energy_total(state: &StateDto) -> usize {
    let active = state
        .me
        .active
        .as_ref()
        .map_or(0, |p| p.energy_attached.len());
    let bench: usize = state.me.bench.iter().map(|p| p.energy_attached.len()).sum();
    active + bench
}

/// 炎+超 両方が付いた drakloak (= 進化すれば即ファントムダイブ可能) の `ActionTarget` 群。
fn charged_drakloak_targets(state: &StateDto) -> Vec<ActionTarget> {
    let mut out = Vec::new();
    for (i, b) in state.me.bench.iter().enumerate() {
        if b.card.as_deref() == Some("drakloak")
            && mon_has_energy(b, "fire-energy")
            && mon_has_energy(b, "psychic-energy")
        {
            out.push(ActionTarget::OwnBench {
                index: u8::try_from(i).unwrap_or(u8::MAX),
            });
        }
    }
    if let Some(a) = &state.me.active {
        if a.card.as_deref() == Some("drakloak")
            && mon_has_energy(a, "fire-energy")
            && mon_has_energy(a, "psychic-energy")
        {
            out.push(ActionTarget::OwnActive);
        }
    }
    out
}

/// 炎+超 が付いた drakloak が場にいるか (アタッカー準備サーチ判定用)。
fn has_charged_drakloak(state: &StateDto) -> bool {
    !charged_drakloak_targets(state).is_empty()
}

/// アタッカー準備 (F-a-11/F-a-12): 場に dragapult-ex が 1 匹もいないとき、炎+超 が付いた drakloak を
/// dragapult-ex に進化させる `PlayCard` (手札の dragapult-ex で当該 drakloak を target) を返す。
fn find_attacker_evolution(state: &StateDto, legal: &[ActionDto]) -> Option<ActionDto> {
    if count_in_play(state, "dragapult-ex") >= 1 {
        return None;
    }
    for t in charged_drakloak_targets(state) {
        if let Some(a) = legal.iter().find(|a| {
            matches!(a,
                ActionDto::PlayCard { entity_id, target: Some(tt) }
                    if *tt == t && my_slug_of(state, entity_id.0) == Some("dragapult-ex"))
        }) {
            return Some(a.clone());
        }
    }
    None
}

/// 炎+超 両方が付いた dragapult-ex (= ファントムダイブ可能) が自分の場にいるか。
fn has_phantom_ready_dragapult(state: &StateDto) -> bool {
    state
        .me
        .active
        .iter()
        .chain(state.me.bench.iter())
        .any(|p| {
            p.card.as_deref() == Some("dragapult-ex")
                && mon_has_energy(p, "fire-energy")
                && mon_has_energy(p, "psychic-energy")
        })
}

/// F-a-8 の温存条件の近似: ファントムダイブ可能な dragapult-ex が場にいて、かつ drakloak が 2 匹以上。
fn board_is_phantom_ready(state: &StateDto) -> bool {
    has_phantom_ready_dragapult(state) && count_in_play(state, "drakloak") >= 2
}

/// エネ付与先の優先度 (§5.4: ドロンチ > ドラパルトex > ドラメシヤ)。budew 等は対象外。
const ENERGY_TARGET_PRIORITY: &[&str] = &["drakloak", "dragapult-ex", "dreepy"];

/// エネ付与 (§5.4): `ENERGY_TARGET_PRIORITY` 順 (各 slug 内はベンチ優先) に、まだ炎+超が
/// 揃っていない最初のポケモンへ不足タイプを 1 枚付ける。炎炎/超超は作らない (= 不足タイプのみ
/// 候補)。炎+超 充足済みは飛ばす (超過分は付けない)。type 順は原文既定の炎＞超だが、2番目の番
/// (C-a-7) でバトル場が budew でないときは超＞炎。
fn find_energy_attach(state: &StateDto, legal: &[ActionDto]) -> Option<ActionDto> {
    // C-a-7: 2番目の番でバトル場が budew でないときは超優先 (それ以外は炎優先)。
    let active_is_budew = state
        .me
        .active
        .as_ref()
        .is_some_and(|p| p.card.as_deref() == Some("budew"));
    let psychic_first = my_turn_number(state) == 2 && !active_is_budew;
    let both: &[&str] = if psychic_first {
        &["psychic-energy", "fire-energy"]
    } else {
        &["fire-energy", "psychic-energy"]
    };
    for (target, mon) in energy_targets_by_priority(state) {
        let has_fire = mon_has_energy(mon, "fire-energy");
        let has_psychic = mon_has_energy(mon, "psychic-energy");
        let wanted: &[&str] = match (has_fire, has_psychic) {
            (false, false) => both,
            (false, true) => &["fire-energy"],
            (true, false) => &["psychic-energy"],
            (true, true) => continue, // 炎+超 充足 → 超過は付けない
        };
        for w in wanted {
            if let Some(a) = legal
                .iter()
                .find(|a| is_energy_attach_to(state, a, w, target))
            {
                return Some(a.clone());
            }
        }
    }
    None
}

/// エネ付与候補 (ドラパルトライン) を優先度順に列挙。各 slug 内はベンチ優先 (§GAP-4)。
fn energy_targets_by_priority(state: &StateDto) -> Vec<(ActionTarget, &PokemonInPlayDto)> {
    let mut out = Vec::new();
    for slug in ENERGY_TARGET_PRIORITY {
        for (i, b) in state.me.bench.iter().enumerate() {
            if b.card.as_deref() == Some(*slug) {
                let index = u8::try_from(i).unwrap_or(u8::MAX);
                out.push((ActionTarget::OwnBench { index }, b));
            }
        }
        if let Some(a) = &state.me.active {
            if a.card.as_deref() == Some(*slug) {
                out.push((ActionTarget::OwnActive, a));
            }
        }
    }
    out
}

/// ポケモンに指定 slug のエネルギーが付いているか。
fn mon_has_energy(mon: &PokemonInPlayDto, slug: &str) -> bool {
    mon.energy_attached
        .iter()
        .any(|e| e.card.as_deref() == Some(slug))
}

/// アクションが「`energy_slug` のエネを `target` に付ける PlayCard」か。
fn is_energy_attach_to(
    state: &StateDto,
    a: &ActionDto,
    energy_slug: &str,
    target: ActionTarget,
) -> bool {
    matches!(
        a,
        ActionDto::PlayCard { entity_id, target: Some(t) }
            if *t == target && my_slug_of(state, entity_id.0) == Some(energy_slug)
    )
}

/// `options` から `slugs` に一致する候補の entity_id を (options 出現順で) 集める。
fn trash_ids_of(options: &[EntityDto], slugs: &[&str]) -> Vec<u32> {
    options
        .iter()
        .filter(|o| o.card.as_deref().is_some_and(|s| slugs.contains(&s)))
        .map(|o| o.entity_id)
        .collect()
}

/// `ids` を無作為に並べ替えて返す (原文が優先を定めない tier 用)。
fn trash_shuffle(ids: &[u32], rng: &mut ChaCha20Rng) -> Vec<u32> {
    let n = u8::try_from(ids.len()).unwrap_or(u8::MAX);
    super::pick_random_subset(rng, ids, n, n)
}

/// `chosen` に `ids` を `count` 件まで (重複なしで) 追加する。
fn trash_push(chosen: &mut Vec<u32>, ids: &[u32], count: usize) {
    for &id in ids {
        if chosen.len() >= count {
            break;
        }
        if !chosen.contains(&id) {
            chosen.push(id);
        }
    }
}

/// 手札トラッシュ cost (ハイパーボール) で捨てる `count` 枚を、原文 B-a-3 の順で選ぶ:
/// 夜のタンカ → スタジアム → 2枚以上被っているポケ(ドロンチ除く)1枚 → ハイパーボール →
/// ふしぎなアメ → その他グッズ → ボス → アカマツ → リーリエ → エネ。`drakloak` は最後まで温存。
/// スタジアム複数 / その他グッズ / エネ 炎超 / fallback など**原文が優先を定めない tier は無作為**。
fn pick_trash_targets(options: &[EntityDto], count: usize, rng: &mut ChaCha20Rng) -> Vec<u32> {
    let mut chosen: Vec<u32> = Vec::new();
    trash_push(
        &mut chosen,
        &trash_ids_of(options, &["night-stretcher"]),
        count,
    );
    trash_push(
        &mut chosen,
        &trash_shuffle(
            &trash_ids_of(options, &["jamming-tower", "team-rocket-s-watchtower"]),
            rng,
        ),
        count,
    );
    // 2枚以上被っているポケモン (drakloak 除く) を 1 枚だけ (どれかは無作為)。
    if chosen.len() < count {
        let dups: Vec<u32> = options
            .iter()
            .filter(|o| {
                o.card.as_deref().is_some_and(|s| {
                    s != "drakloak"
                        && DECK_POKEMON.contains(&s)
                        && options
                            .iter()
                            .filter(|x| x.card.as_deref() == Some(s))
                            .count()
                            >= 2
                })
            })
            .map(|o| o.entity_id)
            .collect();
        if let Some(&one) = trash_shuffle(&dups, rng).first() {
            trash_push(&mut chosen, &[one], count);
        }
    }
    trash_push(&mut chosen, &trash_ids_of(options, &["ultra-ball"]), count);
    trash_push(&mut chosen, &trash_ids_of(options, &["rare-candy"]), count);
    trash_push(
        &mut chosen,
        &trash_shuffle(&trash_ids_of(options, TRASH_OTHER_GOODS), rng),
        count,
    );
    trash_push(
        &mut chosen,
        &trash_ids_of(options, &["boss-s-orders"]),
        count,
    );
    trash_push(&mut chosen, &trash_ids_of(options, &["crispin"]), count);
    trash_push(
        &mut chosen,
        &trash_ids_of(options, &["lillie-s-determination"]),
        count,
    );
    trash_push(
        &mut chosen,
        &trash_shuffle(
            &trash_ids_of(options, &["fire-energy", "psychic-energy"]),
            rng,
        ),
        count,
    );
    // fallback: drakloak 以外の残り (無作為) → 最終手段で drakloak も (cost を払えないと使えない)。
    if chosen.len() < count {
        let rest: Vec<u32> = options
            .iter()
            .filter(|o| o.card.as_deref() != Some("drakloak") && !chosen.contains(&o.entity_id))
            .map(|o| o.entity_id)
            .collect();
        trash_push(&mut chosen, &trash_shuffle(&rest, rng), count);
    }
    if chosen.len() < count {
        let all: Vec<u32> = options.iter().map(|o| o.entity_id).collect();
        trash_push(&mut chosen, &all, count);
    }
    chosen
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bots::testutil::{
        dummy_prompt, dummy_request, in_play, player_with_hand, prompt_with_state, request_with,
        rng, state_with,
    };
    use crate::wire::action::EntityId;
    use crate::wire::state::PlayerView;

    #[test]
    fn chooses_to_go_first() {
        // G6: コイン勝者なら先攻 (yes=true)。竹内版と逆。
        let mut bot = DragapultYopifuttoBot::new(CardFacts::new());
        let mut r = rng();
        let choice = bot.choose_prompt(&dummy_prompt(PromptDto::ChooseFirstOrSecond), &mut r);
        assert_eq!(choice.yes, Some(true));
    }

    #[test]
    fn places_nothing_on_bench_at_setup() {
        // §4 S4: 原文の対戦準備にベンチ初期配置の手順は無い → 0 枚 (何も置かない)。
        let mut bot = DragapultYopifuttoBot::new(CardFacts::new());
        let mut r = rng();
        let p = dummy_prompt(PromptDto::PlaceInitialBench {
            eligible: vec![10, 11, 12],
            bench_max: 5,
        });
        let choice = bot.choose_prompt(&p, &mut r);
        assert!(
            choice.selected.is_empty(),
            "セットアップではベンチに置かない (原文に手順なし)"
        );
    }

    #[test]
    fn first_turn_active_prefers_duskull() {
        // 先攻 (A-c-1): ヨマワル(duskull) 最優先。手札順が dreepy→budew→duskull でも duskull。
        let mut bot = DragapultYopifuttoBot::new(CardFacts::new());
        let mut r = rng();
        let me = player_with_hand(&[(10, "dreepy"), (11, "budew"), (12, "duskull")]);
        let p = prompt_with_state(
            PromptDto::ChooseInitialActive {
                eligible: vec![10, 11, 12],
            },
            state_with("me", me),
        );
        let choice = bot.choose_prompt(&p, &mut r);
        assert_eq!(choice.selected, vec![12], "先攻はヨマワル(entity 12)");
    }

    #[test]
    fn second_turn_active_prefers_budew() {
        // 後攻 (A-c-2): スボミー(budew) 最優先。
        let mut bot = DragapultYopifuttoBot::new(CardFacts::new());
        let mut r = rng();
        let me = player_with_hand(&[(20, "dreepy"), (21, "budew"), (22, "duskull")]);
        let p = prompt_with_state(
            PromptDto::ChooseInitialActive {
                eligible: vec![20, 21, 22],
            },
            state_with("opp", me),
        );
        let choice = bot.choose_prompt(&p, &mut r);
        assert_eq!(choice.selected, vec![21], "後攻はスボミー(entity 21)");
    }

    #[test]
    fn first_turn_falls_through_to_dreepy_without_duskull() {
        // 先攻でヨマワルが無ければドラメシヤ(dreepy)。
        let mut bot = DragapultYopifuttoBot::new(CardFacts::new());
        let mut r = rng();
        let me = player_with_hand(&[(30, "budew"), (31, "dreepy"), (32, "meowth-ex")]);
        let p = prompt_with_state(
            PromptDto::ChooseInitialActive {
                eligible: vec![30, 31, 32],
            },
            state_with("me", me),
        );
        let choice = bot.choose_prompt(&p, &mut r);
        assert_eq!(
            choice.selected,
            vec![31],
            "ヨマワル不在なら dreepy(entity 31)"
        );
    }

    #[test]
    fn uses_recon_directive_above_all() {
        // ベンチに drakloak。ていさつしれい (UseAbility) が EndTurn より優先される。
        let mut bot = DragapultYopifuttoBot::new(CardFacts::new());
        let mut r = rng();
        let mut me = PlayerView {
            bench: vec![in_play(7, "drakloak")],
            ..crate::bots::testutil::empty_player()
        };
        me.active = Some(in_play(1, "dragapult-ex"));
        let legal = vec![
            ActionDto::EndTurn,
            ActionDto::UseAbility {
                entity_id: EntityId(7),
                ability_index: 0,
            },
        ];
        let req = request_with(state_with("me", me), legal);
        let a = bot.choose_action(&req, &mut r).expect("action");
        assert!(
            matches!(a, ActionDto::UseAbility { entity_id, .. } if entity_id == EntityId(7)),
            "drakloak のていさつしれいを最優先で使う (got {a:?})"
        );
    }

    /// hand に進化カード (entity, slug) を持ち、active/bench を構成した state を作る。
    fn state_with_active_and_hand(
        active: (u32, &str),
        bench_extra: Vec<(u32, &str)>,
        hand: &[(u32, &str)],
    ) -> crate::wire::state::StateDto {
        let mut me = player_with_hand(hand);
        me.active = Some(in_play(active.0, active.1));
        me.bench = bench_extra
            .into_iter()
            .map(|(id, slug)| in_play(id, slug))
            .collect();
        state_with("me", me)
    }

    #[test]
    fn evolves_toward_dragapult_line() {
        // ベンチに dreepy、手札に drakloak (進化カード)。PlayCard(drakloak) を選ぶ。
        let mut bot = DragapultYopifuttoBot::new(CardFacts::new());
        let mut r = rng();
        let state =
            state_with_active_and_hand((1, "budew"), vec![(2, "dreepy")], &[(50, "drakloak")]);
        let legal = vec![
            ActionDto::EndTurn,
            ActionDto::PlayCard {
                entity_id: EntityId(50),
                target: None,
            },
        ];
        let req = request_with(state, legal);
        let a = bot.choose_action(&req, &mut r).expect("action");
        assert!(matches!(a, ActionDto::PlayCard { entity_id, .. } if entity_id == EntityId(50)));
    }

    #[test]
    fn blanket_evolution_excludes_dragapult_ex() {
        // 原文 F-a-5: 通常進化はサマヨール/ヨノワール/ドロンチのみ。drakloak→dragapult-ex は
        // 通常進化に含めない (F-a-11/F-a-12 のアタッカー準備パス専用)。手札の dragapult-ex への
        // 進化 (PlayCard) はこのスライスでは選ばず EndTurn。
        let mut bot = DragapultYopifuttoBot::new(CardFacts::new());
        let mut r = rng();
        let state = state_with_active_and_hand(
            (1, "drakloak"),
            vec![(2, "drakloak")],
            &[(60, "dragapult-ex")],
        );
        let legal = vec![
            ActionDto::EndTurn,
            ActionDto::PlayCard {
                entity_id: EntityId(60),
                target: None,
            },
        ];
        let req = request_with(state, legal);
        let a = bot.choose_action(&req, &mut r).expect("action");
        assert_eq!(a, ActionDto::EndTurn);
    }

    #[test]
    fn rare_candy_duskull_to_dusknoir_when_dusknoir_in_hand() {
        // C-a-6/F-a-5: 手札に rare-candy+dusknoir & 場に duskull → rare-candy を出し、
        // 対象 (ChooseTargetPokemon) は duskull (dusclops 飛ばし)。通常進化より優先。
        let mut bot = DragapultYopifuttoBot::new(CardFacts::new());
        let mut r = rng();
        let state = state_with_active_and_hand(
            (1, "drakloak"),
            vec![(2, "duskull")],
            &[(70, "rare-candy"), (71, "dusknoir")],
        );
        let legal = vec![
            ActionDto::EndTurn,
            ActionDto::PlayCard {
                entity_id: EntityId(70),
                target: None,
            },
        ];
        let a = bot
            .choose_action(&request_with(state, legal), &mut r)
            .expect("action");
        assert!(
            matches!(a, ActionDto::PlayCard { entity_id, .. } if entity_id == EntityId(70)),
            "rare-candy を出す (got {a:?})"
        );
        assert!(bot.pending_rare_candy);
        // 対象選択: duskull(2)。
        let st =
            state_with_active_and_hand((1, "drakloak"), vec![(2, "duskull")], &[(71, "dusknoir")]);
        let p = prompt_with_state(PromptDto::ChooseTargetPokemon { targets: vec![2] }, st);
        let c = bot.choose_prompt(&p, &mut r);
        assert_eq!(c.selected, vec![2], "duskull を進化対象に");
    }

    #[test]
    fn no_rare_candy_without_dusknoir_in_hand() {
        // dusknoir が手札に無ければ rare-candy は出さない (通常進化 / EndTurn)。
        let mut bot = DragapultYopifuttoBot::new(CardFacts::new());
        let mut r = rng();
        let state = state_with_active_and_hand(
            (1, "drakloak"),
            vec![(2, "duskull")],
            &[(70, "rare-candy")],
        );
        let legal = vec![
            ActionDto::EndTurn,
            ActionDto::PlayCard {
                entity_id: EntityId(70),
                target: None,
            },
        ];
        let a = bot
            .choose_action(&request_with(state, legal), &mut r)
            .expect("action");
        assert_eq!(a, ActionDto::EndTurn, "dusknoir 無 → rare-candy 不使用");
    }

    #[test]
    fn places_basic_dreepy_to_bench() {
        let mut bot = DragapultYopifuttoBot::new(CardFacts::new());
        let mut r = rng();
        let me = player_with_hand(&[(70, "dreepy")]);
        let state = state_with("me", me);
        let legal = vec![
            ActionDto::EndTurn,
            ActionDto::PlayCard {
                entity_id: EntityId(70),
                target: None,
            },
        ];
        let req = request_with(state, legal);
        let a = bot.choose_action(&req, &mut r).expect("action");
        assert!(matches!(a, ActionDto::PlayCard { entity_id, .. } if entity_id == EntityId(70)));
    }

    #[test]
    fn plays_buddy_buddy_poffin_when_legal() {
        // 手札に なかよしポフィン。たね展開グッズとして使う (§5.3)。
        let mut bot = DragapultYopifuttoBot::new(CardFacts::new());
        let mut r = rng();
        let mut me = player_with_hand(&[(80, "buddy-buddy-poffin")]);
        me.active = Some(in_play(1, "budew"));
        let legal = vec![
            ActionDto::EndTurn,
            ActionDto::PlayCard {
                entity_id: EntityId(80),
                target: None,
            },
        ];
        let req = request_with(state_with("me", me), legal);
        let a = bot.choose_action(&req, &mut r).expect("action");
        assert!(
            matches!(a, ActionDto::PlayCard { entity_id, .. } if entity_id == EntityId(80)),
            "なかよしポフィンを使う (got {a:?})"
        );
    }

    #[test]
    fn poffin_search_picks_dreepy_first() {
        // ポフィン探索候補 = duskull / dreepy / dreepy / budew。max=2 → dreepy 2匹を優先。
        let mut bot = DragapultYopifuttoBot::new(CardFacts::new());
        let mut r = rng();
        let mk = |id: u32, slug: &str| EntityDto {
            entity_id: id,
            card: Some(slug.to_string()),
        };
        let options = vec![
            mk(10, "duskull"),
            mk(11, "dreepy"),
            mk(12, "dreepy"),
            mk(13, "budew"),
        ];
        let mut p = dummy_prompt(PromptDto::ChooseFromZone {
            zone: "deck".to_string(),
            options,
        });
        p.min = 0;
        p.max = 2;
        let c = bot.choose_prompt(&p, &mut r);
        assert_eq!(c.selected, vec![11, 12], "dreepy(11,12) を優先選択");
    }

    #[test]
    fn plays_poke_pad_when_legal() {
        // 手札に ポケパッド (ポフィン無し)。GOODS としてポケパッドを使う (§5.3)。
        let mut bot = DragapultYopifuttoBot::new(CardFacts::new());
        let mut r = rng();
        let mut me = player_with_hand(&[(81, "poke-pad")]);
        me.active = Some(in_play(1, "budew"));
        let legal = vec![
            ActionDto::EndTurn,
            ActionDto::PlayCard {
                entity_id: EntityId(81),
                target: None,
            },
        ];
        let req = request_with(state_with("me", me), legal);
        let a = bot.choose_action(&req, &mut r).expect("action");
        assert!(
            matches!(a, ActionDto::PlayCard { entity_id, .. } if entity_id == EntityId(81)),
            "ポケパッドを使う (got {a:?})"
        );
    }

    #[test]
    fn poke_pad_search_picks_drakloak_when_line_ready() {
        // 盤面: バトル場 dragapult-ex + ベンチ dreepy + budew (ライン2・スボミー有)。
        // → 探索優先は drakloak 先頭。候補 [dreepy, drakloak, budew] から drakloak を選ぶ。
        let mut bot = DragapultYopifuttoBot::new(CardFacts::new());
        let mut r = rng();
        let mut me = crate::bots::testutil::empty_player();
        me.active = Some(in_play(1, "dragapult-ex"));
        me.bench = vec![in_play(2, "dreepy"), in_play(3, "budew")];
        let mk = |id: u32, slug: &str| EntityDto {
            entity_id: id,
            card: Some(slug.to_string()),
        };
        let options = vec![mk(20, "dreepy"), mk(21, "drakloak"), mk(22, "budew")];
        let mut p = prompt_with_state(
            PromptDto::ChooseFromZone {
                zone: "deck".to_string(),
                options,
            },
            state_with("me", me),
        );
        p.min = 1;
        p.max = 1;
        let c = bot.choose_prompt(&p, &mut r);
        assert_eq!(c.selected, vec![21], "ライン充足 + スボミー有 → drakloak");
    }

    #[test]
    fn ultra_ball_trash_picks_low_value_and_spares_drakloak() {
        // ハイパーボールのトラッシュ cost: 手札 = 夜のタンカ / 炎エネ / ドロンチ / ドラメシヤ。
        // 2枚捨てる → 夜のタンカ + 炎エネ (drakloak は温存)。
        let mut bot = DragapultYopifuttoBot::new(CardFacts::new());
        let mut r = rng();
        let me = player_with_hand(&[
            (40, "night-stretcher"),
            (41, "fire-energy"),
            (42, "drakloak"),
            (43, "dreepy"),
        ]);
        let mk = |id: u32, slug: &str| EntityDto {
            entity_id: id,
            card: Some(slug.to_string()),
        };
        // options = 手札と同じ entity 群 (= トラッシュ cost と判別される)。
        let options = vec![
            mk(40, "night-stretcher"),
            mk(41, "fire-energy"),
            mk(42, "drakloak"),
            mk(43, "dreepy"),
        ];
        let mut p = prompt_with_state(
            PromptDto::ChooseFromZone {
                zone: "deck".to_string(),
                options,
            },
            state_with("me", me),
        );
        p.min = 2;
        p.max = 2;
        let c = bot.choose_prompt(&p, &mut r);
        assert_eq!(
            c.selected,
            vec![40, 41],
            "夜のタンカ + 炎エネを捨て drakloak は温存"
        );
    }

    #[test]
    fn trash_dup_pokemon_before_other_goods() {
        // 手札: dreepy×2 (被り) + なかよしポフィン (その他グッズ) + drakloak。count=1。
        // 原文順では「被りポケ1枚」が「その他グッズ」より先 → dreepy のどれかを捨てる。
        let mk = |id: u32, slug: &str| EntityDto {
            entity_id: id,
            card: Some(slug.to_string()),
        };
        let options = vec![
            mk(10, "dreepy"),
            mk(11, "dreepy"),
            mk(12, "buddy-buddy-poffin"),
            mk(13, "drakloak"),
        ];
        let mut r = rng();
        let sel = pick_trash_targets(&options, 1, &mut r);
        assert_eq!(sel.len(), 1);
        assert!(
            [10, 11].contains(&sel[0]),
            "被りポケ (dreepy) をその他グッズより先に捨てる (got {sel:?})"
        );
    }

    #[test]
    fn skips_ultra_ball_when_drakloak_in_hand() {
        // 手札に ドロンチ + ハイパーボール (+捨て札)。B-a-3: ドロンチがあるならボールは温存。
        let mut bot = DragapultYopifuttoBot::new(CardFacts::new());
        let mut r = rng();
        let mut me = player_with_hand(&[
            (50, "ultra-ball"),
            (51, "drakloak"),
            (52, "fire-energy"),
            (53, "psychic-energy"),
        ]);
        me.active = Some(in_play(1, "budew"));
        let legal = vec![
            ActionDto::EndTurn,
            ActionDto::PlayCard {
                entity_id: EntityId(50),
                target: None,
            },
        ];
        let req = request_with(state_with("me", me), legal);
        let a = bot.choose_action(&req, &mut r).expect("action");
        assert_eq!(
            a,
            ActionDto::EndTurn,
            "ドロンチ手札時はボール温存 → EndTurn"
        );
    }

    /// 装着エネ付きの PokemonInPlayDto を作る。
    fn in_play_with_energy(entity_id: u32, slug: &str, energy: &[&str]) -> PokemonInPlayDto {
        let mut p = in_play(entity_id, slug);
        p.energy_attached = energy
            .iter()
            .enumerate()
            .map(|(i, s)| EntityDto {
                entity_id: 900 + u32::try_from(i).unwrap_or(0),
                card: Some((*s).to_string()),
            })
            .collect();
        p
    }

    #[test]
    fn attaches_fire_first_to_bench_drakloak() {
        // ベンチ drakloak (エネなし)・バトル場 budew。炎優先で炎をベンチ drakloak に付ける。
        let mut bot = DragapultYopifuttoBot::new(CardFacts::new());
        let mut r = rng();
        let mut me = player_with_hand(&[(200, "fire-energy"), (201, "psychic-energy")]);
        me.active = Some(in_play(1, "budew"));
        me.bench = vec![in_play(2, "drakloak")];
        let legal = vec![
            ActionDto::EndTurn,
            ActionDto::PlayCard {
                entity_id: EntityId(201),
                target: Some(ActionTarget::OwnBench { index: 0 }),
            },
            ActionDto::PlayCard {
                entity_id: EntityId(200),
                target: Some(ActionTarget::OwnBench { index: 0 }),
            },
        ];
        let a = bot
            .choose_action(&request_with(state_with("me", me), legal), &mut r)
            .expect("action");
        assert!(
            matches!(a, ActionDto::PlayCard { entity_id, .. } if entity_id == EntityId(200)),
            "炎エネを優先して付ける (got {a:?})"
        );
    }

    #[test]
    fn avoids_second_fire_overflow_on_drakloak() {
        // ベンチ drakloak に既に炎。炎炎を避け 超 を付ける。
        let mut bot = DragapultYopifuttoBot::new(CardFacts::new());
        let mut r = rng();
        let mut me = player_with_hand(&[(200, "fire-energy"), (201, "psychic-energy")]);
        me.active = Some(in_play(1, "budew"));
        me.bench = vec![in_play_with_energy(2, "drakloak", &["fire-energy"])];
        let legal = vec![
            ActionDto::EndTurn,
            ActionDto::PlayCard {
                entity_id: EntityId(200),
                target: Some(ActionTarget::OwnBench { index: 0 }),
            },
            ActionDto::PlayCard {
                entity_id: EntityId(201),
                target: Some(ActionTarget::OwnBench { index: 0 }),
            },
        ];
        let a = bot
            .choose_action(&request_with(state_with("me", me), legal), &mut r)
            .expect("action");
        assert!(
            matches!(a, ActionDto::PlayCard { entity_id, .. } if entity_id == EntityId(201)),
            "炎炎を避けて超を付ける (got {a:?})"
        );
    }

    #[test]
    fn skips_energy_when_drakloak_full_and_attaches_dreepy() {
        // ベンチ drakloak は炎+超 充足 → 飛ばして dreepy へ。
        let mut bot = DragapultYopifuttoBot::new(CardFacts::new());
        let mut r = rng();
        let mut me = player_with_hand(&[(200, "fire-energy")]);
        me.active = Some(in_play(1, "budew"));
        me.bench = vec![
            in_play_with_energy(2, "drakloak", &["fire-energy", "psychic-energy"]),
            in_play(3, "dreepy"),
        ];
        let legal = vec![
            ActionDto::EndTurn,
            // drakloak への付与は overflow になるので legal に無い想定。dreepy への炎付与のみ。
            ActionDto::PlayCard {
                entity_id: EntityId(200),
                target: Some(ActionTarget::OwnBench { index: 1 }),
            },
        ];
        let a = bot
            .choose_action(&request_with(state_with("me", me), legal), &mut r)
            .expect("action");
        assert!(
            matches!(
                a,
                ActionDto::PlayCard {
                    target: Some(ActionTarget::OwnBench { index: 1 }),
                    ..
                }
            ),
            "充足 drakloak を飛ばして dreepy に付ける (got {a:?})"
        );
    }

    /// 相手バトル場 + ベンチ列から opp state を作る ((entity, hp_max, damage))。
    fn opp_state(active: Option<(u32, u16, u16)>, bench: &[(u32, u16, u16)]) -> StateDto {
        let mk = |(id, hp_max, dmg): (u32, u16, u16)| {
            let mut p = in_play(id, "dummy");
            p.hp_max = hp_max;
            p.damage = dmg;
            p
        };
        let mut opp = crate::bots::testutil::empty_player();
        opp.active = active.map(mk);
        opp.bench = bench.iter().copied().map(mk).collect();
        let mut s = state_with("me", crate::bots::testutil::empty_player());
        s.opp = opp;
        s
    }

    /// 相手ベンチを (entity, slug, hp_max, damage) で構成した state。
    fn opp_bench_state(bench: &[(u32, &str, u16, u16)]) -> StateDto {
        let mut opp = crate::bots::testutil::empty_player();
        opp.bench = bench
            .iter()
            .map(|&(id, slug, hp_max, dmg)| {
                let mut p = in_play(id, slug);
                p.hp_max = hp_max;
                p.damage = dmg;
                p
            })
            .collect();
        let mut s = state_with("me", crate::bots::testutil::empty_player());
        s.opp = opp;
        s
    }

    #[test]
    fn distribute_falls_to_three_each_for_unknown_slugs() {
        // ①-④ に該当しない slug (dummy) → ⑤: 上位から 3 個ずつ (total=6 → 2 体に 3)。
        let bot = DragapultYopifuttoBot::new(CardFacts::new());
        let state = opp_state(None, &[(1, 30, 0), (2, 60, 0), (3, 200, 0)]);
        let p = prompt_with_state(
            PromptDto::DistributeDamage {
                eligible: vec![1, 2, 3],
                total: 6,
                per_target_max: None,
            },
            state,
        );
        let c = bot
            .distribute_phantom_dive(&p, &[1, 2, 3], 6, None)
            .expect("dist");
        let pairs: std::collections::HashMap<u32, u8> = c
            .selected
            .iter()
            .copied()
            .zip(c.counts.iter().copied())
            .collect();
        assert_eq!(pairs.get(&1), Some(&3));
        assert_eq!(pairs.get(&2), Some(&3));
        assert_eq!(pairs.get(&3), None);
        assert_eq!(c.counts.iter().sum::<u8>(), 6);
    }

    #[test]
    fn distribute_respects_per_target_max() {
        // 残り HP 200 を 1 体だけ。per_target_max=2 → 2 しか置けない (overkill 防止)。
        let bot = DragapultYopifuttoBot::new(CardFacts::new());
        let state = opp_state(None, &[(5, 200, 0)]);
        let p = prompt_with_state(
            PromptDto::DistributeDamage {
                eligible: vec![5],
                total: 6,
                per_target_max: Some(2),
            },
            state,
        );
        let c = bot
            .distribute_phantom_dive(&p, &[5], 6, Some(2))
            .expect("dist");
        assert_eq!(c.selected, vec![5]);
        assert_eq!(c.counts, vec![2]);
    }

    #[test]
    fn distribute_kos_drakloak_then_spills_to_dreepy() {
        // F-a-14 ①: 相手ベンチ drakloak(残30) + dreepy(残60)。total=6。
        // drakloak を 3 個で倒し、余り 3 を ドラメシヤ(dreepy) へ。
        let bot = DragapultYopifuttoBot::new(CardFacts::new());
        let state = opp_bench_state(&[(1, "drakloak", 120, 90), (2, "dreepy", 60, 0)]);
        let p = prompt_with_state(
            PromptDto::DistributeDamage {
                eligible: vec![1, 2],
                total: 6,
                per_target_max: None,
            },
            state,
        );
        let c = bot
            .distribute_phantom_dive(&p, &[1, 2], 6, None)
            .expect("dist");
        let pairs: std::collections::HashMap<u32, u8> = c
            .selected
            .iter()
            .copied()
            .zip(c.counts.iter().copied())
            .collect();
        assert_eq!(pairs.get(&1), Some(&3), "drakloak を 3 個 (残30) で KO");
        assert_eq!(pairs.get(&2), Some(&3), "余り 3 をドラメシヤへ");
    }

    #[test]
    fn distribute_single_primary_target_places_all_counters() {
        // seed116 回帰: 対象が「キチキギスex 1 体」だけのとき、④で 1 個のせた後に余り 5 個を
        // 置く先が無く、旧実装は counts 合計=1 で DistributionMismatch → ask_prompt 無限ループ。
        // 修正後はフォールバックで同じ対象に余りを詰め、合計 6 (cap=total) になる。
        let bot = DragapultYopifuttoBot::new(CardFacts::new());
        let state = opp_bench_state(&[(20, "fezandipiti-ex", 210, 0)]);
        let p = prompt_with_state(
            PromptDto::DistributeDamage {
                eligible: vec![20],
                total: 6,
                per_target_max: None,
            },
            state,
        );
        let c = bot
            .distribute_phantom_dive(&p, &[20], 6, None)
            .expect("dist");
        assert_eq!(
            c.counts.iter().sum::<u8>(),
            6,
            "1 体に 6 個すべて配る (合計=total)"
        );
        assert_eq!(c.selected, vec![20]);
    }

    #[test]
    fn distribute_one_on_kiki_when_no_ko() {
        // F-a-14 ④: KO 対象なし + キチキギスex(fezandipiti-ex) → 1 個のせ、余りを他へ。
        // 相手: fezandipiti-ex(残200) + dummy(残200)。total=6。kiki 1 + 余り 5。
        let bot = DragapultYopifuttoBot::new(CardFacts::new());
        let state = opp_bench_state(&[(1, "fezandipiti-ex", 200, 0), (2, "dummy", 200, 0)]);
        let p = prompt_with_state(
            PromptDto::DistributeDamage {
                eligible: vec![1, 2],
                total: 6,
                per_target_max: None,
            },
            state,
        );
        let c = bot
            .distribute_phantom_dive(&p, &[1, 2], 6, None)
            .expect("dist");
        let pairs: std::collections::HashMap<u32, u8> = c
            .selected
            .iter()
            .copied()
            .zip(c.counts.iter().copied())
            .collect();
        assert_eq!(pairs.get(&1), Some(&1), "キチキギスex に 1 個");
        assert_eq!(c.counts.iter().sum::<u8>(), 6, "余りも置き切る");
    }

    #[test]
    fn find_attack_returns_none_without_registry() {
        // 空 registry ではワザ index を解決できない → None (fallback / end_turn に落ちる)。
        // 実 registry での発火は integration で担保。
        let bot = DragapultYopifuttoBot::new(CardFacts::new());
        let mut me = crate::bots::testutil::empty_player();
        me.active = Some(in_play(1, "dragapult-ex"));
        let state = state_with("me", me);
        let legal = vec![ActionDto::UseAttack { attack_index: 0 }, ActionDto::EndTurn];
        assert!(bot.find_attack(&state, &legal).is_none());
    }

    #[test]
    fn plays_crispin_when_field_energy_low() {
        // 場のエネ総数 0 (<3) + 手札アカマツ → アカマツを使う。
        let mut bot = DragapultYopifuttoBot::new(CardFacts::new());
        let mut r = rng();
        let mut me = player_with_hand(&[(90, "crispin"), (91, "lillie-s-determination")]);
        me.active = Some(in_play(1, "drakloak"));
        let legal = vec![
            ActionDto::EndTurn,
            ActionDto::PlayCard {
                entity_id: EntityId(90),
                target: None,
            },
            ActionDto::PlayCard {
                entity_id: EntityId(91),
                target: None,
            },
        ];
        let a = bot
            .choose_action(&request_with(state_with("me", me), legal), &mut r)
            .expect("action");
        assert!(
            matches!(a, ActionDto::PlayCard { entity_id, .. } if entity_id == EntityId(90)),
            "場エネ<3 でアカマツ優先 (got {a:?})"
        );
    }

    #[test]
    fn skips_crispin_and_plays_lillie_when_field_energy_high_in_f_turn() {
        // F-a-7 (turn 5): 場のエネ総数 3 (>=3) → アカマツ温存。リーリエを使う。
        let mut bot = DragapultYopifuttoBot::new(CardFacts::new());
        let mut r = rng();
        let mut me = player_with_hand(&[(90, "crispin"), (91, "lillie-s-determination")]);
        me.active = Some(in_play_with_energy(
            1,
            "drakloak",
            &["fire-energy", "psychic-energy"],
        ));
        me.bench = vec![in_play_with_energy(2, "dreepy", &["fire-energy"])];
        let mut state = state_with("me", me);
        state.turn = 5; // F
        let legal = vec![
            ActionDto::EndTurn,
            ActionDto::PlayCard {
                entity_id: EntityId(90),
                target: None,
            },
            ActionDto::PlayCard {
                entity_id: EntityId(91),
                target: None,
            },
        ];
        let a = bot
            .choose_action(&request_with(state, legal), &mut r)
            .expect("action");
        assert!(
            matches!(a, ActionDto::PlayCard { entity_id, .. } if entity_id == EntityId(91)),
            "F 番 場エネ>=3 でアカマツ温存 → リーリエ (got {a:?})"
        );
    }

    #[test]
    fn crispin_used_unconditionally_in_c_turn_even_with_high_energy() {
        // C-a-8 (turn 3): 場エネ>=3 でもアカマツを無条件で使う (F-a-7 のような温存条件は無い)。
        let mut bot = DragapultYopifuttoBot::new(CardFacts::new());
        let mut r = rng();
        let mut me = player_with_hand(&[(90, "crispin"), (91, "lillie-s-determination")]);
        me.active = Some(in_play_with_energy(
            1,
            "drakloak",
            &["fire-energy", "psychic-energy"],
        ));
        me.bench = vec![in_play_with_energy(2, "dreepy", &["fire-energy"])];
        let mut state = state_with("me", me);
        state.turn = 3; // C
        let legal = vec![
            ActionDto::EndTurn,
            ActionDto::PlayCard {
                entity_id: EntityId(90),
                target: None,
            },
            ActionDto::PlayCard {
                entity_id: EntityId(91),
                target: None,
            },
        ];
        let a = bot
            .choose_action(&request_with(state, legal), &mut r)
            .expect("action");
        assert!(
            matches!(a, ActionDto::PlayCard { entity_id, .. } if entity_id == EntityId(90)),
            "C 番はアカマツ無条件 (got {a:?})"
        );
    }

    #[test]
    fn crispin_not_used_in_b_turn() {
        // B (turn 1): アカマツは使わない (B-a-8 はリーリエのみ)。手札 crispin+lillie → リーリエ。
        let mut bot = DragapultYopifuttoBot::new(CardFacts::new());
        let mut r = rng();
        let mut me = player_with_hand(&[(90, "crispin"), (91, "lillie-s-determination")]);
        me.active = Some(in_play(1, "drakloak")); // 場エネ 0 (<3)
        let mut state = state_with("me", me);
        state.turn = 1; // B
        let legal = vec![
            ActionDto::EndTurn,
            ActionDto::PlayCard {
                entity_id: EntityId(90),
                target: None,
            },
            ActionDto::PlayCard {
                entity_id: EntityId(91),
                target: None,
            },
        ];
        let a = bot
            .choose_action(&request_with(state, legal), &mut r)
            .expect("action");
        assert!(
            matches!(a, ActionDto::PlayCard { entity_id, .. } if entity_id == EntityId(91)),
            "B 番はアカマツ不使用 → リーリエ (got {a:?})"
        );
    }

    #[test]
    fn lillie_kept_when_board_phantom_ready() {
        // 炎+超 付き dragapult-ex + drakloak 2匹 → リーリエ温存 (F-a-8)。EndTurn に落ちる。
        let mut bot = DragapultYopifuttoBot::new(CardFacts::new());
        let mut r = rng();
        let mut me = player_with_hand(&[(91, "lillie-s-determination")]);
        me.active = Some(in_play_with_energy(
            1,
            "dragapult-ex",
            &["fire-energy", "psychic-energy"],
        ));
        me.bench = vec![in_play(2, "drakloak"), in_play(3, "drakloak")];
        let legal = vec![
            ActionDto::EndTurn,
            ActionDto::PlayCard {
                entity_id: EntityId(91),
                target: None,
            },
        ];
        let a = bot
            .choose_action(&request_with(state_with("me", me), legal), &mut r)
            .expect("action");
        assert_eq!(a, ActionDto::EndTurn, "盤面完成 → リーリエ温存");
    }

    #[test]
    fn crispin_energy_search_picks_both_types() {
        // アカマツのエネ山札サーチ (候補 = 炎/超)。max=2 → 両方選ぶ。
        let mut bot = DragapultYopifuttoBot::new(CardFacts::new());
        let mut r = rng();
        let mk = |id: u32, slug: &str| EntityDto {
            entity_id: id,
            card: Some(slug.to_string()),
        };
        // 候補は手札に無い (= デッキ探索と判別される)。state.me.hand は空。
        let options = vec![mk(60, "fire-energy"), mk(61, "psychic-energy")];
        let mut p = prompt_with_state(
            PromptDto::ChooseFromZone {
                zone: "deck".to_string(),
                options,
            },
            state_with("me", crate::bots::testutil::empty_player()),
        );
        p.min = 0;
        p.max = 2;
        let c = bot.choose_prompt(&p, &mut r);
        assert_eq!(c.selected.len(), 2, "炎+超 を両方拾う");
        assert!(c.selected.contains(&60) && c.selected.contains(&61));
    }

    #[test]
    fn retreats_to_budew_when_active_not_attacker() {
        // バトル場 duskull (にげエネ有)・ベンチ budew。budew へにげる Retreat を選ぶ。
        let mut bot = DragapultYopifuttoBot::new(CardFacts::new());
        let mut r = rng();
        let mut me = crate::bots::testutil::empty_player();
        me.active = Some(in_play_with_energy(1, "duskull", &["psychic-energy"]));
        me.bench = vec![in_play(2, "dreepy"), in_play(3, "budew")];
        // budew はベンチ index 1。engine が列挙する Retreat{to_bench_index:1} を採用する。
        let legal = vec![
            ActionDto::EndTurn,
            ActionDto::Retreat {
                to_bench_index: 0,
                energy_to_discard: vec![EntityId(901)],
            },
            ActionDto::Retreat {
                to_bench_index: 1,
                energy_to_discard: vec![EntityId(901)],
            },
        ];
        let a = bot
            .choose_action(&request_with(state_with("me", me), legal), &mut r)
            .expect("action");
        assert!(
            matches!(
                a,
                ActionDto::Retreat {
                    to_bench_index: 1,
                    ..
                }
            ),
            "budew (bench 1) へにげる (got {a:?})"
        );
    }

    #[test]
    fn retreats_to_phantom_dragapult_over_budew() {
        // ベンチ 0 = 炎+超 付き dragapult-ex、1 = budew。ファントムダイブ優先で dragapult-ex へ。
        let mut bot = DragapultYopifuttoBot::new(CardFacts::new());
        let mut r = rng();
        let mut me = crate::bots::testutil::empty_player();
        me.active = Some(in_play_with_energy(1, "drakloak", &["psychic-energy"]));
        me.bench = vec![
            in_play_with_energy(2, "dragapult-ex", &["fire-energy", "psychic-energy"]),
            in_play(3, "budew"),
        ];
        let legal = vec![
            ActionDto::EndTurn,
            ActionDto::Retreat {
                to_bench_index: 0,
                energy_to_discard: vec![EntityId(901)],
            },
            ActionDto::Retreat {
                to_bench_index: 1,
                energy_to_discard: vec![EntityId(901)],
            },
        ];
        let a = bot
            .choose_action(&request_with(state_with("me", me), legal), &mut r)
            .expect("action");
        assert!(
            matches!(
                a,
                ActionDto::Retreat {
                    to_bench_index: 0,
                    ..
                }
            ),
            "ファントムダイブ可能な dragapult-ex (bench 0) を優先 (got {a:?})"
        );
    }

    #[test]
    fn no_retreat_when_active_is_budew_and_no_ready_dragapult() {
        // バトル場が budew で、撃てる dragapult-ex がベンチにいない (F-a-15 = むずむずかふん) →
        // にげない。Retreat を選ばず EndTurn (find_attack の むずむずかふんは registry 無しのため未解決)。
        let mut bot = DragapultYopifuttoBot::new(CardFacts::new());
        let mut r = rng();
        let mut me = crate::bots::testutil::empty_player();
        me.active = Some(in_play(1, "budew"));
        me.bench = vec![in_play(2, "dreepy")];
        let legal = vec![
            ActionDto::EndTurn,
            ActionDto::Retreat {
                to_bench_index: 0,
                energy_to_discard: vec![],
            },
        ];
        let a = bot
            .choose_action(&request_with(state_with("me", me), legal), &mut r)
            .expect("action");
        assert_eq!(
            a,
            ActionDto::EndTurn,
            "撃てる dragapult-ex 不在なら budew は据え置き (F-a-15)"
        );
    }

    #[test]
    fn budew_retreats_to_ready_dragapult_for_phantom_dive() {
        // 原文 F-a-14: バトル場が budew でも、撃てる (炎+超) dragapult-ex がベンチにいるなら、
        // にげて繰り出す (むずむずかふん 10 ではなくファントムダイブ 200 を撃つため)。
        let mut bot = DragapultYopifuttoBot::new(CardFacts::new());
        let mut r = rng();
        let mut me = crate::bots::testutil::empty_player();
        me.active = Some(in_play(1, "budew"));
        me.bench = vec![in_play_with_energy(
            2,
            "dragapult-ex",
            &["fire-energy", "psychic-energy"],
        )];
        let legal = vec![
            ActionDto::EndTurn,
            ActionDto::Retreat {
                to_bench_index: 0,
                energy_to_discard: vec![],
            },
        ];
        let a = bot
            .choose_action(&request_with(state_with("me", me), legal), &mut r)
            .expect("action");
        assert!(
            matches!(
                a,
                ActionDto::Retreat {
                    to_bench_index: 0,
                    ..
                }
            ),
            "budew をにがして撃てる dragapult-ex を繰り出す (got {a:?})"
        );
    }

    #[test]
    fn okunote_search_picks_lillie() {
        // おくのてキャッチのサポートサーチ (候補 = crispin/lillie/boss)。リーリエ優先。
        let mut bot = DragapultYopifuttoBot::new(CardFacts::new());
        let mut r = rng();
        let mk = |id: u32, slug: &str| EntityDto {
            entity_id: id,
            card: Some(slug.to_string()),
        };
        let options = vec![
            mk(70, "crispin"),
            mk(71, "lillie-s-determination"),
            mk(72, "boss-s-orders"),
        ];
        let mut p = prompt_with_state(
            PromptDto::ChooseFromZone {
                zone: "deck".to_string(),
                options,
            },
            state_with("me", crate::bots::testutil::empty_player()),
        );
        p.min = 1;
        p.max = 1;
        let c = bot.choose_prompt(&p, &mut r);
        assert_eq!(c.selected, vec![71], "リーリエ (ドロー源) を優先");
    }

    /// 手札ボス + ベンチに phantom-ready dragapult-ex を持つ自分 state に、指定の相手場を付ける。
    /// 自分サイドは 6 (F-a-13 路を試すため D 群 (サイド≤4) に入らない)。
    fn boss_setup(
        opp_active: Option<(u32, &str, u16)>,
        opp_bench: &[(u32, &str, u16)],
    ) -> StateDto {
        let mut me = player_with_hand(&[(97, "boss-s-orders")]);
        me.active = Some(in_play(1, "drakloak"));
        me.bench = vec![in_play_with_energy(
            2,
            "dragapult-ex",
            &["fire-energy", "psychic-energy"],
        )];
        me.prizes = (0..6)
            .map(|i| EntityDto {
                entity_id: 700 + i,
                card: None,
            })
            .collect();
        let mut state = state_with("me", me);
        let mut opp = crate::bots::testutil::empty_player();
        opp.active = opp_active.map(|(id, slug, hp)| {
            let mut p = in_play(id, slug);
            p.hp_max = hp;
            p
        });
        opp.bench = opp_bench
            .iter()
            .map(|&(id, slug, hp)| {
                let mut p = in_play(id, slug);
                p.hp_max = hp;
                p
            })
            .collect();
        state.opp = opp;
        state
    }

    fn boss_legal() -> Vec<ActionDto> {
        vec![
            ActionDto::EndTurn,
            ActionDto::PlayCard {
                entity_id: EntityId(97),
                target: None,
            },
        ]
    }

    #[test]
    fn boss_fa13_calls_kiki_when_no_opp_dragapult() {
        // F-a-13①: 相手 dragapult-ex 不在 + 相手ベンチに HP≤200 のキチキギスex → ボスで呼ぶ。
        let mut bot = DragapultYopifuttoBot::new(CardFacts::new());
        let mut r = rng();
        let state = boss_setup(None, &[(50, "fezandipiti-ex", 200)]);
        let a = bot
            .choose_action(&request_with(state, boss_legal()), &mut r)
            .expect("action");
        assert!(
            matches!(a, ActionDto::PlayCard { entity_id, .. } if entity_id == EntityId(97)),
            "F-a-13① でボス使用 (got {a:?})"
        );
    }

    #[test]
    fn boss_fa13_calls_single_drakloak() {
        // F-a-13②: 相手 dragapult-ex 不在 + 相手ドロンチが場に1匹のみ (ベンチ) → ボスで呼ぶ。
        let mut bot = DragapultYopifuttoBot::new(CardFacts::new());
        let mut r = rng();
        let state = boss_setup(Some((40, "snorlax-dummy", 300)), &[(50, "drakloak", 120)]);
        let a = bot
            .choose_action(&request_with(state, boss_legal()), &mut r)
            .expect("action");
        assert!(
            matches!(a, ActionDto::PlayCard { entity_id, .. } if entity_id == EntityId(97)),
            "F-a-13② でボス使用 (got {a:?})"
        );
    }

    #[test]
    fn boss_fa13_not_used_when_opp_has_dragapult() {
        // 相手の場に dragapult-ex がいる → ①② とも前提を満たさずボス不使用。
        let mut bot = DragapultYopifuttoBot::new(CardFacts::new());
        let mut r = rng();
        let state = boss_setup(
            Some((40, "dragapult-ex", 320)),
            &[(50, "fezandipiti-ex", 200)],
        );
        let a = bot
            .choose_action(&request_with(state, boss_legal()), &mut r)
            .expect("action");
        assert_eq!(a, ActionDto::EndTurn, "相手 dragapult-ex 在 → ボス不使用");
    }

    #[test]
    fn boss_fa13_not_used_without_matching_target() {
        // 相手 dragapult-ex 不在だが、キチキギス/ニャースex 無し・ドロンチ複数 → ①②外でボス不使用。
        let mut bot = DragapultYopifuttoBot::new(CardFacts::new());
        let mut r = rng();
        let state = boss_setup(None, &[(50, "drakloak", 120), (51, "drakloak", 120)]);
        let a = bot
            .choose_action(&request_with(state, boss_legal()), &mut r)
            .expect("action");
        assert_eq!(a, ActionDto::EndTurn, "①②該当なし → ボス温存");
    }

    #[test]
    fn choose_target_is_random_without_pending() {
        // pending_target が無い (原文が的を指定しない) 場面 → eligible から無作為に 1 つ選ぶ
        // (発明しない)。少なくとも eligible のいずれか 1 つを返す。
        let mut bot = DragapultYopifuttoBot::new(CardFacts::new());
        let mut r = rng();
        let state = opp_state(None, &[(1, 200, 0), (2, 60, 0), (3, 120, 0)]);
        let p = prompt_with_state(
            PromptDto::ChooseTargetPokemon {
                targets: vec![1, 2, 3],
            },
            state,
        );
        let c = bot.choose_prompt(&p, &mut r);
        assert_eq!(c.selected.len(), 1);
        assert!([1, 2, 3].contains(&c.selected[0]), "eligible から無作為");
    }

    #[test]
    fn cursed_bomb_target_is_largest_hp_ex() {
        // カースドボム使用 → pending_target に F-a-12「残りHPの大きい方」の ex。
        // 相手に残 HP 330 の ex (id 50) と 残 HP 60 の ex (id 51)。残 HP 最大の 330 を選ぶ (KO 不問)。
        let mut bot = DragapultYopifuttoBot::new(registry_with_ex("opp-ex"));
        let mut r = rng();
        let mut me = crate::bots::testutil::empty_player();
        me.active = Some(in_play(1, "drakloak"));
        me.bench = vec![
            in_play(2, "dusknoir"),
            in_play_with_energy(3, "dragapult-ex", &["fire-energy", "psychic-energy"]),
        ];
        let mut state = state_with("me", me);
        let mut opp = crate::bots::testutil::empty_player();
        let mut ex_big = in_play(50, "opp-ex");
        ex_big.hp_max = 330;
        let mut ex_small = in_play(51, "opp-ex");
        ex_small.hp_max = 60;
        opp.active = Some(ex_big);
        opp.bench = vec![ex_small];
        opp.prizes = (0..6)
            .map(|i| EntityDto {
                entity_id: 800 + i,
                card: None,
            })
            .collect();
        state.opp = opp;
        // カースドボム使用を決定 (pending_target 設定)。
        let legal = vec![
            ActionDto::EndTurn,
            ActionDto::UseAbility {
                entity_id: EntityId(2),
                ability_index: 0,
            },
        ];
        let a = bot
            .choose_action(&request_with(state.clone(), legal), &mut r)
            .expect("action");
        assert!(matches!(a, ActionDto::UseAbility { entity_id, .. } if entity_id == EntityId(2)));
        // 直後の ChooseTargetPokemon で残 HP 最大の ex (id 50) を選ぶ。
        let p = prompt_with_state(
            PromptDto::ChooseTargetPokemon {
                targets: vec![50, 51],
            },
            state,
        );
        let c = bot.choose_prompt(&p, &mut r);
        assert_eq!(c.selected, vec![50], "残 HP 最大の ex (F-a-12)");
    }

    /// `slug` を ex (prize 2) として持つ最小 [`CardFacts`]。
    fn registry_with_ex(slug: &str) -> CardFacts {
        CardFacts::new().with_card(slug, 2, &[])
    }

    #[test]
    fn cursed_bomb_used_when_phantom_ready_and_opp_has_ex() {
        // F-a-12: ファントムダイブ可能 dragapult-ex + dusknoir + 相手 ex (HP 不問) + サイド 6 → 使う。
        let mut bot = DragapultYopifuttoBot::new(registry_with_ex("opp-ex"));
        let mut r = rng();
        let mut me = crate::bots::testutil::empty_player();
        me.active = Some(in_play(1, "drakloak"));
        me.bench = vec![
            in_play(2, "dusknoir"),
            in_play_with_energy(3, "dragapult-ex", &["fire-energy", "psychic-energy"]),
        ];
        let mut state = state_with("me", me);
        let mut opp = crate::bots::testutil::empty_player();
        let mut ex = in_play(50, "opp-ex");
        ex.hp_max = 200; // KO 不問 (軟化目的)
        opp.active = Some(ex);
        opp.prizes = (0..6)
            .map(|i| EntityDto {
                entity_id: 800 + i,
                card: None,
            })
            .collect();
        state.opp = opp;
        let legal = vec![
            ActionDto::EndTurn,
            ActionDto::UseAbility {
                entity_id: EntityId(2),
                ability_index: 0,
            },
        ];
        let a = bot
            .choose_action(&request_with(state, legal), &mut r)
            .expect("action");
        assert!(
            matches!(a, ActionDto::UseAbility { entity_id, .. } if entity_id == EntityId(2)),
            "F-a-12 で ex を軟化 (got {a:?})"
        );
    }

    #[test]
    fn cursed_bomb_not_used_when_opp_prizes_le_1() {
        // 同条件でも相手サイド 1 → 自滅で負け筋なので使わない (グローバル制約)。
        let mut bot = DragapultYopifuttoBot::new(registry_with_ex("opp-ex"));
        let mut r = rng();
        let mut me = crate::bots::testutil::empty_player();
        me.active = Some(in_play(1, "drakloak"));
        me.bench = vec![
            in_play(2, "dusknoir"),
            in_play_with_energy(3, "dragapult-ex", &["fire-energy", "psychic-energy"]),
        ];
        let mut state = state_with("me", me);
        let mut opp = crate::bots::testutil::empty_player();
        let mut ex = in_play(50, "opp-ex");
        ex.hp_max = 200;
        opp.active = Some(ex);
        opp.prizes = vec![EntityDto {
            entity_id: 800,
            card: None,
        }];
        state.opp = opp;
        let legal = vec![
            ActionDto::EndTurn,
            ActionDto::UseAbility {
                entity_id: EntityId(2),
                ability_index: 0,
            },
        ];
        let a = bot
            .choose_action(&request_with(state, legal), &mut r)
            .expect("action");
        assert_eq!(a, ActionDto::EndTurn, "相手サイド≤1 ではカースドボム不可");
    }

    #[test]
    fn cursed_bomb_not_used_without_phantom_ready_dragapult() {
        // F-a-12 前提: ファントムダイブ可能な dragapult-ex が場にいない → 使わない。
        let mut bot = DragapultYopifuttoBot::new(registry_with_ex("opp-ex"));
        let mut r = rng();
        let mut me = crate::bots::testutil::empty_player();
        me.active = Some(in_play(1, "drakloak"));
        me.bench = vec![in_play(2, "dusknoir")]; // dragapult-ex 不在
        let mut state = state_with("me", me);
        let mut opp = crate::bots::testutil::empty_player();
        let mut ex = in_play(50, "opp-ex");
        ex.hp_max = 200;
        opp.active = Some(ex);
        opp.prizes = (0..6)
            .map(|i| EntityDto {
                entity_id: 800 + i,
                card: None,
            })
            .collect();
        state.opp = opp;
        let legal = vec![
            ActionDto::EndTurn,
            ActionDto::UseAbility {
                entity_id: EntityId(2),
                ability_index: 0,
            },
        ];
        let a = bot
            .choose_action(&request_with(state, legal), &mut r)
            .expect("action");
        assert_eq!(
            a,
            ActionDto::EndTurn,
            "ファントムダイブ可能 dragapult-ex 不在では使わない"
        );
    }

    #[test]
    fn uses_take_advantage_when_legal() {
        // ベンチ fezandipiti-ex + さかてにとる (UseAbility) legal → 使う (3 ドロー)。
        let mut bot = DragapultYopifuttoBot::new(CardFacts::new());
        let mut r = rng();
        let mut me = crate::bots::testutil::empty_player();
        me.active = Some(in_play(1, "drakloak"));
        me.bench = vec![in_play(2, "fezandipiti-ex")];
        let legal = vec![
            ActionDto::EndTurn,
            ActionDto::UseAbility {
                entity_id: EntityId(2),
                ability_index: 0,
            },
        ];
        let a = bot
            .choose_action(&request_with(state_with("me", me), legal), &mut r)
            .expect("action");
        assert!(
            matches!(a, ActionDto::UseAbility { entity_id, .. } if entity_id == EntityId(2)),
            "さかてにとるを使う (got {a:?})"
        );
    }

    #[test]
    fn benches_fezandipiti_after_ko() {
        // 前番に自ポケ KO (had_ko_last_turn) + 手札 fezandipiti-ex + 場に不在 → ベンチに出す。
        let mut bot = DragapultYopifuttoBot::new(CardFacts::new());
        let mut r = rng();
        let mut me = player_with_hand(&[(98, "fezandipiti-ex")]);
        me.active = Some(in_play(1, "budew"));
        me.had_ko_last_turn = true;
        let legal = vec![
            ActionDto::EndTurn,
            ActionDto::PlayCard {
                entity_id: EntityId(98),
                target: None,
            },
        ];
        let a = bot
            .choose_action(&request_with(state_with("me", me), legal), &mut r)
            .expect("action");
        assert!(
            matches!(a, ActionDto::PlayCard { entity_id, .. } if entity_id == EntityId(98)),
            "KO 後は fezandipiti-ex をベンチに出す (got {a:?})"
        );
    }

    #[test]
    fn no_fezandipiti_setup_without_ko() {
        // KO していない番は fezandipiti-ex (ex) を場に晒さない → 出さない。
        let mut bot = DragapultYopifuttoBot::new(CardFacts::new());
        let mut r = rng();
        let mut me = player_with_hand(&[(98, "fezandipiti-ex")]);
        me.active = Some(in_play(1, "budew"));
        me.had_ko_last_turn = false;
        let legal = vec![
            ActionDto::EndTurn,
            ActionDto::PlayCard {
                entity_id: EntityId(98),
                target: None,
            },
        ];
        let a = bot
            .choose_action(&request_with(state_with("me", me), legal), &mut r)
            .expect("action");
        assert_eq!(
            a,
            ActionDto::EndTurn,
            "KO 無しでは fezandipiti-ex を出さない"
        );
    }

    /// ベンチに `slugs` を並べた自分の state (turn 指定)。
    fn me_bench_state(turn: u32, slugs: &[&str]) -> StateDto {
        let mut me = crate::bots::testutil::empty_player();
        me.bench = slugs
            .iter()
            .enumerate()
            .map(|(i, s)| in_play(u32::try_from(i).unwrap_or(0) + 1, s))
            .collect();
        let mut s = state_with("me", me);
        s.turn = turn;
        s
    }

    #[test]
    fn poffin_targets_three_dreepy_in_later_turns() {
        // 2番目以降 (turn 3 → my_turn 2): ドラメシヤ目標3。ライン2 でも dreepy を最優先で詰める。
        let state = me_bench_state(3, &["dreepy", "dreepy", "budew"]);
        let pri = poffin_fetch_priority(&state);
        assert_eq!(
            pri.first(),
            Some(&"dreepy"),
            "C/F はライン2でも dreepy 目標3"
        );
    }

    #[test]
    fn poffin_targets_two_dreepy_in_first_turn() {
        // 1番目 (turn 1): 目標2。ライン2 達成済・スボミー有 → ドラメシヤでなくヨマワルを詰める。
        let state = me_bench_state(1, &["dreepy", "dreepy", "budew"]);
        let pri = poffin_fetch_priority(&state);
        assert_eq!(pri.first(), Some(&"duskull"), "B はライン2達成 → ヨマワル");
    }

    #[test]
    fn pokepad_fetches_dusclops_in_later_turn_with_two_hand_drakloak() {
        // C-a-4: 2番目以降 + ライン≥2 + スボミー有 + 手札ドロンチ≥2 + 場にヨマワル → サマヨールを先に。
        let mut me = crate::bots::testutil::empty_player();
        me.bench = vec![
            in_play(1, "dreepy"),
            in_play(2, "dreepy"),
            in_play(3, "budew"),
            in_play(4, "duskull"),
        ];
        me.hand = vec![
            EntityDto {
                entity_id: 20,
                card: Some("drakloak".to_string()),
            },
            EntityDto {
                entity_id: 21,
                card: Some("drakloak".to_string()),
            },
        ];
        let mut state = state_with("me", me);
        state.turn = 3; // my_turn 2 (C)
        let pri = desired_fetch_priority(&state);
        assert_eq!(
            pri.first(),
            Some(&"dusclops"),
            "手札ドロンチ2 + 場ヨマワル → サマヨール先"
        );
    }

    #[test]
    fn evolves_charged_drakloak_into_dragapult_ex() {
        // F-a-11: 場に dragapult-ex 不在 + 炎+超 付き drakloak (bench 0) + 手札 dragapult-ex →
        // その drakloak を target に dragapult-ex へ進化。
        let mut bot = DragapultYopifuttoBot::new(CardFacts::new());
        let mut r = rng();
        let mut me = player_with_hand(&[(80, "dragapult-ex")]);
        me.active = Some(in_play(1, "budew"));
        me.bench = vec![in_play_with_energy(
            2,
            "drakloak",
            &["fire-energy", "psychic-energy"],
        )];
        let legal = vec![
            ActionDto::EndTurn,
            // dragapult-ex を bench 0 の drakloak に進化させる合法手。
            ActionDto::PlayCard {
                entity_id: EntityId(80),
                target: Some(ActionTarget::OwnBench { index: 0 }),
            },
        ];
        let a = bot
            .choose_action(&request_with(state_with("me", me), legal), &mut r)
            .expect("action");
        assert!(
            matches!(a, ActionDto::PlayCard { entity_id, target: Some(ActionTarget::OwnBench { index: 0 }) } if entity_id == EntityId(80)),
            "炎+超 付き drakloak を dragapult-ex に進化 (got {a:?})"
        );
    }

    #[test]
    fn no_attacker_evolution_when_drakloak_not_charged() {
        // 炎のみの drakloak (超なし) → ファントムダイブ可能にならないので進化しない。
        let mut bot = DragapultYopifuttoBot::new(CardFacts::new());
        let mut r = rng();
        let mut me = player_with_hand(&[(80, "dragapult-ex")]);
        me.active = Some(in_play(1, "budew"));
        me.bench = vec![in_play_with_energy(2, "drakloak", &["fire-energy"])];
        let legal = vec![
            ActionDto::EndTurn,
            ActionDto::PlayCard {
                entity_id: EntityId(80),
                target: Some(ActionTarget::OwnBench { index: 0 }),
            },
        ];
        let a = bot
            .choose_action(&request_with(state_with("me", me), legal), &mut r)
            .expect("action");
        assert_eq!(a, ActionDto::EndTurn, "未充足 drakloak は進化しない");
    }

    #[test]
    fn fetch_prioritizes_dragapult_ex_when_charged_drakloak_waiting() {
        // F-a-11: 炎+超 付き drakloak + 場に dragapult-ex 不在 → ハイパーボール探索で dragapult-ex 最優先。
        let mut me = crate::bots::testutil::empty_player();
        me.bench = vec![in_play_with_energy(
            1,
            "drakloak",
            &["fire-energy", "psychic-energy"],
        )];
        let state = state_with("me", me);
        let pri = desired_fetch_priority(&state);
        assert_eq!(
            pri.first(),
            Some(&"dragapult-ex"),
            "充足 drakloak 待ち → dragapult-ex 最優先"
        );
    }

    #[test]
    fn recon_priority_c_turn_many_pokemon_prefers_lillie() {
        // C-a-10 場ポケ>2: リーリエ＞アカマツ＞ポケパッド… の順 (サポート優先)。
        let mut r = rng();
        let state = me_bench_state(3, &["dreepy", "drakloak", "duskull", "budew"]); // 場4体 (>2), my_turn 2 = C
        let pri = recon_priority(&state, &mut r);
        assert_eq!(
            pri.first(),
            Some(&"lillie-s-determination"),
            "C 番 >2 はリーリエ最優先"
        );
        // アカマツがリーリエの次。
        assert_eq!(pri.get(1), Some(&"crispin"));
    }

    #[test]
    fn recon_priority_f_turn_many_pokemon_prefers_crispin() {
        // F-a-9 場ポケ>2: アカマツ＞ハイパーボール＞リーリエ… の順。
        let mut r = rng();
        let state = me_bench_state(5, &["dreepy", "drakloak", "duskull", "budew"]); // my_turn 3 = F
        let pri = recon_priority(&state, &mut r);
        assert_eq!(pri.first(), Some(&"crispin"), "F 番 >2 はアカマツ最優先");
        assert_eq!(pri.get(1), Some(&"ultra-ball"));
    }

    #[test]
    fn recon_priority_few_pokemon_prefers_development_goods() {
        // 場ポケ≤2 (C): なかよしポフィン＞ポケパッド＞リーリエ… (盤面展開グッズ優先)。
        let mut r = rng();
        let state = me_bench_state(3, &["dreepy"]); // 場1体 (≤2)
        let pri = recon_priority(&state, &mut r);
        assert_eq!(
            pri.first(),
            Some(&"buddy-buddy-poffin"),
            "場薄いときは展開グッズ優先"
        );
    }

    #[test]
    fn recon_f_few_pokemon_basics_are_front_in_any_order() {
        // F-a-9 場ポケ≤2: 先頭は「たねポケモン」群 (内部順は無作為)。先頭3つに dreepy/duskull/budew が
        // 揃い、その直後が なかよしポフィン。
        let mut r = rng();
        let state = me_bench_state(5, &["dreepy"]); // my_turn 3 = F, 場1体 (≤2)
        let pri = recon_priority(&state, &mut r);
        let front: std::collections::HashSet<&str> = pri.iter().take(3).copied().collect();
        assert!(
            front.contains("dreepy") && front.contains("duskull") && front.contains("budew"),
            "先頭3つは たね3種 (順不同) (got {pri:?})"
        );
        assert_eq!(
            pri.get(3),
            Some(&"buddy-buddy-poffin"),
            "たねの後はポフィン"
        );
    }

    /// D 群テスト用: 手札にボス + ベンチに phantom-ready dragapult-ex + 自分サイド `my_sides` 枚 +
    /// 相手の場 (active, bench) を構成。registry は ex 判定用。
    fn d_group_state(
        my_sides: u32,
        opp_active: Option<(u32, &str, u16)>,
        opp_bench: &[(u32, &str, u16)],
    ) -> StateDto {
        let mut me = player_with_hand(&[(97, "boss-s-orders")]);
        me.active = Some(in_play(1, "drakloak"));
        me.bench = vec![in_play_with_energy(
            2,
            "dragapult-ex",
            &["fire-energy", "psychic-energy"],
        )];
        me.prizes = (0..my_sides)
            .map(|i| EntityDto {
                entity_id: 700 + i,
                card: None,
            })
            .collect();
        let mut state = state_with("me", me);
        let mut opp = crate::bots::testutil::empty_player();
        let mk = |(id, slug, hp): (u32, &str, u16)| {
            let mut p = in_play(id, slug);
            p.hp_max = hp;
            p
        };
        opp.active = opp_active.map(mk);
        opp.bench = opp_bench.iter().copied().map(mk).collect();
        opp.prizes = (0..6)
            .map(|i| EntityDto {
                entity_id: 900 + i,
                card: None,
            })
            .collect();
        state.opp = opp;
        state
    }

    #[test]
    fn d_group_sides2_bosses_ko_able_bench_ex() {
        // D-a-3-① (サイド2): 相手ベンチに残 HP≤200 の ex → ボスで呼ぶ。
        let mut bot = DragapultYopifuttoBot::new(registry_with_ex("opp-ex"));
        let mut r = rng();
        let state = d_group_state(2, None, &[(50, "opp-ex", 200)]);
        let a = bot
            .choose_action(&request_with(state, boss_legal()), &mut r)
            .expect("action");
        assert!(
            matches!(a, ActionDto::PlayCard { entity_id, .. } if entity_id == EntityId(97)),
            "D-a-3-①: KO 可能 ex をボスで呼ぶ (got {a:?})"
        );
    }

    #[test]
    fn d_a_1_bosses_ex_and_plans_six_counter_kos() {
        // D-a-1 (サイド4): 相手に残HP≤200 ex + 合計残HP≤60 の非ルールポケ2匹 → ボスで ex を呼び、
        // pending_distribute に小ポケ2匹を記録。続く DistributeDamage で 6 カウンタを 3+3 で割る。
        let mut bot = DragapultYopifuttoBot::new(registry_with_ex("opp-ex"));
        let mut r = rng();
        let state = d_group_state(
            4,
            None,
            &[(50, "opp-ex", 200), (51, "dummy", 30), (52, "dummy", 30)],
        );
        let a = bot
            .choose_action(&request_with(state.clone(), boss_legal()), &mut r)
            .expect("action");
        assert!(
            matches!(a, ActionDto::PlayCard { entity_id, .. } if entity_id == EntityId(97)),
            "本命 ex をボスで呼ぶ (got {a:?})"
        );
        // 続く DistributeDamage (相手ベンチ 51/52) で 3+3。
        let p = prompt_with_state(
            PromptDto::DistributeDamage {
                eligible: vec![51, 52],
                total: 6,
                per_target_max: None,
            },
            state,
        );
        let c = bot.choose_prompt(&p, &mut r);
        let pairs: std::collections::HashMap<u32, u8> = c
            .selected
            .iter()
            .copied()
            .zip(c.counts.iter().copied())
            .collect();
        assert_eq!(pairs.get(&51), Some(&3), "小ポケ 51 を 3 カウンタで KO");
        assert_eq!(pairs.get(&52), Some(&3), "小ポケ 52 を 3 カウンタで KO");
    }

    #[test]
    fn d_a_1_cursed_bombs_330_ex_first() {
        // D-a-1 (サイド4、軟化): 残HP330 ex + 合計≤60 小ポケ2匹 + dusknoir + ボス → まずカースドボム。
        let mut bot = DragapultYopifuttoBot::new(registry_with_ex("opp-ex"));
        let mut r = rng();
        let mut state = d_group_state(
            4,
            None,
            &[(50, "opp-ex", 330), (51, "dummy", 30), (52, "dummy", 30)],
        );
        // ベンチに dusknoir を追加 (カースドボム起動可)。
        state.me.bench.push(in_play(3, "dusknoir"));
        let legal = vec![
            ActionDto::EndTurn,
            ActionDto::PlayCard {
                entity_id: EntityId(97),
                target: None,
            },
            ActionDto::UseAbility {
                entity_id: EntityId(3),
                ability_index: 0,
            },
        ];
        let a = bot
            .choose_action(&request_with(state, legal), &mut r)
            .expect("action");
        assert!(
            matches!(a, ActionDto::UseAbility { entity_id, .. } if entity_id == EntityId(3)),
            "残HP330 ex はまずカースドボムで軟化 (got {a:?})"
        );
    }

    #[test]
    fn d_a_1_falls_through_without_small_targets() {
        // サイド4 + 本命 ex はいるが 合計≤60 の小ポケが無い → D-a-1 不成立 → F-a-1 (ボス使わず EndTurn)。
        let mut bot = DragapultYopifuttoBot::new(registry_with_ex("opp-ex"));
        let mut r = rng();
        let state = d_group_state(4, None, &[(50, "opp-ex", 200), (51, "dummy", 200)]);
        let a = bot
            .choose_action(&request_with(state, boss_legal()), &mut r)
            .expect("action");
        assert_eq!(a, ActionDto::EndTurn, "小ポケ無しで D-a-1 不成立 → F-a-1");
    }

    #[test]
    fn d_group_inactive_when_sides_high() {
        // サイド5 (>4) → F-ENTRY を満たさず D 群に入らない (F-a 路へ → ここではボス F-a-13 条件外で EndTurn)。
        let mut bot = DragapultYopifuttoBot::new(registry_with_ex("opp-ex"));
        let mut r = rng();
        let state = d_group_state(5, None, &[(50, "opp-ex", 200)]);
        let a = bot
            .choose_action(&request_with(state, boss_legal()), &mut r)
            .expect("action");
        // F-a-13①: 相手 dragapult-ex 不在 + ベンチ HP≤200 の キチキギス/ニャースex ではない (opp-ex) → ボス不使用。
        assert_eq!(a, ActionDto::EndTurn, "サイド>4 では D 群非該当");
    }

    #[test]
    fn d_group_sides2_bosses_non_ex_highest_hp() {
        // D-a-3-② (サイド2): 非ex も対象。相手ベンチに非ex 残HP60 と 残HP200 → HP高い方(200)を呼ぶ。
        let mut bot = DragapultYopifuttoBot::new(CardFacts::new()); // 空 registry → 全て非ex
        let mut r = rng();
        let state = d_group_state(2, None, &[(50, "dummy-a", 60), (51, "dummy-b", 200)]);
        let a = bot
            .choose_action(&request_with(state.clone(), boss_legal()), &mut r)
            .expect("action");
        assert!(
            matches!(a, ActionDto::PlayCard { entity_id, .. } if entity_id == EntityId(97)),
            "D-a-3-②: 非ex でもボス (got {a:?})"
        );
        // ChooseTargetPokemon で残HP最大 (id 51) を呼ぶ。
        let p = prompt_with_state(
            PromptDto::ChooseTargetPokemon {
                targets: vec![50, 51],
            },
            state,
        );
        let c = bot.choose_prompt(&p, &mut r);
        assert_eq!(c.selected, vec![51], "HP が高い方を呼ぶ (D-a-3-②)");
    }

    #[test]
    fn d_group_sides3_ignores_non_ex() {
        // サイド3 (D-a-2): 非ex は対象外 (ex 限定)。相手ベンチに非ex のみ → D 群はボスを使わない。
        let mut bot = DragapultYopifuttoBot::new(CardFacts::new());
        let mut r = rng();
        let state = d_group_state(3, None, &[(50, "dummy", 200)]);
        let a = bot
            .choose_action(&request_with(state, boss_legal()), &mut r)
            .expect("action");
        // D 群非該当 → F-a-13 も非該当 (dummy は kiki/meowth でない) → EndTurn。
        assert_eq!(a, ActionDto::EndTurn, "サイド3 は非ex を D 群対象にしない");
    }

    #[test]
    fn special_red_card_skipped_after_unfair_stamp_this_turn() {
        // F-a-13 いいえ: 同じ番にアンフェアスタンプを使った後はスペシャルレッドカードを使わない。
        let mut bot = DragapultYopifuttoBot::new(CardFacts::new());
        let mut r = rng();
        // 1st action: アンフェアスタンプ (前番KO + 相手手札4)。
        let mut me1 = player_with_hand(&[(99, "unfair-stamp"), (98, "special-red-card")]);
        me1.active = Some(in_play(1, "drakloak"));
        me1.had_ko_last_turn = true;
        let mut s1 = state_with("me", me1);
        s1.turn = 7;
        s1.opp.hand = (0..4)
            .map(|i| EntityDto {
                entity_id: 600 + i,
                card: None,
            })
            .collect();
        s1.opp.prizes = (0..3)
            .map(|i| EntityDto {
                entity_id: 900 + i,
                card: None,
            })
            .collect();
        let legal1 = vec![
            ActionDto::EndTurn,
            ActionDto::PlayCard {
                entity_id: EntityId(99),
                target: None,
            },
            ActionDto::PlayCard {
                entity_id: EntityId(98),
                target: None,
            },
        ];
        let a1 = bot
            .choose_action(&request_with(s1, legal1), &mut r)
            .expect("a1");
        assert!(
            matches!(a1, ActionDto::PlayCard { entity_id, .. } if entity_id == EntityId(99)),
            "まずアンフェアスタンプ"
        );
        // 2nd action (同じ番 turn 7): special-red-card だけ legal でも、アンスタ使用済なので使わない。
        let mut me2 = player_with_hand(&[(98, "special-red-card")]);
        me2.active = Some(in_play(1, "drakloak"));
        let mut s2 = state_with("me", me2);
        s2.turn = 7; // 同じ番
        s2.opp.prizes = (0..3)
            .map(|i| EntityDto {
                entity_id: 900 + i,
                card: None,
            })
            .collect();
        let legal2 = vec![
            ActionDto::EndTurn,
            ActionDto::PlayCard {
                entity_id: EntityId(98),
                target: None,
            },
        ];
        let a2 = bot
            .choose_action(&request_with(s2, legal2), &mut r)
            .expect("a2");
        assert_eq!(a2, ActionDto::EndTurn, "アンスタ使用済 → スペレ温存");
    }

    #[test]
    fn d_group_meowth_refills_boss_when_no_boss_in_hand() {
        // D-a-2 (サイド3): 本命 ex(≤200) + 残HP≤60 非ルールポケ1匹 (= 残り1サイド) + 手札にボス無し +
        // meowth-ex 出せる → ニャースex でおくのてキャッチ補充。
        let mut bot = DragapultYopifuttoBot::new(registry_with_ex("opp-ex"));
        let mut r = rng();
        let mut state = d_group_state(3, None, &[(50, "opp-ex", 200), (51, "dummy", 30)]);
        // 手札のボスを meowth-ex に差し替え。
        state.me.hand = vec![EntityDto {
            entity_id: 98,
            card: Some("meowth-ex".to_string()),
        }];
        let legal = vec![
            ActionDto::EndTurn,
            ActionDto::PlayCard {
                entity_id: EntityId(98),
                target: None,
            },
        ];
        let a = bot
            .choose_action(&request_with(state, legal), &mut r)
            .expect("action");
        assert!(
            matches!(a, ActionDto::PlayCard { entity_id, .. } if entity_id == EntityId(98)),
            "D 群: ボス無し → ニャースex で補充 (got {a:?})"
        );
    }

    #[test]
    fn energy_psychic_first_in_c_turn_non_budew_active() {
        // C-a-7 (turn 3, バトル場 drakloak): エネ無し drakloak へ超を先に付ける。
        let mut bot = DragapultYopifuttoBot::new(CardFacts::new());
        let mut r = rng();
        let mut me = player_with_hand(&[(200, "fire-energy"), (201, "psychic-energy")]);
        me.active = Some(in_play(1, "drakloak")); // budew でない
        let mut state = state_with("me", me);
        state.turn = 3; // C
        let legal = vec![
            ActionDto::EndTurn,
            ActionDto::PlayCard {
                entity_id: EntityId(200),
                target: Some(ActionTarget::OwnActive),
            },
            ActionDto::PlayCard {
                entity_id: EntityId(201),
                target: Some(ActionTarget::OwnActive),
            },
        ];
        let a = bot
            .choose_action(&request_with(state, legal), &mut r)
            .expect("action");
        assert!(
            matches!(a, ActionDto::PlayCard { entity_id, .. } if entity_id == EntityId(201)),
            "C 番非budewは超優先 (got {a:?})"
        );
    }

    #[test]
    fn energy_fire_first_in_f_turn() {
        // F (turn 5): 既定の炎優先。
        let mut bot = DragapultYopifuttoBot::new(CardFacts::new());
        let mut r = rng();
        let mut me = player_with_hand(&[(200, "fire-energy"), (201, "psychic-energy")]);
        me.active = Some(in_play(1, "drakloak"));
        let mut state = state_with("me", me);
        state.turn = 5; // F
        let legal = vec![
            ActionDto::EndTurn,
            ActionDto::PlayCard {
                entity_id: EntityId(200),
                target: Some(ActionTarget::OwnActive),
            },
            ActionDto::PlayCard {
                entity_id: EntityId(201),
                target: Some(ActionTarget::OwnActive),
            },
        ];
        let a = bot
            .choose_action(&request_with(state, legal), &mut r)
            .expect("action");
        assert!(
            matches!(a, ActionDto::PlayCard { entity_id, .. } if entity_id == EntityId(200)),
            "F 番は炎優先 (got {a:?})"
        );
    }

    #[test]
    fn plays_unfair_stamp_after_ko_with_big_opp_hand() {
        // F-a-3-①: 前番きぜつ + 相手手札4枚 → アンフェアスタンプ。
        let mut bot = DragapultYopifuttoBot::new(CardFacts::new());
        let mut r = rng();
        let mut me = player_with_hand(&[(99, "unfair-stamp")]);
        me.active = Some(in_play(1, "drakloak"));
        me.had_ko_last_turn = true;
        let mut state = state_with("me", me);
        state.opp.hand = (0..4)
            .map(|i| EntityDto {
                entity_id: 600 + i,
                card: None,
            })
            .collect();
        let legal = vec![
            ActionDto::EndTurn,
            ActionDto::PlayCard {
                entity_id: EntityId(99),
                target: None,
            },
        ];
        let a = bot
            .choose_action(&request_with(state, legal), &mut r)
            .expect("action");
        assert!(
            matches!(a, ActionDto::PlayCard { entity_id, .. } if entity_id == EntityId(99)),
            "KO 後 + 相手手札4枚 → アンフェアスタンプ (got {a:?})"
        );
    }

    #[test]
    fn plays_special_red_card_when_opp_low_prizes_and_no_phantom() {
        // F-a-13 いいえ: ファントムダイブ不可 + 相手サイド3 → スペシャルレッドカード。
        let mut bot = DragapultYopifuttoBot::new(CardFacts::new());
        let mut r = rng();
        let mut me = player_with_hand(&[(99, "special-red-card")]);
        me.active = Some(in_play(1, "drakloak")); // phantom-ready dragapult-ex 不在
        let mut state = state_with("me", me);
        state.opp.prizes = (0..3)
            .map(|i| EntityDto {
                entity_id: 900 + i,
                card: None,
            })
            .collect();
        let legal = vec![
            ActionDto::EndTurn,
            ActionDto::PlayCard {
                entity_id: EntityId(99),
                target: None,
            },
        ];
        let a = bot
            .choose_action(&request_with(state, legal), &mut r)
            .expect("action");
        assert!(
            matches!(a, ActionDto::PlayCard { entity_id, .. } if entity_id == EntityId(99)),
            "相手サイド≤3 + 非phantom → スペシャルレッドカード (got {a:?})"
        );
    }

    #[test]
    fn delegates_to_random_for_now() {
        let mut bot = DragapultYopifuttoBot::new(CardFacts::new());
        let legal = vec![ActionDto::EndTurn, ActionDto::Concede];
        let mut r = rng();
        let a = bot
            .choose_action(&dummy_request(legal.clone()), &mut r)
            .expect("legal");
        assert!(legal.contains(&a));
    }
}
