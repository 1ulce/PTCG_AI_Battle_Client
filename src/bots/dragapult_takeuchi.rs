//! `DragapultTakeuchiBot` — `decks/dragapult-ex.yaml` 固定デッキの決め打ち戦略 bot
//! (クチート竹内版ペルソナ)。
//!
//! 仕様の真値は `docs/bots/dragapult-ex.takeuchi.machine.md` (機械可読正規化版)。
//! 戦略ロジックはスライス単位で積み、未実装の判断は [`RandomPolicy`] に委譲する。
//!
//! 判断に必要なカード事実 (ex 判定・ワザ index) は [`CardFacts`] から引く (StateDto は
//! slug / stage / hp_max / damage / 装着エネ slug / status / 特性使用済みフラグのみを持つ)。

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
    pick_random_subset, play_card_slug, shuffle_strs, BotPolicy, PromptChoice, RandomPolicy,
};

/// S2: バトル場の最初の1体の優先度 (先攻)。仕様 §4。
const ACTIVE_PRIORITY_FIRST: &[&str] =
    &["dreepy", "budew", "duskull", "fezandipiti-ex", "meowth-ex"];
/// S2: バトル場の最初の1体の優先度 (後攻)。仕様 §4。
const ACTIVE_PRIORITY_SECOND: &[&str] =
    &["budew", "dreepy", "duskull", "fezandipiti-ex", "meowth-ex"];

/// crispin (アカマツ) の装着 2 段 prompt の進行段階 (§5.6.1)。
/// エネ search の後、keep-in-hand (装着エネ選択) → attach-target (装着先) と続く。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CrispinStage {
    None,
    KeepInHand,
    AttachTarget,
}

/// crispin の装着先 PICK-FIRST (§V5(c) の dragapult-ex(バトル場優先)→drakloak→dreepy)。
/// crispin は手札に来た時点では出所 (直接 / meowth おくのて) を区別できないため、常にこの優先を使う
/// (原文 §5.6.1 の dreepy 先頭は meowth 経由限定だが、手札からの play 時に判別不能 = 既知の近似)。
const CRISPIN_ATTACH_PRIORITY_DIRECT: &[&str] = &["dragapult-ex", "drakloak", "dreepy"];

/// dragapult-ex 固定デッキの決め打ち戦略 bot (クチート竹内版)。
pub struct DragapultTakeuchiBot {
    /// startup に clone した registry (orchestrator に move される本体とは別実体)。
    /// ワザ名→index 解決などカード事実の参照に使う。
    registry: CardFacts,
    /// 未実装の判断のフォールバック先。
    fallback: RandomPolicy,
    /// 直前のアクション (ボスの指令 / カースドボム) で決めた対象 entity。直後の
    /// `ChooseTargetPokemon` でこれを使う (原文が的を指定する場面の橋渡し)。take 後にクリア。
    pending_target: Option<u32>,
    /// 直前に出した探索カードの slug (poke-pad / buddy-buddy-poffin / ultra-ball)。
    /// 直後の `ChooseFromZone` で §5.3 のカード別フェッチ対象選択に使う。take 後にクリア。
    pending_search: Option<String>,
    /// 番内フラグの基準ターン (変わったらリセット)。
    current_turn: Option<u32>,
    /// §5.7 強制発動カースドボムを使った直後か (対象を §5.7 優先でなく RANDOM にする)。
    /// 直後の `ChooseTargetPokemon` で消費。
    pending_cursed_random: bool,
    /// §5.4 ふしぎなアメを出した直後か (対象たねを §5.4 優先で選ぶ)。
    /// 直後の `ChooseTargetPokemon` で消費。
    pending_rare_candy: bool,
    /// §5.6.1 crispin の装着 2 段 prompt の進行段階。
    crispin_stage: CrispinStage,
    /// crispin の装着先 (keep-in-hand 段で計画した対象、attach-target 段で使う)。
    crispin_attach_target: Option<u32>,
    /// crispin の装着先優先 (常に `CRISPIN_ATTACH_PRIORITY_DIRECT`、play 時に設定)。
    crispin_attach_pri: &'static [&'static str],
    /// 自分が先攻か (`ChooseFirstOrSecond` / `ChooseInitialActive` で確定)。
    /// `isFirstTurnGoingSecond` 等の番判定に使う (§3)。
    am_i_first: Option<bool>,
}

impl DragapultTakeuchiBot {
    #[must_use]
    pub fn new(registry: CardFacts) -> Self {
        Self {
            registry,
            fallback: RandomPolicy,
            pending_target: None,
            pending_search: None,
            current_turn: None,
            pending_cursed_random: false,
            pending_rare_candy: false,
            crispin_stage: CrispinStage::None,
            crispin_attach_target: None,
            crispin_attach_pri: CRISPIN_ATTACH_PRIORITY_DIRECT,
            am_i_first: None,
        }
    }
}

impl BotPolicy for DragapultTakeuchiBot {
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
            self.pending_cursed_random = false;
            self.pending_rare_candy = false;
            self.crispin_stage = CrispinStage::None;
            self.crispin_attach_target = None;
            // 直後の prompt で消費される前提だが、prompt 未発火で番を跨ぐ場合の stale を防ぐ。
            self.pending_target = None;
            self.pending_search = None;
        }
        // 全行動に優先して特性「ていさつしれい」(drakloak) を使う (仕様 共通行動 / U1 / V1)。
        // 引いたカードの選択 (look_at_deck_top) は後続 prompt で GAP-5 によりランダム応答。
        if let Some(a) = find_recon_directive(state, legal) {
            // GAP-5: recon の「加える1枚」は無作為。stale な goods source を持ち込まないようクリア。
            self.pending_search = None;
            return Ok(a);
        }
        // カースドボム: ヨノワール (dusknoir) に進化したらすぐ使う (§5.7)。自身を気絶させ
        // 相手にスナイプ。対象は choose_prompt の ChooseTargetPokemon で優先度選択。
        if let Some(a) = find_cursed_bomb(state, legal) {
            return Ok(a);
        }
        // ② 進化 (ドラパルト系優先 + ドロンチ止め) → たね展開
        if let Some(a) = find_evolution(state, legal, EVOLVE_PRIORITY) {
            return Ok(a);
        }
        // §5.4 ふしぎなアメ: 炎+超 dreepy → dragapult-ex (ドラパルト不在時) / 既定 duskull → dusknoir。
        // 対象たねは pending_rare_candy 経由で ChooseTargetPokemon にて §5.4 優先で選ぶ。
        if let Some(a) = self.find_rare_candy(state, legal) {
            return Ok(a);
        }
        // たね展開 (PLACE_BASICS、番別優先: U2 dreepy>budew>duskull / V2 dreepy>duskull>budew /
        // T1・T2 は dreepy 先頭・他は G1 無作為)。
        let basics = basic_priority(state, rng);
        if let Some(a) = find_basic_placement(state, legal, &basics) {
            return Ok(a);
        }
        // ③ グッズ (§5.3): poke-pad → buddy-buddy-poffin → ultra-ball → night-stretcher。
        // 出したグッズ slug を記録し、直後の ChooseFromZone でカード別フェッチ (§5.3) に使う。
        if let Some(a) = find_goods(state, legal) {
            self.pending_search = play_card_slug(state, &a).map(str::to_string);
            return Ok(a);
        }
        // §5.8 にげエネ: にげたいのににげエネ不足なら、バトル場に付けてにげを可能にする
        // (通常のアタッカー手張りより優先)。
        if let Some(a) = find_retreat_energy(state, legal) {
            return Ok(a);
        }
        // T1.4 (先攻初手): バトル場の dreepy に超優先で付ける (ベンチ優先より先)。
        if let Some(a) = self.find_t1_active_energy(state, legal) {
            return Ok(a);
        }
        // ④ エネ付与: ファントムダイブ用の炎+超を準備 (炎炎/超超回避・ベンチ優先)。
        if let Some(a) = find_energy_attach(state, legal) {
            return Ok(a);
        }
        // §V2: 監視塔 (両者の特性を無効化) が出ていて、この番 おくのてキャッチで boss を加えたい
        // なら jamming-tower で上書きし、自分の特性 (おくのてキャッチ) を復活させる (meowth 配置の前)。
        if let Some(a) = find_jamming_override(state, legal) {
            return Ok(a);
        }
        // ⑤ サポート (§5.6 / T2.5 / U6 / V5): 番別に lillie / crispin / boss を選ぶ。
        // boss は P_bossUse 成立時のみ・対象を pending_target に置く。
        if let Some(a) = self.find_support(state, legal, rng) {
            // 直後の ChooseFromZone 用に pending_search を設定:
            // crispin = エネサーチ (§5.6.1、装着先優先は §V5(c) 直接プレイ) /
            // meowth-ex = おくのてキャッチのサポートサーチ (§5.6) / それ以外はクリア。
            self.pending_search = match play_card_slug(state, &a) {
                Some("crispin") => {
                    self.crispin_attach_pri = CRISPIN_ATTACH_PRIORITY_DIRECT;
                    Some("crispin".to_string())
                }
                Some("meowth-ex") => Some("okunote".to_string()),
                _ => None,
            };
            return Ok(a);
        }
        // V6 / T1.5 (サポート後の妨害・スタジアム、順不同 G1): unfair-stamp / special-red-card /
        // team-rocket-s-watchtower。
        if let Some(a) = find_disruption(state, legal, rng) {
            return Ok(a);
        }
        // §5.7 強制発動: バトル場の dusclops/dusknoir がにげられず、ベンチに撃てる dragapult-ex か
        // budew が場にいるなら、カースドボムで自滅して攻撃役を繰り出す (対象は RANDOM)。
        if let Some(a) = self.find_forced_cursed_bomb(state, legal) {
            return Ok(a);
        }
        // にがして殴る (§5.8): case1 撃てる dragapult-ex へ / case2 budew へにげる (次ループで攻撃)。
        if let Some(a) = find_retreat(state, legal) {
            return Ok(a);
        }
        // ⑥ ワザ: ファントムダイブ最優先 → ジェットヘッド → むずむずかふん。
        // (ダメカン配分は choose_prompt の DistributeDamage で HP 低い順に処理)
        if let Some(a) = self.find_attack(state, legal) {
            return Ok(a);
        }
        // TODO(slice 3e+): グッズ / サポート。
        // 未実装の局面は「番を終える」を既定とする (random にすると未実装トレーナーズを
        // 誤爆する。特にふしぎなアメは engine 側が stub で sick 対象に対し match を中断
        // させる既知バグがあるため、それを踏まない)。EndTurn が無い特殊局面のみ random。
        if let Some(end) = legal.iter().find(|a| matches!(a, ActionDto::EndTurn)) {
            return Ok(end.clone());
        }
        self.fallback.choose_action(req, rng)
    }

    fn choose_prompt(&mut self, p: &PromptMsg, rng: &mut ChaCha20Rng) -> PromptChoice {
        match &p.kind {
            // G6: ジャンケン (コイン) に勝ったら後攻を選ぶ (yes=false → 相手が先攻)。
            // 後攻を選んだ = 自分は後攻 (am_i_first=false)。
            PromptDto::ChooseFirstOrSecond => {
                self.am_i_first = Some(false);
                PromptChoice {
                    selected: vec![],
                    counts: vec![],
                    yes: Some(false),
                    branch_index: None,
                }
            }
            // S2: バトル場の最初の1体を先攻/後攻別の優先度で選ぶ。
            // active_player=="me" ⟺ 自分が先攻 — ここで am_i_first を確定 (コイン敗者でも通る)。
            // (setup のベンチ初期配置 PlaceInitialBench は GAP-2 によりランダム = fallback)
            PromptDto::ChooseInitialActive { eligible } => {
                if let Some(s) = p.state.as_ref() {
                    self.am_i_first = Some(s.active_player == "me");
                }
                self.pick_initial_active(p, eligible)
                    .unwrap_or_else(|| self.fallback.choose_prompt(p, rng))
            }
            // ファントムダイブ等のダメカン配分: 相手の HP が低いポケモンから優先 (§U8/V7)。
            PromptDto::DistributeDamage {
                eligible,
                total,
                per_target_max,
            } => distribute_damage_low_hp_first(p, eligible, *total, *per_target_max)
                .unwrap_or_else(|| self.fallback.choose_prompt(p, rng)),
            // ChooseFromZone は (a) crispin の装着 2 段 / (b) ハイパーボールの手札トラッシュ cost /
            // (c) デッキ探索 に使われる。crispin 段階を最優先で判定し、残りは手札メンバーシップで判別。
            PromptDto::ChooseFromZone { options, .. } => self.choose_from_zone(p, options, rng),
            // 対象 1 匹選択: §5.4 ふしぎなアメの直後は対象たねを §5.4 優先で。§5.7 強制発動の直後は
            // RANDOM。それ以外は pending_target (ボスの的) → §5.7 スナイプ優先度 → 無作為 (発明しない)。
            PromptDto::ChooseTargetPokemon { targets } => {
                let chosen = if std::mem::take(&mut self.pending_rare_candy) {
                    pick_rare_candy_target(p.state.as_ref(), targets).or_else(|| {
                        (!targets.is_empty()).then(|| targets[rng.gen_range(0..targets.len())])
                    })
                } else if std::mem::take(&mut self.pending_cursed_random) {
                    (!targets.is_empty()).then(|| targets[rng.gen_range(0..targets.len())])
                } else {
                    self.pending_target
                        .take()
                        .filter(|t| targets.contains(t))
                        .or_else(|| pick_snipe_target(p, targets))
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
            // §5.9 REFILL_ACTIVE: バトル場が空になった時の繰り出し (番別の優先度)。
            PromptDto::ReplaceActiveAfterKo { bench_options } => {
                pick_refill_active(p.state.as_ref(), bench_options, rng).map_or_else(
                    || self.fallback.choose_prompt(p, rng),
                    |e| PromptChoice {
                        selected: vec![e],
                        counts: vec![],
                        yes: None,
                        branch_index: None,
                    },
                )
            }
            _ => self.fallback.choose_prompt(p, rng),
        }
    }
}

impl DragapultTakeuchiBot {
    /// S2: バトル場の最初の1体を選ぶ。`state` が無い場合は `None` (呼び出し側で random)。
    // 後続スライスで registry/メモリを参照するため method として置く。
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

    /// ⑥ ワザ選択: バトル場が dragapult-ex なら ファントムダイブ→ジェットヘッド、
    /// budew なら むずむずかふん。dreepy/drakloak のワザは使わない (仕様)。
    /// 合法な (= コスト充足) UseAttack のみ返す。
    fn find_attack(&self, state: &StateDto, legal: &[ActionDto]) -> Option<ActionDto> {
        let slug = state.me.active.as_ref()?.card.as_deref()?;
        let prefs: &[&str] = match slug {
            "dragapult-ex" => &["ファントムダイブ", "ジェットヘッド"],
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

    /// カード slug のワザ一覧 (POL `CardEffectDef.attacks`、YAML 順 = engine の
    /// `attack_index` 基準) から、名前一致するワザの index を引く。
    fn attack_index_by_name(&self, slug: &str, attack_name: &str) -> Option<u8> {
        self.registry.attack_index(slug, attack_name)
    }

    /// §3 `isFirstTurnGoingSecond`: 後攻側の自分の最初の番か。
    fn is_first_turn_going_second(&self, state: &StateDto) -> bool {
        self.am_i_first == Some(false) && my_turn_number(state) == 1
    }

    /// §5.3 共通「スボミー条件」: `isFirstTurnGoingSecond` または `P_phantomUnlikely`。
    fn budew_condition(&self, state: &StateDto) -> bool {
        self.is_first_turn_going_second(state) || p_phantom_unlikely(state)
    }

    /// T1.4 (先攻の最初の番): バトル場の dreepy に超優先で手張りする (ベンチ優先より先)。
    /// 炎+超 充足済み / バトル場が dreepy でなければ `None`。
    fn find_t1_active_energy(&self, state: &StateDto, legal: &[ActionDto]) -> Option<ActionDto> {
        if !(my_turn_number(state) == 1 && self.am_i_first == Some(true)) {
            return None;
        }
        let active = state.me.active.as_ref()?;
        if active.card.as_deref() != Some("dreepy") {
            return None;
        }
        // 超優先・!energyOverflow。
        let wanted: &[&str] = match (
            has_energy(active, "fire-energy"),
            has_energy(active, "psychic-energy"),
        ) {
            (_, false) => &["psychic-energy"],
            (false, true) => &["fire-energy"],
            (true, true) => return None,
        };
        for w in wanted {
            if let Some(a) = legal
                .iter()
                .find(|a| is_energy_attach_to(state, a, w, ActionTarget::OwnActive))
            {
                return Some(a.clone());
            }
        }
        None
    }

    /// ⑤ サポート (番別): T2.5 (後攻初手) lillie / U6 (2番目) lillie>crispin・boss不使用 /
    /// V5 (3番目以降) 分岐。boss を打つときは pending_target に的を置く。
    fn find_support(
        &mut self,
        state: &StateDto,
        legal: &[ActionDto],
        rng: &mut ChaCha20Rng,
    ) -> Option<ActionDto> {
        // §5.6 最優先: 使えるサポートが無い (or boss のみ & !P_bossUse) → meowth-ex を場に出して
        // 「おくのてキャッチ」でサポートを補充する。
        if let Some(a) = find_meowth_okunote(state, legal) {
            return Some(a);
        }
        match my_turn_number(state) {
            // 最初の番: 先攻 (T1) は support 無し。後攻 (T2.5) は lillie。
            0 | 1 => {
                if self.am_i_first == Some(false) {
                    play_card_named(state, legal, "lillie-s-determination")
                } else {
                    None
                }
            }
            // U6 (2番目の番): lillie > crispin、boss は使わない。
            2 => play_card_named(state, legal, "lillie-s-determination")
                .or_else(|| play_card_named(state, legal, "crispin")),
            // V5 (3番目以降): 分岐。
            _ => self.find_support_v5(state, legal, rng),
        }
    }

    /// §8.2 V5: (a) `P_phantomReadyActive` / (b) `P_phantomReadyBench` / (c) ファントムダイブ未充足。
    /// (a)(b) は共通: `P_bossUse`→boss が最優先 / 手札≤4 & !P_bossUse→lillie / 既定 crispin。
    /// (c): 場に dragapult-ex あり→crispin / なし→lillie。
    fn find_support_v5(
        &mut self,
        state: &StateDto,
        legal: &[ActionDto],
        rng: &mut ChaCha20Rng,
    ) -> Option<ActionDto> {
        if p_phantom_ready_active(state) || p_phantom_ready_bench(state) {
            // (a)/(b)。
            if p_boss_use(state) {
                if let Some(a) = self.find_boss(state, legal, rng) {
                    return Some(a);
                }
            } else if state.me.hand.len() <= 4 {
                if let Some(a) = play_card_named(state, legal, "lillie-s-determination") {
                    return Some(a);
                }
            }
            return play_card_named(state, legal, "crispin");
        }
        // (c): ファントムダイブ未充足。場に dragapult-ex があれば (= 炎/超 のどれか未装着) crispin。
        if count_in_play(state, "dragapult-ex") >= 1 {
            play_card_named(state, legal, "crispin")
        } else {
            play_card_named(state, legal, "lillie-s-determination")
        }
    }

    /// §5.6 ボスの指令: legal にあり的が取れれば打つ。的 = ダメカン無し & budew以外 & HP≤200
    /// の相手 (複数は G1 無作為)。的を pending_target に置く。
    fn find_boss(
        &mut self,
        state: &StateDto,
        legal: &[ActionDto],
        rng: &mut ChaCha20Rng,
    ) -> Option<ActionDto> {
        let boss = play_card_named(state, legal, "boss-s-orders")?;
        let cands: Vec<u32> = state
            .opp
            .active
            .iter()
            .chain(state.opp.bench.iter())
            .filter(|p| {
                p.damage == 0 && p.card.as_deref() != Some("budew") && remaining_hp(p) <= 200
            })
            .map(|p| p.entity_id)
            .collect();
        if cands.is_empty() {
            return None;
        }
        self.pending_target = Some(cands[rng.gen_range(0..cands.len())]); // G1
        Some(boss)
    }

    /// §5.7 強制発動カースドボム: バトル場が dusclops/dusknoir でにげられず (legal に Retreat 無し)、
    /// ベンチに canPhantomDive な dragapult-ex がいる or budew が場にいるなら、active のカースドボムで
    /// 自滅して攻撃役を繰り出す。対象は RANDOM (pending_cursed_random を立て ChooseTargetPokemon で消費)。
    fn find_forced_cursed_bomb(
        &mut self,
        state: &StateDto,
        legal: &[ActionDto],
    ) -> Option<ActionDto> {
        let active = state.me.active.as_ref()?;
        let slug = active.card.as_deref()?;
        if slug != "dusclops" && slug != "dusknoir" {
            return None;
        }
        // にげられない (= 合法な Retreat が無い)。
        if legal.iter().any(|a| matches!(a, ActionDto::Retreat { .. })) {
            return None;
        }
        // 繰り出す攻撃役がいる (ベンチに撃てる dragapult-ex / budew が場)。
        if !(p_phantom_ready_bench(state) || count_in_play(state, "budew") >= 1) {
            return None;
        }
        // active (dusclops/dusknoir) のカースドボム (UseAbility) を使う。
        let bomb = legal.iter().find(|a| {
            matches!(a, ActionDto::UseAbility { entity_id, .. } if entity_id.0 == active.entity_id)
        })?;
        self.pending_cursed_random = true;
        Some(bomb.clone())
    }

    /// §5.4 ふしぎなアメ: 出すべき局面なら `rare-candy` の `PlayCard` を返す (対象たねは
    /// pending_rare_candy 経由で ChooseTargetPokemon にて選ぶ)。出す条件は PICK-FIRST で、case1 =
    /// 炎+超 付き dreepy が場にいて手札に dragapult-ex があり場に dragapult-ex 不在 (ドロンチ止め)、
    /// case2 (既定) = duskull が場にいて手札に dusknoir がある。どちらも満たさなければ使わない
    /// (engine の legal は他のたねにも出せるが、戦略対象に限定する)。
    fn find_rare_candy(&mut self, state: &StateDto, legal: &[ActionDto]) -> Option<ActionDto> {
        let rc = play_card_named(state, legal, "rare-candy")?;
        if !(rare_candy_case1(state) || rare_candy_case2(state)) {
            return None;
        }
        self.pending_rare_candy = true;
        Some(rc)
    }

    /// `ChooseFromZone` の振り分け: crispin 装着 2 段 (§5.6.1) を最優先で処理し、残りは
    /// (a) 手札トラッシュ cost (ハイパーボール、GAP-3 ランダム) / (b) crispin エネ search /
    /// (c) §5.3 デッキ探索 / 無指定は G1 fallback。
    fn choose_from_zone(
        &mut self,
        p: &PromptMsg,
        options: &[EntityDto],
        rng: &mut ChaCha20Rng,
    ) -> PromptChoice {
        match self.crispin_stage {
            CrispinStage::KeepInHand => return self.crispin_keep_in_hand(p, options, rng),
            CrispinStage::AttachTarget => {
                return self.crispin_attach_target_choice(p, options, rng)
            }
            CrispinStage::None => {}
        }
        if is_hand_discard(p, options) {
            // (a) GAP-3: ハイパーボールのトラッシュは手札からランダム2枚 (温存ロジック無し)。
            let pool: Vec<u32> = options.iter().map(|o| o.entity_id).collect();
            return PromptChoice {
                selected: pick_random_subset(rng, &pool, p.min, p.max),
                counts: vec![],
                yes: None,
                branch_index: None,
            };
        }
        let source = self.pending_search.take();
        if source.as_deref() == Some("crispin") {
            // (b) crispin エネ search: ちがうタイプの基本エネ 2 枚 = 超 + 炎。
            let choice = pick_from_zone(options, &["psychic-energy", "fire-energy"], p.min, p.max);
            // keep-in-hand prompt は 2 枚以上取れた時だけ来る (attach_count = 取得数 - keep_in_hand=1)。
            // 1 枚以下なら後続 prompt が無いので stage を進めない (stale 化 → 別 prompt 誤消費を防ぐ)。
            if choice.selected.len() >= 2 {
                self.crispin_stage = CrispinStage::KeepInHand;
            }
            return choice;
        }
        // (c) §5.3 デッキ探索。無指定 (night-stretcher / recon 等) は G1 = fallback。
        match self.fetch_priority(p.state.as_ref(), source.as_deref(), rng) {
            Some(pri) => pick_from_zone(options, &pri, p.min, p.max),
            None => self.fallback.choose_prompt(p, rng),
        }
    }

    /// §5.6.1 crispin keep-in-hand 段: 装着する 1 枚を選ぶ。装着先と type を一緒に計画し
    /// (`!energyOverflow` を満たす)、装着先を crispin_attach_target に記録。計画できなければ G1。
    fn crispin_keep_in_hand(
        &mut self,
        p: &PromptMsg,
        options: &[EntityDto],
        rng: &mut ChaCha20Rng,
    ) -> PromptChoice {
        let plan = p
            .state
            .as_ref()
            .and_then(|s| self.plan_crispin_attach(s, options));
        self.crispin_stage = CrispinStage::AttachTarget;
        if let Some((energy_e, target_e)) = plan {
            if usize::from(p.max) == 1 {
                self.crispin_attach_target = Some(target_e);
                return PromptChoice {
                    selected: vec![energy_e],
                    counts: vec![],
                    yes: None,
                    branch_index: None,
                };
            }
        }
        // 計画不能 / 想定外の枚数 → 装着エネは G1、装着先も後段で G1。
        self.crispin_attach_target = None;
        let pool: Vec<u32> = options.iter().map(|o| o.entity_id).collect();
        PromptChoice {
            selected: pick_random_subset(rng, &pool, p.min, p.max),
            counts: vec![],
            yes: None,
            branch_index: None,
        }
    }

    /// §5.6.1 crispin attach-target 段: 計画した装着先を選ぶ。無ければ装着先優先 → G1。
    fn crispin_attach_target_choice(
        &mut self,
        p: &PromptMsg,
        options: &[EntityDto],
        rng: &mut ChaCha20Rng,
    ) -> PromptChoice {
        self.crispin_stage = CrispinStage::None;
        let chosen = self
            .crispin_attach_target
            .take()
            .filter(|t| options.iter().any(|o| o.entity_id == *t))
            .or_else(|| {
                p.state
                    .as_ref()
                    .and_then(|s| pick_own_by_priority(s, options, self.crispin_attach_pri))
            })
            .or_else(|| {
                (!options.is_empty()).then(|| options[rng.gen_range(0..options.len())].entity_id)
            });
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

    /// §5.6.1 crispin の装着 (エネ, 装着先) を一緒に計画する。装着先優先 (active 先) に走査し、
    /// 不足 type (超優先) を持つ最初のポケと、その type のエネ entity を返す。該当無しは `None`。
    fn plan_crispin_attach(&self, state: &StateDto, options: &[EntityDto]) -> Option<(u32, u32)> {
        let energy_of = |slug: &str| {
            options
                .iter()
                .find(|o| o.card.as_deref() == Some(slug))
                .map(|o| o.entity_id)
        };
        let fire_e = energy_of("fire-energy");
        let psychic_e = energy_of("psychic-energy");
        for slug in self.crispin_attach_pri {
            // active 優先 → ベンチ (§V5(c) バトル場優先)。
            let mut mons: Vec<&PokemonInPlayDto> = Vec::new();
            if let Some(a) = &state.me.active {
                if a.card.as_deref() == Some(*slug) {
                    mons.push(a);
                }
            }
            mons.extend(
                state
                    .me
                    .bench
                    .iter()
                    .filter(|b| b.card.as_deref() == Some(*slug)),
            );
            for mon in mons {
                if !has_energy(mon, "psychic-energy") {
                    if let Some(e) = psychic_e {
                        return Some((e, mon.entity_id));
                    }
                }
                if !has_energy(mon, "fire-energy") {
                    if let Some(e) = fire_e {
                        return Some((e, mon.entity_id));
                    }
                }
            }
        }
        None
    }

    /// 直前に出した探索グッズ (`source`) に応じたデッキ探索の対象優先度 (§5.3)。
    /// `None` を返すと呼び出し側で G1 (ランダム fallback)。
    fn fetch_priority(
        &self,
        state: Option<&StateDto>,
        source: Option<&str>,
        rng: &mut ChaCha20Rng,
    ) -> Option<Vec<&'static str>> {
        let state = state?;
        match source {
            Some("poke-pad") => Some(self.poke_pad_fetch(state, rng)),
            Some("buddy-buddy-poffin") => Some(self.poffin_fetch(state)),
            Some("ultra-ball") => Some(self.ultra_ball_fetch(state, rng)),
            // meowth-ex おくのてキャッチ (§5.6): サポートを優先度順に補充。
            Some("okunote") => Some(okunote_supporter_priority(state)),
            // night-stretcher は「そのSTEPで必要なポケモン (無指定なら G1)」→ ランダム fallback。
            _ => None,
        }
    }

    /// §5.3 `poke-pad` の取得対象。最優先 tier〔G3b 無作為〕→ 次 tier〔PICK-FIRST〕→ 既定 budew
    /// を **連結したフォールスルー型**で返す (優先順位リストは、上位の対象が山札に無ければ次位へ
    /// 落ちるのが常識。SKILL「優先順位フォールスルー」参照)。`pick_from_zone` が priority 順で
    /// 最初に山札にある候補を取るので、最優先が山札にあればそれ、無ければ次 tier、最後に budew。
    fn poke_pad_fetch(&self, state: &StateDto, rng: &mut ChaCha20Rng) -> Vec<&'static str> {
        let mut pri: Vec<&'static str> = Vec::new();
        // 最優先 tier: ガードを満たすものを集め、複数なら G3b でランダム順。
        let mut top: Vec<&'static str> = Vec::new();
        if self.budew_condition(state) {
            top.push("budew");
        }
        // 「進化できる dreepy がいる」→ drakloak (§5.3)。進化可 = 召喚酔いでない (turn_in_play>=1)。
        if has_evolvable_dreepy(state) {
            top.push("drakloak");
        }
        pri.extend(shuffle_strs(&top, rng)); // G3b
                                             // 次 tier (PICK-FIRST 上から)。最優先が山札に無いときの受け皿として常に連結する
                                             // (最優先が山札にあれば pick_from_zone が先にそちらを取るので無害)。
        let drakloak2 = count_in_play(state, "drakloak") >= 2;
        if count_in_play(state, "dreepy") == 0 {
            pri.push("dreepy");
        } else if drakloak2 && count_in_play(state, "duskull") >= 1 {
            let rare_candy = count_in_hand(state, "rare-candy") >= 1;
            let duskull_bench = state
                .me
                .bench
                .iter()
                .any(|p| p.card.as_deref() == Some("duskull"));
            if rare_candy && duskull_bench {
                pri.push("dusknoir");
            } else {
                pri.extend(shuffle_strs(&["dusclops", "dusknoir"], rng)); // G1
            }
        } else if drakloak2 {
            // drakloak≥2 かつ duskull 不在。
            pri.push("duskull");
        }
        pri.push("budew"); // 既定 (最低)
        pri
    }

    /// §5.3 `buddy-buddy-poffin` の取得対象 (たね限定 PICK-FIRST)。
    fn poffin_fetch(&self, state: &StateDto) -> Vec<&'static str> {
        let mut pri: Vec<&'static str> = Vec::new();
        if self.budew_condition(state) {
            pri.push("budew");
        }
        pri.push("dreepy");
        let line = count_in_play(state, "dreepy")
            + count_in_play(state, "drakloak")
            + count_in_play(state, "dragapult-ex");
        if line >= 3 {
            pri.push("duskull");
        }
        // たね限定の fallback (min 充足用)。
        pri.extend_from_slice(&["budew", "duskull", "dreepy"]);
        pri
    }

    /// §5.3 `ultra-ball` の取得対象。最優先 tier〔G3b 無作為・meowth-ex 重複排除〕→
    /// 次 tier〔PICK-FIRST〕→ 既定 budew を **連結したフォールスルー型**で返す
    /// (優先順位フォールスルー。SKILL 参照)。
    fn ultra_ball_fetch(&self, state: &StateDto, rng: &mut ChaCha20Rng) -> Vec<&'static str> {
        let mut pri: Vec<&'static str> = Vec::new();
        let mut top: Vec<&'static str> = Vec::new();
        // 手札に使えるサポートが無い / boss のみ → meowth-ex。
        let supporters: Vec<&str> = ["lillie-s-determination", "crispin", "boss-s-orders"]
            .into_iter()
            .filter(|s| count_in_hand(state, s) >= 1)
            .collect();
        let no_usable_support =
            supporters.is_empty() || supporters.iter().all(|s| *s == "boss-s-orders");
        if no_usable_support {
            top.push("meowth-ex");
        }
        if p_boss_win(state) {
            top.push("meowth-ex");
        }
        if self.budew_condition(state) {
            top.push("budew");
        }
        // 最優先 tier: meowth-ex 重複排除 (順序保持) → G3b 無作為。
        let mut seen = std::collections::HashSet::new();
        let dedup: Vec<&'static str> = top.into_iter().filter(|s| seen.insert(*s)).collect();
        pri.extend(shuffle_strs(&dedup, rng));
        // 次 tier (PICK-FIRST 上から)。最優先が山札に無いときの受け皿として常に連結する。
        if count_in_play(state, "dragapult-ex") == 0 && count_in_play(state, "drakloak") == 0 {
            pri.push("drakloak");
        }
        if energized_in_play(state, "drakloak") {
            pri.push("dragapult-ex");
        }
        if energized_in_play(state, "dreepy") && count_in_hand(state, "rare-candy") >= 1 {
            pri.push("dragapult-ex");
        }
        pri.push("dusclops");
        pri.push("dusknoir");
        pri.push("budew"); // 既定 (最低)
        pri
    }
}

/// 進化先の優先度 (仕様 §5.2: ドラパルトex 最優先 → ドロンチ → サマヨール → ヨノワール)。
/// 場に dragapult-ex がいる間は drakloak→dragapult-ex を保留 (find_evolution のドロンチ止め guard)。
const EVOLVE_PRIORITY: &[&str] = &["dragapult-ex", "drakloak", "dusclops", "dusknoir"];
/// ベンチに出すたねの番別優先度 (§5.1 / T1.2 / U2 / V2、ex は条件付きで除外)。
/// U2 (2番目): dreepy > budew > duskull。V2 (3番目以降): dreepy > duskull > budew。
/// T1 / T2 (最初の番): dreepy 先頭、残り (duskull / budew) は順不定 → G1 無作為。
fn basic_priority(state: &StateDto, rng: &mut ChaCha20Rng) -> Vec<&'static str> {
    match my_turn_number(state) {
        2 => vec!["dreepy", "budew", "duskull"],
        n if n >= 3 => vec!["dreepy", "duskull", "budew"],
        _ => {
            let mut v = vec!["dreepy"];
            v.extend(shuffle_strs(&["duskull", "budew"], rng));
            v
        }
    }
}

/// `options` に含まれる自分の場ポケから、slug `priority` 順 (active 先) に最初の entity を選ぶ。
fn pick_own_by_priority(state: &StateDto, options: &[EntityDto], priority: &[&str]) -> Option<u32> {
    let in_options = |eid: u32| options.iter().any(|o| o.entity_id == eid);
    for slug in priority {
        if let Some(a) = &state.me.active {
            if a.card.as_deref() == Some(*slug) && in_options(a.entity_id) {
                return Some(a.entity_id);
            }
        }
        if let Some(b) = state
            .me
            .bench
            .iter()
            .find(|b| b.card.as_deref() == Some(*slug) && in_options(b.entity_id))
        {
            return Some(b.entity_id);
        }
    }
    None
}

/// 自分の場 (バトル場 + ベンチ) から entity を引く。
fn my_in_play(state: &StateDto, entity_id: u32) -> Option<&PokemonInPlayDto> {
    state
        .me
        .active
        .iter()
        .chain(state.me.bench.iter())
        .find(|p| p.entity_id == entity_id)
}

/// §5.4 case1: 炎+超 付き dreepy が場にいて 手札に dragapult-ex があり 場に dragapult-ex 不在。
fn rare_candy_case1(state: &StateDto) -> bool {
    count_in_play(state, "dragapult-ex") == 0
        && count_in_hand(state, "dragapult-ex") >= 1
        && state
            .me
            .active
            .iter()
            .chain(state.me.bench.iter())
            .any(|p| {
                p.card.as_deref() == Some("dreepy")
                    && has_energy(p, "fire-energy")
                    && has_energy(p, "psychic-energy")
            })
}

/// §5.4 case2 (既定): duskull が場にいて 手札に dusknoir がある。
fn rare_candy_case2(state: &StateDto) -> bool {
    count_in_hand(state, "dusknoir") >= 1
        && state
            .me
            .active
            .iter()
            .chain(state.me.bench.iter())
            .any(|p| p.card.as_deref() == Some("duskull"))
}

/// §5.4 ふしぎなアメの対象たね選択 (ChooseTargetPokemon の rare_candy_base pool から)。
/// case1 (炎+超 dreepy・dragapult-ex 手札 & 不在) → その dreepy / 既定 → duskull。該当無しは `None`。
fn pick_rare_candy_target(state: Option<&StateDto>, targets: &[u32]) -> Option<u32> {
    let state = state?;
    // case1 が成立するなら、offered pool 中の 炎+超 dreepy を狙う。
    if rare_candy_case1(state) {
        if let Some(t) = targets.iter().copied().find(|&t| {
            my_in_play(state, t).is_some_and(|p| {
                p.card.as_deref() == Some("dreepy")
                    && has_energy(p, "fire-energy")
                    && has_energy(p, "psychic-energy")
            })
        }) {
            return Some(t);
        }
    }
    // 既定: duskull。
    targets
        .iter()
        .copied()
        .find(|&t| my_in_play(state, t).and_then(|p| p.card.as_deref()) == Some("duskull"))
}

/// 自分の場 (バトル場 + ベンチ) でなく相手の場に `slug` がいるか。
fn opp_has_in_play(state: &StateDto, slug: &str) -> bool {
    state
        .opp
        .active
        .iter()
        .chain(state.opp.bench.iter())
        .any(|p| p.card.as_deref() == Some(slug))
}

/// §8.2 V6 / §6 T1.5 サポート後の妨害・スタジアム (順不同 → 該当群から G1 で 1 つ):
/// (1) 前の番に自ポケがきぜつ & 手札に unfair-stamp → 使う。
/// (2) 相手のサイド残り ≤ 3 & 手札に special-red-card → 使う。
/// (3) meowth-ex おくのてキャッチ使用済 (= meowth-ex が自分の場にいる) & 相手が meowth-ex を場に
/// 出していない & 手札に team-rocket-s-watchtower → 出す。
fn find_disruption(
    state: &StateDto,
    legal: &[ActionDto],
    rng: &mut ChaCha20Rng,
) -> Option<ActionDto> {
    let mut options: Vec<ActionDto> = Vec::new();
    if state.me.had_ko_last_turn {
        if let Some(a) = play_card_named(state, legal, "unfair-stamp") {
            options.push(a);
        }
    }
    if state.opp.prizes.len() <= 3 {
        if let Some(a) = play_card_named(state, legal, "special-red-card") {
            options.push(a);
        }
    }
    if count_in_play(state, "meowth-ex") >= 1 && !opp_has_in_play(state, "meowth-ex") {
        if let Some(a) = play_card_named(state, legal, "team-rocket-s-watchtower") {
            options.push(a);
        }
    }
    if options.is_empty() {
        return None;
    }
    Some(options[rng.gen_range(0..options.len())].clone()) // 順不同 G1
}

/// §V2 jamming-tower 上書き: ロケット団の監視塔 (両者全員の特性を無効化) が場にあると、
/// 自分の meowth-ex「おくのてキャッチ」(特性) も無効化される。この番おくのてで boss を加えたい
/// (= 補充必要 & `P_bossUse` & meowth-ex を場に出せる) なら、jamming-tower (どうぐ無効) で上書きして
/// 特性を復活させる。§8.2 V2 (3番目以降の番)。
fn find_jamming_override(state: &StateDto, legal: &[ActionDto]) -> Option<ActionDto> {
    if my_turn_number(state) < 3 {
        return None;
    }
    if state.stadium.as_ref().and_then(|s| s.card.as_deref()) != Some("team-rocket-s-watchtower") {
        return None;
    }
    // 「この番おくのてで boss を加えたい」= 補充が必要で、かつ okunote の取得先が boss
    // (dragapult-ex エネ未充足で crispin が先に来る局面では boss を加えないので上書きしない)。
    if !supporter_refill_needed(state)
        || okunote_supporter_priority(state).first() != Some(&"boss-s-orders")
    {
        return None;
    }
    if !legal
        .iter()
        .any(|a| play_card_slug(state, a) == Some("meowth-ex"))
    {
        return None;
    }
    play_card_named(state, legal, "jamming-tower")
}

/// §5.6 共通: 使えるサポートが無い (or `boss-s-orders` のみ & `P_bossUse` 不成立) とき、
/// meowth-ex を場に出して「おくのてキャッチ」(on_play_to_bench triggered) でサポートを補充する。
/// okunote のサーチ対象は pending_search="okunote" 経由で §5.6 優先度から選ぶ
/// (pending_search は choose_action の find_support 後処理で設定)。
fn find_meowth_okunote(state: &StateDto, legal: &[ActionDto]) -> Option<ActionDto> {
    // 最初の番 (T1.3 / T2.3) は「手札に lillie が無いとき lillie を補充」が原文の条件。
    // 2 番目以降は §5.6 共通条件 (使えるサポート無 or boss のみ & !P_bossUse)。
    let need = if my_turn_number(state) <= 1 {
        count_in_hand(state, "lillie-s-determination") == 0
    } else {
        supporter_refill_needed(state)
    };
    if !need {
        return None;
    }
    legal
        .iter()
        .find(|a| play_card_slug(state, a) == Some("meowth-ex"))
        .cloned()
}

/// §5.6: サポート補充が必要か (手札に使えるサポートが無い、または `boss-s-orders` のみで
/// `P_bossUse` 不成立)。lillie / crispin は無条件で「使える」、boss は P_bossUse 成立時のみ。
fn supporter_refill_needed(state: &StateDto) -> bool {
    let supporters: Vec<&str> = ["lillie-s-determination", "crispin", "boss-s-orders"]
        .into_iter()
        .filter(|s| count_in_hand(state, s) >= 1)
        .collect();
    if supporters.is_empty() {
        return true;
    }
    supporters.iter().all(|s| *s == "boss-s-orders") && !p_boss_use(state)
}

/// §5.6 meowth-ex「おくのてキャッチ」で持ってくるサポートの優先度:
/// 1. dragapult-ex がいて ファントムダイブのエネ未充足 → crispin
/// 2. `P_bossUse` → boss-s-orders
/// 3. (既定) lillie-s-determination
fn okunote_supporter_priority(state: &StateDto) -> Vec<&'static str> {
    let mut pri: Vec<&'static str> = Vec::new();
    let dragapult_needs_energy = state
        .me
        .active
        .iter()
        .chain(state.me.bench.iter())
        .any(|p| {
            p.card.as_deref() == Some("dragapult-ex")
                && !(has_energy(p, "fire-energy") && has_energy(p, "psychic-energy"))
        });
    if dragapult_needs_energy {
        pri.push("crispin");
    }
    if p_boss_use(state) {
        pri.push("boss-s-orders");
    }
    pri.push("lillie-s-determination");
    pri
}

/// 手札の `slug` カード (サポート / グッズ / スタジアム = いずれも `PlayCard`) を使う手を返す。
fn play_card_named(state: &StateDto, legal: &[ActionDto], slug: &str) -> Option<ActionDto> {
    legal
        .iter()
        .find(|a| play_card_slug(state, a) == Some(slug))
        .cloned()
}

/// ポケモンが今すぐファントムダイブを撃てるか (dragapult-ex + 炎 + 超 装着)。
fn can_phantom_dive(mon: &PokemonInPlayDto) -> bool {
    mon.card.as_deref() == Some("dragapult-ex")
        && has_energy(mon, "fire-energy")
        && has_energy(mon, "psychic-energy")
}

/// active が攻撃役 (このデッキで使うワザを持つ dragapult-ex / budew) か。
fn active_is_attacker(mon: &PokemonInPlayDto) -> bool {
    matches!(mon.card.as_deref(), Some("dragapult-ex" | "budew"))
}

/// ベンチ index `idx` へのにげが legal にあればそれを返す。
fn retreat_to(legal: &[ActionDto], idx: usize) -> Option<ActionDto> {
    let i = u8::try_from(idx).ok()?;
    legal
        .iter()
        .find(|a| matches!(a, ActionDto::Retreat { to_bench_index, .. } if *to_bench_index == i))
        .cloned()
}

/// §5.8 にげる判断。case1 = `!canPhantomDive(active)` かつ `P_phantomReadyBench` のとき撃てる
/// dragapult-ex へにげる。case2 = `!canPhantomDive(active)` かつ `!P_phantomReadyBench` かつ
/// active が攻撃役でないとき、ベンチの budew へにげて むずむずかふん (budew 不在なら GOODS で
/// 先に出す = 後段で創発)。にげエネ付与は `find_retreat_energy` が先行ステップで処理する。
fn find_retreat(state: &StateDto, legal: &[ActionDto]) -> Option<ActionDto> {
    let active = state.me.active.as_ref()?;
    if can_phantom_dive(active) {
        return None; // 既にバトル場で撃てる → にげ不要
    }
    // case1: ベンチに撃てる dragapult-ex。
    if let Some(idx) = state.me.bench.iter().position(can_phantom_dive) {
        if let Some(a) = retreat_to(legal, idx) {
            return Some(a);
        }
    }
    // case2: 撃てるベンチ無し & active が攻撃役でない → budew をバトル場へ。
    if !p_phantom_ready_bench(state) && !active_is_attacker(active) {
        if let Some(idx) = state
            .me
            .bench
            .iter()
            .position(|p| p.card.as_deref() == Some("budew"))
        {
            if let Some(a) = retreat_to(legal, idx) {
                return Some(a);
            }
        }
    }
    None
}

/// §5.8 にげエネ付与: にげたい (case1 撃てる dragapult-ex / case2 budew がベンチ) のににげが
/// legal でない (にげエネ不足) とき、バトル場に基本エネを 1 枚付けてにげを可能にする。
/// type は不問 (にげ時にトラッシュ)。通常のアタッカー手張りより優先 (= ENERGY_ATTACH の前段)。
fn find_retreat_energy(state: &StateDto, legal: &[ActionDto]) -> Option<ActionDto> {
    let active = state.me.active.as_ref()?;
    if can_phantom_dive(active) {
        return None;
    }
    // 既ににげが合法なら付け足し不要 (find_retreat が処理)。
    if legal.iter().any(|a| matches!(a, ActionDto::Retreat { .. })) {
        return None;
    }
    let wants_case1 = p_phantom_ready_bench(state);
    let wants_case2 = !p_phantom_ready_bench(state)
        && !active_is_attacker(active)
        && state
            .me
            .bench
            .iter()
            .any(|p| p.card.as_deref() == Some("budew"));
    if !(wants_case1 || wants_case2) {
        return None;
    }
    for ty in ["psychic-energy", "fire-energy"] {
        if let Some(a) = legal
            .iter()
            .find(|a| is_energy_attach_to(state, a, ty, ActionTarget::OwnActive))
        {
            return Some(a.clone());
        }
    }
    None
}

/// §5.9 REFILL_ACTIVE: バトル場が空になった時 (相手ワザできぜつ / 自分のカースドボム自滅 等)
/// の繰り出し全般ルール。`bench_options` から繰り出す 1 体を**ターンで切り分けて**選ぶ
/// (2026-06-10 ユーザー確定: KO 原因は state に無いので自分の番/相手の番で分ける)。
/// 自分の番 (= 自分のカースドボム自滅等) は budew → エネ付きポケ G1。相手の番 (= 相手ワザできぜつ)
/// は budew → canPhantomDive dragapult-ex → エネ付き drakloak → エネ付き dreepy →
/// CURSED_BOMB 可 (dusclops/dusknoir) → duskull → dreepy。
/// `state` 無し / 該当無しは `None` (= 呼び出し側で G1 ランダム)。
fn pick_refill_active(
    state: Option<&StateDto>,
    bench_options: &[u32],
    rng: &mut ChaCha20Rng,
) -> Option<u32> {
    let state = state?;
    let lookup = |e: u32| state.me.bench.iter().find(|p| p.entity_id == e);
    let by_slug = |slug: &str| {
        bench_options
            .iter()
            .copied()
            .find(|&e| lookup(e).and_then(|p| p.card.as_deref()) == Some(slug))
    };
    let energized_slug = |slug: &str| {
        bench_options.iter().copied().find(|&e| {
            lookup(e)
                .is_some_and(|p| p.card.as_deref() == Some(slug) && !p.energy_attached.is_empty())
        })
    };
    if state.active_player == "me" {
        // 自分の番: budew → エネ付きポケ G1。
        if let Some(e) = by_slug("budew") {
            return Some(e);
        }
        let energized: Vec<u32> = bench_options
            .iter()
            .copied()
            .filter(|&e| lookup(e).is_some_and(|p| !p.energy_attached.is_empty()))
            .collect();
        return (!energized.is_empty()).then(|| energized[rng.gen_range(0..energized.len())]);
    }
    // 相手の番 (相手ワザできぜつ): rich 優先度。
    by_slug("budew")
        .or_else(|| {
            bench_options
                .iter()
                .copied()
                .find(|&e| lookup(e).is_some_and(can_phantom_dive))
        })
        .or_else(|| energized_slug("drakloak"))
        .or_else(|| energized_slug("dreepy"))
        .or_else(|| {
            bench_options.iter().copied().find(|&e| {
                matches!(
                    lookup(e).and_then(|p| p.card.as_deref()),
                    Some("dusclops" | "dusknoir")
                )
            })
        })
        .or_else(|| by_slug("duskull"))
        .or_else(|| by_slug("dreepy"))
}

/// ③ グッズの使用優先度 (§5.3 全体優先): poke-pad → buddy-buddy-poffin → ultra-ball →
/// night-stretcher。グッズロック中 (むずむずかふん) は engine が legal から除外する。
const GOODS_PRIORITY: &[&str] = &[
    "poke-pad",
    "buddy-buddy-poffin",
    "ultra-ball",
    "night-stretcher",
];

/// ③ グッズを全体優先度順に使う (legal にあれば)。fetch 対象は choose_prompt 側で
/// pending_search に応じて §5.3 のカード別フェッチを行う。
fn find_goods(state: &StateDto, legal: &[ActionDto]) -> Option<ActionDto> {
    for want in GOODS_PRIORITY {
        if let Some(a) = legal
            .iter()
            .find(|a| play_card_slug(state, a) == Some(*want))
        {
            return Some(a.clone());
        }
    }
    None
}

/// §3 述語: ポケモン slug が ex (ルール持ち) か。固定デッキの ex は全て `-ex` suffix
/// (dragapult-ex / meowth-ex / fezandipiti-ex) のため suffix 判定で厳密。
fn is_ex(slug: &str) -> bool {
    slug.ends_with("-ex")
}

/// ポケモンのサイド枚数 (ex=2 / それ以外=1)。固定デッキに 3 枚取りカードは無い。
fn prize_value_of(slug: &str) -> u32 {
    if is_ex(slug) {
        2
    } else {
        1
    }
}

/// ポケモンの残り HP。
fn remaining_hp(p: &PokemonInPlayDto) -> u16 {
    p.hp_max.saturating_sub(p.damage)
}

/// §3 `P_phantomReadyActive`: バトル場が dragapult-ex かつ canPhantomDive。
fn p_phantom_ready_active(state: &StateDto) -> bool {
    state.me.active.as_ref().is_some_and(can_phantom_dive)
}

/// §3 `P_phantomReadyBench`: ベンチに canPhantomDive な dragapult-ex がいる。
fn p_phantom_ready_bench(state: &StateDto) -> bool {
    state.me.bench.iter().any(can_phantom_dive)
}

/// §3 `P_phantomUnlikely`: ファントムダイブが現況の技選択肢に無い (= !P_phantomReadyActive)。
/// GAP-1: 手札先読みはしない。
fn p_phantom_unlikely(state: &StateDto) -> bool {
    !p_phantom_ready_active(state)
}

/// §5.3「進化できる dreepy」: 場の dreepy のうち召喚酔いでない (この番に出していない) ものがいるか。
/// `turn_in_play` は自番開始ごとに +1、配置直後は 0 (`reset_turn_flags`)。自分の番では `>= 1` が
/// 「前の番以前から場にいる」= 進化可能を意味する。dreepy はたねなので evolved_this_turn は無関係。
fn has_evolvable_dreepy(state: &StateDto) -> bool {
    state
        .me
        .active
        .iter()
        .chain(state.me.bench.iter())
        .any(|p| p.card.as_deref() == Some("dreepy") && p.turn_in_play >= 1)
}

/// 自分の場 (バトル場 + ベンチ) に、エネが 1 個以上付いた `slug` のポケモンがいるか。
fn energized_in_play(state: &StateDto, slug: &str) -> bool {
    state
        .me
        .active
        .iter()
        .chain(state.me.bench.iter())
        .any(|p| p.card.as_deref() == Some(slug) && !p.energy_attached.is_empty())
}

/// ファントムダイブ 1 回で達成できる `(最大サイド数, 最大 KO 数)`。バトル場へ 200 + 相手ベンチへ
/// ダメカン 6 個 (=60)。ボスで任意の相手を前面に引ける前提で、前面化する相手 X を全候補から選び、
/// X を 200 で倒し、残り (= ベンチ扱い) を 6 カウンタの knapsack で取り切る最大化 (prize/KO は独立に最大化)。
fn phantom_dive_outcomes(state: &StateDto) -> (u32, u32) {
    let opp: Vec<&PokemonInPlayDto> = state
        .opp
        .active
        .iter()
        .chain(state.opp.bench.iter())
        .collect();
    let mut best_prizes = 0u32;
    let mut best_kos = 0u32;
    for (xi, x) in opp.iter().enumerate() {
        let (main_p, main_k) = if remaining_hp(x) <= 200 {
            (prize_value_of(x.card.as_deref().unwrap_or("")), 1)
        } else {
            (0, 0)
        };
        let rest: Vec<&PokemonInPlayDto> = opp
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != xi)
            .map(|(_, p)| *p)
            .collect();
        let (bench_p, bench_k) = bench_counter_outcomes(&rest, 6);
        best_prizes = best_prizes.max(main_p + bench_p);
        best_kos = best_kos.max(main_k + bench_k);
    }
    (best_prizes, best_kos)
}

/// `items` を `cap` カウンタ (1 個 = ダメカン 10) 以内で倒して得られる `(最大サイド数, 最大 KO 数)`
/// (0/1 knapsack、prize/KO 数は独立に最大化)。
fn bench_counter_outcomes(items: &[&PokemonInPlayDto], cap: u32) -> (u32, u32) {
    let n = items.len();
    let mut best_p = 0u32;
    let mut best_k = 0u32;
    for mask in 0u32..(1u32 << n) {
        let mut cost = 0u32;
        let mut prizes = 0u32;
        let mut kos = 0u32;
        for (i, it) in items.iter().enumerate() {
            if mask & (1u32 << i) != 0 {
                cost += u32::from(remaining_hp(it).div_ceil(10));
                prizes += prize_value_of(it.card.as_deref().unwrap_or(""));
                kos += 1;
            }
        }
        if cost <= cap {
            best_p = best_p.max(prizes);
            best_k = best_k.max(kos);
        }
    }
    (best_p, best_k)
}

/// §3 `P_bossWin`: ボスの指令で勝てる (ファントムダイブで取り切れる OR 自分サイド残=1)。
fn p_boss_win(state: &StateDto) -> bool {
    let my_prizes = u32::try_from(state.me.prizes.len()).unwrap_or(u32::MAX);
    if my_prizes == 1 {
        return true;
    }
    if !(p_phantom_ready_active(state) || p_phantom_ready_bench(state)) {
        return false;
    }
    phantom_dive_outcomes(state).0 >= my_prizes
}

/// §3 `P_bossUse`: ボスの指令を使う条件 (相手 ex を倒せる OR 2匹以上倒せる OR 自分サイド残=1)。
fn p_boss_use(state: &StateDto) -> bool {
    if state.me.prizes.len() == 1 {
        return true; // cond C
    }
    // cond A/B はファントムダイブが撃てる前提。
    if !(p_phantom_ready_active(state) || p_phantom_ready_bench(state)) {
        return false;
    }
    // cond A: 相手 ex を 200 で倒せる。
    let can_ko_ex = state
        .opp
        .active
        .iter()
        .chain(state.opp.bench.iter())
        .any(|p| p.card.as_deref().is_some_and(is_ex) && remaining_hp(p) <= 200);
    // cond B: ファントムダイブで 2 匹以上倒せる。
    can_ko_ex || phantom_dive_outcomes(state).1 >= 2
}

/// カースドボム (dusknoir の起動特性) の `UseAbility` を探す。
/// dusknoir の起動特性はカースドボムのみ。進化したら即使用 (§5.7)。
fn find_cursed_bomb(state: &StateDto, legal: &[ActionDto]) -> Option<ActionDto> {
    legal
        .iter()
        .find(|a| {
            matches!(
                a,
                ActionDto::UseAbility { entity_id, .. }
                    if my_slug_of(state, entity_id.0) == Some("dusknoir")
            )
        })
        .cloned()
}

/// §5.7 スナイプ対象優先度 (相手の場): ex(最大HP) → エネ付き drakloak(エネ多) →
/// エネ付き dreepy → エネ無し drakloak → エネ無し dreepy → dusclops/dusknoir → 先頭。
/// `targets` に相手ポケモンが無ければ `None` (= 呼び出し側で random)。
fn pick_snipe_target(p: &PromptMsg, targets: &[u32]) -> Option<u32> {
    struct Cand {
        e: u32,
        is_ex: bool,
        energy: usize,
        slug: String,
        hp_max: u16,
    }
    let state = p.state.as_ref()?;
    let opp: Vec<Cand> = targets
        .iter()
        .filter_map(|&e| {
            let m = opp_in_play(state, e)?;
            let slug = m.card.clone().unwrap_or_default();
            Some(Cand {
                e,
                is_ex: is_ex(&slug),
                energy: m.energy_attached.len(),
                slug,
                hp_max: m.hp_max,
            })
        })
        .collect();
    if opp.is_empty() {
        return None;
    }
    if let Some(c) = opp.iter().filter(|c| c.is_ex).max_by_key(|c| c.hp_max) {
        return Some(c.e);
    }
    if let Some(c) = opp
        .iter()
        .filter(|c| c.slug == "drakloak" && c.energy > 0)
        .max_by_key(|c| c.energy)
    {
        return Some(c.e);
    }
    if let Some(c) = opp
        .iter()
        .filter(|c| c.slug == "dreepy" && c.energy > 0)
        .max_by_key(|c| c.energy)
    {
        return Some(c.e);
    }
    for want in ["drakloak", "dreepy"] {
        if let Some(c) = opp.iter().find(|c| c.slug == want) {
            return Some(c.e);
        }
    }
    if let Some(c) = opp
        .iter()
        .find(|c| c.slug == "dusclops" || c.slug == "dusknoir")
    {
        return Some(c.e);
    }
    // §5.7 の限定列挙に該当する相手が居ない (非ミラー等) → 原文は対象を定めない → None。
    // 呼び出し側 (ChooseTargetPokemon) が G1 ランダムにフォールバックする。
    None
}

/// ④ エネ付与先の優先度 (§5.5: ドラパルトex > ドロンチ > ドラメシヤ)。
const ENERGY_TARGET_PRIORITY: &[&str] = &["dragapult-ex", "drakloak", "dreepy"];

/// ④ エネ付与: アタッカー (ドラパルト系) に ファントムダイブ用の炎+超を、
/// 炎炎/超超を作らない範囲で 1 枚付ける。優先度順に「まだ充足していない」最初の
/// ポケモンへ。充足済み (炎+超 両方) は飛ばして次へ (§5.5 / V4: 1 匹ずつ揃える)。
fn find_energy_attach(state: &StateDto, legal: &[ActionDto]) -> Option<ActionDto> {
    for (target, mon) in attacker_targets_by_priority(state) {
        let has_fire = has_energy(mon, "fire-energy");
        let has_psychic = has_energy(mon, "psychic-energy");
        // 不足している type のみを候補にする (= 炎炎/超超を作らない)。超優先。
        let wanted: &[&str] = match (has_fire, has_psychic) {
            (false, false) => &["psychic-energy", "fire-energy"],
            (true, false) => &["psychic-energy"],
            (false, true) => &["fire-energy"],
            (true, true) => continue, // 充足 → 次のアタッカー
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

/// アタッカー候補 (ドラパルト系) を優先度順に列挙。各 slug 内はベンチ優先 (§GAP-4)。
fn attacker_targets_by_priority(state: &StateDto) -> Vec<(ActionTarget, &PokemonInPlayDto)> {
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
fn has_energy(mon: &PokemonInPlayDto, slug: &str) -> bool {
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

/// ファントムダイブ等のダメカン配分: 相手の HP が低いポケモンから優先的にのせる。
/// KO に必要な分だけ置き、余りは次に HP が低いポケモンへ (§U8/V7)。`per_target_max` と
/// `total` を厳密に守る (合計 = 置けるだけ、各 ≤ cap)。`state` 無し等は `None` (→ random)。
fn distribute_damage_low_hp_first(
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
    // (entity, 残り HP, KO に必要なカウンタ数)
    let mut targets: Vec<(u32, u16, u8)> = eligible
        .iter()
        .filter_map(|&e| {
            let mon = opp_in_play(state, e)?;
            let remaining = mon.hp_max.saturating_sub(mon.damage);
            let ko = u8::try_from(remaining.div_ceil(10))
                .unwrap_or(u8::MAX)
                .max(1);
            Some((e, remaining, ko))
        })
        .collect();
    if targets.is_empty() {
        return None;
    }
    // HP 低い順 (tie は entity_id で決定的に)。
    targets.sort_by(|a, b| a.1.cmp(&b.1).then(a.0.cmp(&b.0)));
    let mut counts: Vec<(u32, u8)> = targets.iter().map(|t| (t.0, 0u8)).collect();
    let mut left = total;
    // Phase 1: HP 低い順に「KO に必要な分」だけ (cap 以内)。
    for (i, t) in targets.iter().enumerate() {
        if left == 0 {
            break;
        }
        let give = left.min(cap).min(t.2);
        counts[i].1 = give;
        left -= give;
    }
    // Phase 2: 余りを上から cap まで詰める (全 total を置き切る)。
    while left > 0 {
        let mut progressed = false;
        for c in &mut counts {
            if left == 0 {
                break;
            }
            if c.1 < cap {
                c.1 += 1;
                left -= 1;
                progressed = true;
            }
        }
        if !progressed {
            break; // 全 target が cap 到達
        }
    }
    let selected: Vec<u32> = counts.iter().filter(|c| c.1 > 0).map(|c| c.0).collect();
    let counts: Vec<u8> = counts.iter().filter(|c| c.1 > 0).map(|c| c.1).collect();
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

/// 相手の場 (バトル場 + ベンチ) から entity を引く。
fn opp_in_play(state: &StateDto, entity_id: u32) -> Option<&PokemonInPlayDto> {
    if let Some(a) = &state.opp.active {
        if a.entity_id == entity_id {
            return Some(a);
        }
    }
    state.opp.bench.iter().find(|b| b.entity_id == entity_id)
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
    fn chooses_to_go_second() {
        // G6: コイン勝者なら後攻 (yes=false)。
        let mut bot = DragapultTakeuchiBot::new(CardFacts::new());
        let mut r = rng();
        let choice = bot.choose_prompt(&dummy_prompt(PromptDto::ChooseFirstOrSecond), &mut r);
        assert_eq!(choice.yes, Some(false));
    }

    #[test]
    fn first_turn_active_prefers_dreepy() {
        // 先攻 (active_player="me"): dreepy 最優先。手札順は budew→duskull→dreepy でも dreepy。
        let mut bot = DragapultTakeuchiBot::new(CardFacts::new());
        let mut r = rng();
        let me = player_with_hand(&[(10, "budew"), (11, "duskull"), (12, "dreepy")]);
        let p = prompt_with_state(
            PromptDto::ChooseInitialActive {
                eligible: vec![10, 11, 12],
            },
            state_with("me", me),
        );
        let choice = bot.choose_prompt(&p, &mut r);
        assert_eq!(choice.selected, vec![12], "先攻は dreepy(entity 12)");
    }

    #[test]
    fn second_turn_active_prefers_budew() {
        // 後攻 (active_player="opp"): budew 最優先。
        let mut bot = DragapultTakeuchiBot::new(CardFacts::new());
        let mut r = rng();
        let me = player_with_hand(&[(20, "dreepy"), (21, "budew"), (22, "duskull")]);
        let p = prompt_with_state(
            PromptDto::ChooseInitialActive {
                eligible: vec![20, 21, 22],
            },
            state_with("opp", me),
        );
        let choice = bot.choose_prompt(&p, &mut r);
        assert_eq!(choice.selected, vec![21], "後攻は budew(entity 21)");
    }

    #[test]
    fn second_turn_active_falls_through_to_dreepy_without_budew() {
        // 後攻で budew が無ければ dreepy。
        let mut bot = DragapultTakeuchiBot::new(CardFacts::new());
        let mut r = rng();
        let me = player_with_hand(&[(30, "duskull"), (31, "dreepy"), (32, "meowth-ex")]);
        let p = prompt_with_state(
            PromptDto::ChooseInitialActive {
                eligible: vec![30, 31, 32],
            },
            state_with("opp", me),
        );
        let choice = bot.choose_prompt(&p, &mut r);
        assert_eq!(
            choice.selected,
            vec![31],
            "budew 不在なら dreepy(entity 31)"
        );
    }

    #[test]
    fn uses_recon_directive_above_all() {
        // ベンチに drakloak。ていさつしれい (UseAbility) が EndTurn より優先される。
        let mut bot = DragapultTakeuchiBot::new(CardFacts::new());
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

    /// hand に進化カード (entity, slug) を持ち、active に dreepy がいる state を作る。
    fn state_with_active_and_hand(
        active: (u32, &str),
        in_play_extra: Vec<(u32, &str)>,
        hand: &[(u32, &str)],
    ) -> crate::wire::state::StateDto {
        let mut me = player_with_hand(hand);
        me.active = Some(in_play(active.0, active.1));
        me.bench = in_play_extra
            .into_iter()
            .map(|(id, slug)| in_play(id, slug))
            .collect();
        state_with("me", me)
    }

    #[test]
    fn evolves_toward_dragapult_line() {
        // ベンチに dreepy、手札に drakloak (進化カード)。PlayCard(drakloak) を選ぶ。
        let mut bot = DragapultTakeuchiBot::new(CardFacts::new());
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
    fn dronchi_stop_holds_drakloak_when_dragapult_already_in_play() {
        // 場に dragapult-ex がいる → 手札 dragapult-ex への進化 (PlayCard) は保留 → EndTurn 等へ。
        let mut bot = DragapultTakeuchiBot::new(CardFacts::new());
        let mut r = rng();
        let state = state_with_active_and_hand(
            (1, "dragapult-ex"),
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
        // 進化を選ばず EndTurn (この legal セットでは fallback も EndTurn 一択)。
        assert_eq!(a, ActionDto::EndTurn);
    }

    #[test]
    fn places_basic_dreepy_to_bench() {
        let mut bot = DragapultTakeuchiBot::new(CardFacts::new());
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

    /// PokemonInPlayDto に装着エネ slug を持たせる。
    fn in_play_with_energy(entity_id: u32, slug: &str, energy: &[&str]) -> PokemonInPlayDto {
        let mut p = in_play(entity_id, slug);
        p.energy_attached = energy
            .iter()
            .enumerate()
            .map(|(i, s)| crate::wire::state::EntityDto {
                entity_id: 900 + u32::try_from(i).unwrap_or(0),
                card: Some((*s).to_string()),
            })
            .collect();
        p
    }

    #[test]
    fn attaches_psychic_first_to_dragapult() {
        // バトル場 dragapult-ex (エネなし)。超優先で psychic を付ける。
        let mut bot = DragapultTakeuchiBot::new(CardFacts::new());
        let mut r = rng();
        let mut me = player_with_hand(&[(200, "psychic-energy"), (201, "fire-energy")]);
        me.active = Some(in_play(1, "dragapult-ex"));
        let legal = vec![
            ActionDto::EndTurn,
            ActionDto::PlayCard {
                entity_id: EntityId(201),
                target: Some(ActionTarget::OwnActive),
            },
            ActionDto::PlayCard {
                entity_id: EntityId(200),
                target: Some(ActionTarget::OwnActive),
            },
        ];
        let a = bot
            .choose_action(&request_with(state_with("me", me), legal), &mut r)
            .expect("action");
        assert!(
            matches!(a, ActionDto::PlayCard { entity_id, .. } if entity_id == EntityId(200)),
            "超エネ優先で付ける (got {a:?})"
        );
    }

    #[test]
    fn avoids_second_fire_overflow() {
        // 既に炎が付いた dragapult-ex → 炎炎を避け、psychic を付ける。
        let mut bot = DragapultTakeuchiBot::new(CardFacts::new());
        let mut r = rng();
        let mut me = player_with_hand(&[(201, "fire-energy"), (200, "psychic-energy")]);
        me.active = Some(in_play_with_energy(1, "dragapult-ex", &["fire-energy"]));
        let legal = vec![
            ActionDto::EndTurn,
            ActionDto::PlayCard {
                entity_id: EntityId(201),
                target: Some(ActionTarget::OwnActive),
            },
            ActionDto::PlayCard {
                entity_id: EntityId(200),
                target: Some(ActionTarget::OwnActive),
            },
        ];
        let a = bot
            .choose_action(&request_with(state_with("me", me), legal), &mut r)
            .expect("action");
        assert!(
            matches!(a, ActionDto::PlayCard { entity_id, .. } if entity_id == EntityId(200)),
            "炎炎を避けて超を付ける (got {a:?})"
        );
    }

    #[test]
    fn prefers_bench_dragapult_over_active_drakloak() {
        // バトル場 drakloak / ベンチ dragapult-ex。ベンチの dragapult-ex を優先して付ける。
        let mut bot = DragapultTakeuchiBot::new(CardFacts::new());
        let mut r = rng();
        let mut me = player_with_hand(&[(200, "psychic-energy")]);
        me.active = Some(in_play(1, "drakloak"));
        me.bench = vec![in_play(2, "dragapult-ex")];
        let legal = vec![
            ActionDto::EndTurn,
            ActionDto::PlayCard {
                entity_id: EntityId(200),
                target: Some(ActionTarget::OwnActive),
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
            matches!(
                a,
                ActionDto::PlayCard {
                    target: Some(ActionTarget::OwnBench { index: 0 }),
                    ..
                }
            ),
            "ベンチの dragapult-ex を優先 (got {a:?})"
        );
    }

    /// 相手バトル場 + ベンチ列から opp state を作る。
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

    #[test]
    fn distribute_targets_lowest_hp_first() {
        // ベンチ 3 体: 残り HP 30 / 60 / 200。total=6。
        // 30(=3 counters KO) → 60(=3 counters) で 6 を使い切る。200 には乗らない。
        let state = opp_state(None, &[(1, 30, 0), (2, 60, 0), (3, 200, 0)]);
        let p = prompt_with_state(
            PromptDto::DistributeDamage {
                eligible: vec![1, 2, 3],
                total: 6,
                per_target_max: None,
            },
            state,
        );
        let c = distribute_damage_low_hp_first(&p, &[1, 2, 3], 6, None).expect("dist");
        // entity 1 に 3、entity 2 に 3。
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
        let state = opp_state(None, &[(5, 200, 0)]);
        let p = prompt_with_state(
            PromptDto::DistributeDamage {
                eligible: vec![5],
                total: 6,
                per_target_max: Some(2),
            },
            state,
        );
        let c = distribute_damage_low_hp_first(&p, &[5], 6, Some(2)).expect("dist");
        assert_eq!(c.selected, vec![5]);
        assert_eq!(c.counts, vec![2]);
    }

    /// 指定 turn の自分番 request を作る (support の番判定用)。
    fn request_on_turn(me: PlayerView, legal: Vec<ActionDto>, turn: u32) -> RequestMsg {
        let mut s = state_with("me", me);
        s.turn = turn;
        request_with(s, legal)
    }

    #[test]
    fn u6_plays_lillie_over_crispin_no_boss() {
        // U6 (2番目の番 = turn 3): lillie > crispin、boss 不使用。手札に lillie と boss。
        let mut bot = DragapultTakeuchiBot::new(CardFacts::new());
        let mut r = rng();
        let mut me = player_with_hand(&[(80, "lillie-s-determination"), (81, "boss-s-orders")]);
        me.active = Some(in_play(1, "drakloak"));
        let legal = vec![
            ActionDto::EndTurn,
            ActionDto::PlayCard {
                entity_id: EntityId(80),
                target: None,
            },
            ActionDto::PlayCard {
                entity_id: EntityId(81),
                target: None,
            },
        ];
        let a = bot
            .choose_action(&request_on_turn(me, legal, 3), &mut r)
            .expect("action");
        assert!(matches!(a, ActionDto::PlayCard { entity_id, .. } if entity_id == EntityId(80)));
    }

    #[test]
    fn t2_5_second_player_plays_lillie() {
        // T2.5 (後攻の最初の番 = turn 2、am_i_first=false): lillie を使う。
        let mut bot = DragapultTakeuchiBot::new(CardFacts::new());
        bot.am_i_first = Some(false);
        let mut r = rng();
        let mut me = player_with_hand(&[(80, "lillie-s-determination")]);
        me.active = Some(in_play(1, "budew"));
        let legal = vec![
            ActionDto::EndTurn,
            ActionDto::PlayCard {
                entity_id: EntityId(80),
                target: None,
            },
        ];
        let a = bot
            .choose_action(&request_on_turn(me, legal, 2), &mut r)
            .expect("action");
        assert!(matches!(a, ActionDto::PlayCard { entity_id, .. } if entity_id == EntityId(80)));
    }

    #[test]
    fn v5_c_plays_lillie_without_dragapult() {
        // V5(c) (3番目以降・ファントムダイブ未充足・場に dragapult-ex なし): boss しか手札に
        // 無くても打たず (P_bossUse 前提なし)、lillie が無ければ何もしない → EndTurn。
        let mut bot = DragapultTakeuchiBot::new(CardFacts::new());
        let mut r = rng();
        let mut me = player_with_hand(&[(81, "boss-s-orders")]);
        me.active = Some(in_play(1, "drakloak")); // 撃てない・場に dragapult-ex 不在。
        let legal = vec![
            ActionDto::EndTurn,
            ActionDto::PlayCard {
                entity_id: EntityId(81),
                target: None,
            },
        ];
        let a = bot
            .choose_action(&request_on_turn(me, legal, 5), &mut r)
            .expect("action");
        assert_eq!(a, ActionDto::EndTurn, "V5(c) で boss は打たない");
    }

    #[test]
    fn v5_plays_boss_and_sets_target_when_boss_use() {
        // V5(a) P_phantomReadyActive かつ P_bossUse (相手 ex 残HP≤200) → boss を打ち、
        // 対象 (ダメカン無 & budew以外 & HP≤200) を pending_target に置く。
        let mut bot = DragapultTakeuchiBot::new(CardFacts::new());
        let mut r = rng();
        let mut me = phantom_ready_me(); // バトル場が撃てる dragapult-ex。
        me.hand = vec![EntityDto {
            entity_id: 81,
            card: Some("boss-s-orders".into()),
        }];
        let mut s = state_with("me", me);
        s.turn = 5; // V5。
        let mut ex = in_play(40, "dragapult-ex");
        ex.hp_max = 200; // 残HP200・ダメカン無 → ボスの的。
        s.opp.bench = vec![ex];
        let legal = vec![
            ActionDto::EndTurn,
            ActionDto::PlayCard {
                entity_id: EntityId(81),
                target: None,
            },
        ];
        let a = bot
            .choose_action(&request_with(s, legal), &mut r)
            .expect("action");
        assert!(matches!(a, ActionDto::PlayCard { entity_id, .. } if entity_id == EntityId(81)));
        assert_eq!(bot.pending_target, Some(40), "ボスの的を pending_target に");
    }

    #[test]
    fn p_boss_use_true_when_phantom_kos_two() {
        // P_phantomReadyActive かつ相手 active dreepy(60) + bench dreepy(60) →
        // 200 で active KO + 6 カウンタで bench KO = 2 匹 → P_bossUse true。
        let mut me = phantom_ready_me();
        me.prizes = prizes_of(6);
        let mut s = state_with("me", me);
        s.opp.active = Some({
            let mut p = in_play(50, "dreepy");
            p.hp_max = 60;
            p
        });
        s.opp.bench = vec![{
            let mut p = in_play(51, "dreepy");
            p.hp_max = 60;
            p
        }];
        assert!(p_boss_use(&s));
    }

    #[test]
    fn forced_cursed_bomb_when_dusclops_stuck_with_bench_attacker() {
        // バトル場 dusclops (にげる手なし) / ベンチに撃てる dragapult-ex → カースドボムで自滅。
        let mut bot = DragapultTakeuchiBot::new(CardFacts::new());
        let mut r = rng();
        let mut me = crate::bots::testutil::empty_player();
        me.active = Some(in_play(5, "dusclops"));
        me.bench = vec![in_play_with_energy(
            2,
            "dragapult-ex",
            &["fire-energy", "psychic-energy"],
        )];
        let legal = vec![
            ActionDto::EndTurn,
            ActionDto::UseAbility {
                entity_id: EntityId(5),
                ability_index: 0,
            },
        ];
        let a = bot
            .choose_action(&request_with(state_with("me", me), legal), &mut r)
            .expect("action");
        assert!(
            matches!(a, ActionDto::UseAbility { entity_id, .. } if entity_id == EntityId(5)),
            "active dusclops のカースドボムで自滅 (got {a:?})"
        );
        assert!(bot.pending_cursed_random, "対象は RANDOM フラグ");
    }

    #[test]
    fn no_forced_cursed_bomb_when_retreat_available() {
        // にげる手があれば強制カースドはしない (退避で対応)。
        let mut bot = DragapultTakeuchiBot::new(CardFacts::new());
        let mut r = rng();
        let mut me = crate::bots::testutil::empty_player();
        me.active = Some(in_play(5, "dusclops"));
        me.bench = vec![in_play_with_energy(
            2,
            "dragapult-ex",
            &["fire-energy", "psychic-energy"],
        )];
        let legal = vec![
            ActionDto::EndTurn,
            ActionDto::UseAbility {
                entity_id: EntityId(5),
                ability_index: 0,
            },
            ActionDto::Retreat {
                to_bench_index: 0,
                energy_to_discard: vec![],
            },
        ];
        let a = bot
            .choose_action(&request_with(state_with("me", me), legal), &mut r)
            .expect("action");
        // にげる手があるので強制カースドは出ず、退避 (Retreat) を選ぶ。
        assert!(
            matches!(a, ActionDto::Retreat { .. }),
            "にげられるなら退避 (got {a:?})"
        );
    }

    #[test]
    fn uses_cursed_bomb_when_dusknoir_in_play() {
        // ベンチに dusknoir → カースドボム (UseAbility) を進化系より優先で使う。
        let mut bot = DragapultTakeuchiBot::new(CardFacts::new());
        let mut r = rng();
        let mut me = PlayerView {
            bench: vec![in_play(9, "dusknoir")],
            ..crate::bots::testutil::empty_player()
        };
        me.active = Some(in_play(1, "dragapult-ex"));
        let legal = vec![
            ActionDto::EndTurn,
            ActionDto::UseAbility {
                entity_id: EntityId(9),
                ability_index: 0,
            },
        ];
        let a = bot
            .choose_action(&request_with(state_with("me", me), legal), &mut r)
            .expect("action");
        assert!(matches!(a, ActionDto::UseAbility { entity_id, .. } if entity_id == EntityId(9)));
    }

    #[test]
    fn snipe_targets_opponent_ex_with_max_hp() {
        // 相手の場: ドロンチ(エネ2) / ドラパルトex(HP320) / 別ドラパルトex(HP280)。
        // ex 最大 HP を狙う (entity 21, HP320)。
        let mut opp = crate::bots::testutil::empty_player();
        let mut ex_big = in_play(21, "dragapult-ex");
        ex_big.hp_max = 320;
        let mut ex_small = in_play(22, "dragapult-ex");
        ex_small.hp_max = 280;
        let mut dro = in_play(23, "drakloak");
        dro.energy_attached = vec![
            crate::wire::state::EntityDto {
                entity_id: 901,
                card: Some("fire-energy".into()),
            },
            crate::wire::state::EntityDto {
                entity_id: 902,
                card: Some("psychic-energy".into()),
            },
        ];
        opp.active = Some(ex_small);
        opp.bench = vec![ex_big, dro];
        let mut s = state_with("me", crate::bots::testutil::empty_player());
        s.opp = opp;
        let p = prompt_with_state(
            PromptDto::ChooseTargetPokemon {
                targets: vec![21, 22, 23],
            },
            s,
        );
        assert_eq!(pick_snipe_target(&p, &[21, 22, 23]), Some(21));
    }

    #[test]
    fn pending_target_overrides_snipe_priority() {
        // pending_target が立っていれば §5.7 スナイプより優先される (ボス/強制カースドの橋渡し)。
        let mut bot = DragapultTakeuchiBot::new(CardFacts::new());
        let mut r = rng();
        let mut opp = crate::bots::testutil::empty_player();
        let mut ex = in_play(21, "dragapult-ex");
        ex.hp_max = 320;
        opp.active = Some(ex); // snipe なら ex(21) を狙うはず。
        opp.bench = vec![in_play(22, "dreepy")];
        let mut s = state_with("me", crate::bots::testutil::empty_player());
        s.opp = opp;
        bot.pending_target = Some(22); // だが pending で dreepy(22) を指定。
        let p = prompt_with_state(
            PromptDto::ChooseTargetPokemon {
                targets: vec![21, 22],
            },
            s,
        );
        let c = bot.choose_prompt(&p, &mut r);
        assert_eq!(c.selected, vec![22], "pending_target が優先");
        // take 済みなので、再度 prompt すると snipe にフォールバック (ex を狙う)。
    }

    #[test]
    fn snipe_falls_to_energized_drakloak_without_ex() {
        // ex 無し: エネ付き drakloak を狙う。
        let mut opp = crate::bots::testutil::empty_player();
        let mut dro = in_play(31, "drakloak");
        dro.energy_attached = vec![crate::wire::state::EntityDto {
            entity_id: 903,
            card: Some("fire-energy".into()),
        }];
        opp.active = Some(in_play(30, "dreepy"));
        opp.bench = vec![dro];
        let mut s = state_with("me", crate::bots::testutil::empty_player());
        s.opp = opp;
        let p = prompt_with_state(
            PromptDto::ChooseTargetPokemon {
                targets: vec![30, 31],
            },
            s,
        );
        assert_eq!(pick_snipe_target(&p, &[30, 31]), Some(31));
    }

    #[test]
    fn retreats_to_bring_up_phantom_ready_dragapult() {
        // バトル場 drakloak (撃てない) / ベンチ[0] dragapult-ex (炎+超) → ベンチ0へにげる。
        let mut bot = DragapultTakeuchiBot::new(CardFacts::new());
        let mut r = rng();
        let mut me = crate::bots::testutil::empty_player();
        me.active = Some(in_play(1, "drakloak"));
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
        assert!(matches!(
            a,
            ActionDto::Retreat {
                to_bench_index: 0,
                ..
            }
        ));
    }

    #[test]
    fn case2_retreats_to_budew_when_no_phantom_bench() {
        // バトル場 drakloak (攻撃役でない・撃てない) / ベンチ phantom 無し / ベンチに budew →
        // budew へにげる (case2)。
        let mut bot = DragapultTakeuchiBot::new(CardFacts::new());
        let mut r = rng();
        let mut me = crate::bots::testutil::empty_player();
        me.active = Some(in_play(1, "drakloak"));
        me.bench = vec![in_play(2, "budew")];
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
            "budew へにげる (got {a:?})"
        );
    }

    #[test]
    fn retreat_energy_attached_when_retreat_not_yet_legal() {
        // case2 にげたいが Retreat 非合法 (にげエネ不足) → バトル場に超エネを付ける (にげ準備)。
        let mut bot = DragapultTakeuchiBot::new(CardFacts::new());
        let mut r = rng();
        let mut me = player_with_hand(&[(200, "psychic-energy")]);
        me.active = Some(in_play(1, "drakloak"));
        me.bench = vec![in_play(2, "budew")];
        let legal = vec![
            ActionDto::EndTurn,
            ActionDto::PlayCard {
                entity_id: EntityId(200),
                target: Some(ActionTarget::OwnActive),
            },
        ];
        let a = bot
            .choose_action(&request_with(state_with("me", me), legal), &mut r)
            .expect("action");
        assert!(
            matches!(
                a,
                ActionDto::PlayCard {
                    target: Some(ActionTarget::OwnActive),
                    ..
                }
            ),
            "にげエネをバトル場に付ける (got {a:?})"
        );
    }

    #[test]
    fn does_not_retreat_when_active_can_phantom_dive() {
        // バトル場 dragapult-ex (炎+超) → にげず攻撃 (この legal では UseAttack 無し→EndTurn)。
        let mut bot = DragapultTakeuchiBot::new(CardFacts::new());
        let mut r = rng();
        let mut me = crate::bots::testutil::empty_player();
        me.active = Some(in_play_with_energy(
            1,
            "dragapult-ex",
            &["fire-energy", "psychic-energy"],
        ));
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
        assert_eq!(a, ActionDto::EndTurn, "撃てるならにげない");
    }

    #[test]
    fn plays_pokemon_goods_poke_pad_first() {
        // 手札に poke-pad と poffin → poke-pad を優先 (③ グッズ)。
        let mut bot = DragapultTakeuchiBot::new(CardFacts::new());
        let mut r = rng();
        let mut me = player_with_hand(&[(40, "buddy-buddy-poffin"), (41, "poke-pad")]);
        me.active = Some(in_play(1, "drakloak"));
        let legal = vec![
            ActionDto::EndTurn,
            ActionDto::PlayCard {
                entity_id: EntityId(40),
                target: None,
            },
            ActionDto::PlayCard {
                entity_id: EntityId(41),
                target: None,
            },
        ];
        let a = bot
            .choose_action(&request_with(state_with("me", me), legal), &mut r)
            .expect("action");
        assert!(matches!(a, ActionDto::PlayCard { entity_id, .. } if entity_id == EntityId(41)));
    }

    /// face-down のサイド (`card=None`) を n 枚。`p_boss_win` 等の残サイド数判定用。
    fn prizes_of(n: usize) -> Vec<EntityDto> {
        (0..n)
            .map(|i| EntityDto {
                entity_id: 800 + u32::try_from(i).unwrap_or(0),
                card: None,
            })
            .collect()
    }

    /// pending_search を立て、state 付き ChooseFromZone (deck 探索) を投げて選択を得る。
    fn fetch_with(
        bot: &mut DragapultTakeuchiBot,
        source: &str,
        state: StateDto,
        opts: Vec<EntityDto>,
        min: u8,
        max: u8,
    ) -> Vec<u32> {
        bot.pending_search = Some(source.to_string());
        let mut p = prompt_with_state(
            PromptDto::ChooseFromZone {
                zone: "my_deck".into(),
                options: opts,
            },
            state,
        );
        p.min = min;
        p.max = max;
        let mut r = rng();
        bot.choose_prompt(&p, &mut r).selected
    }

    /// バトル場に炎+超付き dragapult-ex を置いた自分 state (= `P_phantomReadyActive`、budew 条件 false)。
    fn phantom_ready_me() -> PlayerView {
        let mut me = crate::bots::testutil::empty_player();
        me.active = Some(in_play_with_energy(
            1,
            "dragapult-ex",
            &["fire-energy", "psychic-energy"],
        ));
        me.prizes = prizes_of(6);
        me
    }

    #[test]
    fn poke_pad_next_tier_dreepy_when_phantom_ready_and_no_dreepy() {
        // budew 条件 false (バトル場が撃てる dragapult-ex) かつ 場に dreepy 不在
        // → 次 tier 1 番「dreepy 不在 → dreepy」。
        let mut bot = DragapultTakeuchiBot::new(CardFacts::new());
        let opts = vec![
            EntityDto {
                entity_id: 51,
                card: Some("dreepy".into()),
            },
            EntityDto {
                entity_id: 52,
                card: Some("drakloak".into()),
            },
        ];
        let sel = fetch_with(
            &mut bot,
            "poke-pad",
            state_with("me", phantom_ready_me()),
            opts,
            1,
            1,
        );
        assert_eq!(sel, vec![51], "次 tier の dreepy を取る");
    }

    #[test]
    fn poke_pad_excludes_summoning_sick_dreepy_for_drakloak() {
        // 「進化できる dreepy」= 召喚酔いでない (turn_in_play>=1)。この番出したばかり (=0) は除外。
        // budew 条件を false にするためバトル場を撃てる dragapult-ex に。
        let mut bot = DragapultTakeuchiBot::new(CardFacts::new());
        let mut me = phantom_ready_me();
        // ベンチに召喚酔いの dreepy (turn_in_play=0) のみ → 進化できる dreepy 無し。
        let mut sick = in_play(2, "dreepy");
        sick.turn_in_play = 0;
        me.bench = vec![sick];
        assert!(!has_evolvable_dreepy(&state_with("me", me.clone())));
        // poke-pad: drakloak は最優先 tier に入らない (dreepy 不在扱い) → 次 tier dreepy。
        let opts = vec![
            EntityDto {
                entity_id: 51,
                card: Some("dreepy".into()),
            },
            EntityDto {
                entity_id: 52,
                card: Some("drakloak".into()),
            },
        ];
        let sel = fetch_with(&mut bot, "poke-pad", state_with("me", me), opts, 1, 1);
        assert_eq!(
            sel,
            vec![51],
            "召喚酔い dreepy では drakloak を最優先化しない"
        );

        // turn_in_play>=1 の dreepy → 進化できる扱い。
        let mut me2 = phantom_ready_me();
        let mut ok = in_play(2, "dreepy");
        ok.turn_in_play = 1;
        me2.bench = vec![ok];
        assert!(has_evolvable_dreepy(&state_with("me", me2)));
    }

    #[test]
    fn poke_pad_top_tier_budew_when_phantom_unlikely() {
        // バトル場が撃てない (drakloak) → P_phantomUnlikely → budew 条件成立 → 最優先 budew。
        // dreepy も場にいない (drakloak 最優先化させない) ので最優先 tier は budew 単独。
        let mut bot = DragapultTakeuchiBot::new(CardFacts::new());
        let mut me = crate::bots::testutil::empty_player();
        me.active = Some(in_play(1, "drakloak"));
        me.prizes = prizes_of(6);
        let opts = vec![
            EntityDto {
                entity_id: 50,
                card: Some("budew".into()),
            },
            EntityDto {
                entity_id: 51,
                card: Some("dreepy".into()),
            },
        ];
        let sel = fetch_with(&mut bot, "poke-pad", state_with("me", me), opts, 1, 1);
        assert_eq!(sel, vec![50], "スボミー条件で budew 最優先");
    }

    #[test]
    fn poke_pad_falls_through_when_top_tier_absent_in_deck() {
        // 優先順位フォールスルー: スボミー条件成立 (最優先 budew) でも、山札に budew が無ければ
        // 次 tier (場に dreepy 不在 → dreepy) に落ちて dreepy を取る。0 枚にはしない。
        let mut bot = DragapultTakeuchiBot::new(CardFacts::new());
        let mut me = crate::bots::testutil::empty_player();
        me.active = Some(in_play(1, "drakloak")); // 撃てない → P_phantomUnlikely → budew 条件成立
        me.prizes = prizes_of(6); // 場に dreepy 不在 → 次 tier 1 番「dreepy」
                                  // 山札候補に budew は無い。dreepy / dusclops はある。
        let opts = vec![
            EntityDto {
                entity_id: 70,
                card: Some("dusclops".into()),
            },
            EntityDto {
                entity_id: 71,
                card: Some("dreepy".into()),
            },
        ];
        let sel = fetch_with(&mut bot, "poke-pad", state_with("me", me), opts, 0, 1);
        assert_eq!(
            sel,
            vec![71],
            "budew 不在 → 次 tier の dreepy に落ちる (対象なしにしない)"
        );
    }

    #[test]
    fn poffin_takes_two_dreepy_when_phantom_ready() {
        // budew 条件 false → poffin 優先 dreepy。候補 [dreepy,dreepy,duskull]/max2 → dreepy 2 枚。
        let mut bot = DragapultTakeuchiBot::new(CardFacts::new());
        let opts = vec![
            EntityDto {
                entity_id: 60,
                card: Some("dreepy".into()),
            },
            EntityDto {
                entity_id: 61,
                card: Some("dreepy".into()),
            },
            EntityDto {
                entity_id: 62,
                card: Some("duskull".into()),
            },
        ];
        let sel = fetch_with(
            &mut bot,
            "buddy-buddy-poffin",
            state_with("me", phantom_ready_me()),
            opts,
            0,
            2,
        );
        assert_eq!(sel, vec![60, 61]);
    }

    #[test]
    fn ultra_ball_top_tier_meowth_when_no_supporter() {
        // 手札にサポート無し → 最優先 tier meowth-ex。候補 [meowth-ex, drakloak]/max1 → meowth-ex。
        let mut bot = DragapultTakeuchiBot::new(CardFacts::new());
        let opts = vec![
            EntityDto {
                entity_id: 70,
                card: Some("meowth-ex".into()),
            },
            EntityDto {
                entity_id: 71,
                card: Some("drakloak".into()),
            },
        ];
        let sel = fetch_with(
            &mut bot,
            "ultra-ball",
            state_with("me", phantom_ready_me()),
            opts,
            1,
            1,
        );
        assert_eq!(sel, vec![70]);
    }

    #[test]
    fn ultra_ball_next_tier_dragapult_when_energized_drakloak() {
        // 最優先 tier 空 (手札に使えるサポート lillie あり / budew 条件 false / P_bossWin false) かつ
        // エネ付き drakloak がベンチ → 次 tier 2 番「エネ付き drakloak → dragapult-ex」。
        let mut bot = DragapultTakeuchiBot::new(CardFacts::new());
        let mut me = phantom_ready_me();
        me.hand = vec![EntityDto {
            entity_id: 99,
            card: Some("lillie-s-determination".into()),
        }];
        me.bench = vec![in_play_with_energy(2, "drakloak", &["fire-energy"])];
        let opts = vec![
            EntityDto {
                entity_id: 80,
                card: Some("dragapult-ex".into()),
            },
            EntityDto {
                entity_id: 81,
                card: Some("dusclops".into()),
            },
        ];
        let sel = fetch_with(&mut bot, "ultra-ball", state_with("me", me), opts, 1, 1);
        assert_eq!(
            sel,
            vec![80],
            "エネ付き drakloak がいるので dragapult-ex を取る"
        );
    }

    #[test]
    fn ultra_ball_trash_picks_two_random_from_hand() {
        // 候補が全て手札 (= トラッシュ cost) → 手札からランダム2枚 (GAP-3)。
        let mut bot = DragapultTakeuchiBot::new(CardFacts::new());
        let mut r = rng();
        let mut me = player_with_hand(&[(40, "fire-energy"), (41, "poke-pad"), (42, "dreepy")]);
        me.prizes = prizes_of(6);
        let opts = vec![
            EntityDto {
                entity_id: 40,
                card: Some("fire-energy".into()),
            },
            EntityDto {
                entity_id: 41,
                card: Some("poke-pad".into()),
            },
            EntityDto {
                entity_id: 42,
                card: Some("dreepy".into()),
            },
        ];
        let mut p = prompt_with_state(
            PromptDto::ChooseFromZone {
                zone: "my_deck".into(),
                options: opts,
            },
            state_with("me", me),
        );
        p.min = 2;
        p.max = 2;
        let sel = bot.choose_prompt(&p, &mut r).selected;
        assert_eq!(sel.len(), 2, "コスト2枚をトラッシュ");
        for e in &sel {
            assert!([40, 41, 42].contains(e), "手札の entity から選ぶ");
        }
    }

    #[test]
    fn phantom_predicates_distinguish_active_and_bench() {
        // §3: active が撃てる dragapult-ex なら ready_active / unlikely=false。
        let charged = in_play_with_energy(1, "dragapult-ex", &["fire-energy", "psychic-energy"]);
        let mut me = crate::bots::testutil::empty_player();
        me.active = Some(charged.clone());
        let s = state_with("me", me);
        assert!(p_phantom_ready_active(&s));
        assert!(!p_phantom_ready_bench(&s));
        assert!(!p_phantom_unlikely(&s));
        // ベンチのみ撃てる → ready_bench だが unlikely=true (active が撃てない)。
        let mut me2 = crate::bots::testutil::empty_player();
        me2.active = Some(in_play(9, "drakloak"));
        me2.bench = vec![charged];
        let s2 = state_with("me", me2);
        assert!(!p_phantom_ready_active(&s2));
        assert!(p_phantom_ready_bench(&s2));
        assert!(p_phantom_unlikely(&s2));
    }

    #[test]
    fn is_first_turn_going_second_only_on_second_player_first_turn() {
        let mut bot = DragapultTakeuchiBot::new(CardFacts::new());
        bot.am_i_first = Some(false);
        let mut s = state_with("me", crate::bots::testutil::empty_player());
        s.turn = 2; // 後攻の最初の番。
        assert!(bot.is_first_turn_going_second(&s));
        s.turn = 4; // 後攻の2番目の番。
        assert!(!bot.is_first_turn_going_second(&s));
        bot.am_i_first = Some(true);
        s.turn = 1;
        assert!(!bot.is_first_turn_going_second(&s));
    }

    #[test]
    fn p_boss_win_true_on_last_prize_or_lethal_phantom() {
        // 自分サイド残=1 → 無条件 true。
        let mut me = phantom_ready_me();
        me.prizes = prizes_of(1);
        let s = state_with("me", me);
        assert!(p_boss_win(&s));
        // サイド2残・相手 active ex 残HP≤200 1匹 → 200 で 2 prize 取れる → true。
        let mut me2 = phantom_ready_me();
        me2.prizes = prizes_of(2);
        let mut s2 = state_with("me", me2);
        let mut ex = in_play(20, "dragapult-ex");
        ex.hp_max = 200;
        s2.opp.active = Some(ex);
        assert!(p_boss_win(&s2));
        // サイド2残・相手が残HP320 の ex のみ → 200 で倒せない → false。
        let mut me3 = phantom_ready_me();
        me3.prizes = prizes_of(2);
        let mut s3 = state_with("me", me3);
        let mut tough = in_play(30, "dragapult-ex");
        tough.hp_max = 320;
        s3.opp.active = Some(tough);
        assert!(!p_boss_win(&s3));
    }

    #[test]
    fn refill_own_turn_prefers_budew() {
        // 自分の番の繰り出し (self-KO 後): budew 最優先。
        let mut bot = DragapultTakeuchiBot::new(CardFacts::new());
        let mut r = rng();
        let mut me = crate::bots::testutil::empty_player();
        me.bench = vec![
            in_play_with_energy(2, "dragapult-ex", &["fire-energy", "psychic-energy"]),
            in_play(3, "budew"),
        ];
        let p = prompt_with_state(
            PromptDto::ReplaceActiveAfterKo {
                bench_options: vec![2, 3],
            },
            state_with("me", me),
        );
        let c = bot.choose_prompt(&p, &mut r);
        assert_eq!(c.selected, vec![3], "自分の番は budew を出す");
    }

    #[test]
    fn refill_opp_turn_prefers_phantom_ready_dragapult_after_budew() {
        // 相手の番の繰り出し (相手ワザ KO): budew 不在 → canPhantomDive dragapult-ex。
        let mut bot = DragapultTakeuchiBot::new(CardFacts::new());
        let mut r = rng();
        let mut me = crate::bots::testutil::empty_player();
        me.bench = vec![
            in_play(2, "dreepy"),
            in_play_with_energy(3, "dragapult-ex", &["fire-energy", "psychic-energy"]),
        ];
        let p = prompt_with_state(
            PromptDto::ReplaceActiveAfterKo {
                bench_options: vec![2, 3],
            },
            state_with("opp", me),
        );
        let c = bot.choose_prompt(&p, &mut r);
        assert_eq!(
            c.selected,
            vec![3],
            "相手の番は撃てる dragapult-ex を繰り出す"
        );
    }

    #[test]
    fn u2_basic_order_prefers_budew_over_duskull() {
        // U2 (turn 3 → 2番目の番): dreepy 不在のとき budew > duskull。
        let mut bot = DragapultTakeuchiBot::new(CardFacts::new());
        let mut r = rng();
        let mut me = player_with_hand(&[(50, "duskull"), (51, "budew")]);
        me.active = Some(in_play(1, "drakloak"));
        let legal = vec![
            ActionDto::EndTurn,
            ActionDto::PlayCard {
                entity_id: EntityId(50),
                target: None,
            },
            ActionDto::PlayCard {
                entity_id: EntityId(51),
                target: None,
            },
        ];
        let a = bot
            .choose_action(&request_on_turn(me, legal, 3), &mut r)
            .expect("action");
        assert!(
            matches!(a, ActionDto::PlayCard { entity_id, .. } if entity_id == EntityId(51)),
            "U2 は budew を先に出す (got {a:?})"
        );
    }

    #[test]
    fn v2_basic_order_prefers_duskull_over_budew() {
        // V2 (turn 5 → 3番目以降): dreepy 不在のとき duskull > budew。
        let mut bot = DragapultTakeuchiBot::new(CardFacts::new());
        let mut r = rng();
        let mut me = player_with_hand(&[(50, "budew"), (51, "duskull")]);
        me.active = Some(in_play(1, "drakloak"));
        let legal = vec![
            ActionDto::EndTurn,
            ActionDto::PlayCard {
                entity_id: EntityId(50),
                target: None,
            },
            ActionDto::PlayCard {
                entity_id: EntityId(51),
                target: None,
            },
        ];
        let a = bot
            .choose_action(&request_on_turn(me, legal, 5), &mut r)
            .expect("action");
        assert!(
            matches!(a, ActionDto::PlayCard { entity_id, .. } if entity_id == EntityId(51)),
            "V2 は duskull を先に出す (got {a:?})"
        );
    }

    #[test]
    fn t1_attaches_psychic_to_active_dreepy() {
        // T1.4 (先攻初手): バトル場 dreepy に超優先で付ける (ベンチ dreepy より active を優先)。
        let mut bot = DragapultTakeuchiBot::new(CardFacts::new());
        bot.am_i_first = Some(true);
        let mut r = rng();
        let mut me = player_with_hand(&[(200, "psychic-energy")]);
        me.active = Some(in_play(1, "dreepy"));
        me.bench = vec![in_play(2, "dreepy")];
        let mut s = state_with("me", me);
        s.turn = 1; // 先攻の最初の番。
        let legal = vec![
            ActionDto::EndTurn,
            ActionDto::PlayCard {
                entity_id: EntityId(200),
                target: Some(ActionTarget::OwnActive),
            },
            ActionDto::PlayCard {
                entity_id: EntityId(200),
                target: Some(ActionTarget::OwnBench { index: 0 }),
            },
        ];
        let a = bot
            .choose_action(&request_with(s, legal), &mut r)
            .expect("action");
        assert!(
            matches!(
                a,
                ActionDto::PlayCard {
                    target: Some(ActionTarget::OwnActive),
                    ..
                }
            ),
            "バトル場 dreepy に付ける (got {a:?})"
        );
    }

    #[test]
    fn plays_unfair_stamp_when_ko_last_turn() {
        // 前の番に自ポケがきぜつ (had_ko_last_turn) & 手札に unfair-stamp → 使う。
        let mut bot = DragapultTakeuchiBot::new(CardFacts::new());
        let mut r = rng();
        let mut me = player_with_hand(&[(90, "unfair-stamp")]);
        me.active = Some(in_play(1, "drakloak"));
        me.had_ko_last_turn = true;
        let legal = vec![
            ActionDto::EndTurn,
            ActionDto::PlayCard {
                entity_id: EntityId(90),
                target: None,
            },
        ];
        let a = bot
            .choose_action(&request_on_turn(me, legal, 5), &mut r)
            .expect("action");
        assert!(matches!(a, ActionDto::PlayCard { entity_id, .. } if entity_id == EntityId(90)));
    }

    #[test]
    fn no_unfair_stamp_without_ko_last_turn() {
        // 前番きぜつ無し → unfair-stamp は使わない。
        let mut bot = DragapultTakeuchiBot::new(CardFacts::new());
        let mut r = rng();
        let mut me = player_with_hand(&[(90, "unfair-stamp")]);
        me.active = Some(in_play(1, "drakloak")); // ワザ無し → 妨害まで到達。
        let legal = vec![
            ActionDto::EndTurn,
            ActionDto::PlayCard {
                entity_id: EntityId(90),
                target: None,
            },
        ];
        let a = bot
            .choose_action(&request_on_turn(me, legal, 5), &mut r)
            .expect("action");
        assert_eq!(a, ActionDto::EndTurn);
    }

    #[test]
    fn plays_watchtower_when_meowth_used_and_opp_has_no_meowth() {
        // meowth-ex が自分の場 (おくのて使用済) & 相手 meowth-ex 不在 & 手札 watchtower → 出す。
        let mut bot = DragapultTakeuchiBot::new(CardFacts::new());
        let mut r = rng();
        let mut me = player_with_hand(&[(91, "team-rocket-s-watchtower")]);
        me.active = Some(in_play(1, "drakloak"));
        me.bench = vec![in_play(2, "meowth-ex")];
        let legal = vec![
            ActionDto::EndTurn,
            ActionDto::PlayCard {
                entity_id: EntityId(91),
                target: None,
            },
        ];
        let a = bot
            .choose_action(&request_on_turn(me, legal, 5), &mut r)
            .expect("action");
        assert!(matches!(a, ActionDto::PlayCard { entity_id, .. } if entity_id == EntityId(91)));
    }

    #[test]
    fn no_watchtower_when_opp_has_meowth() {
        // 相手が meowth-ex を場に出している → watchtower は出さない。
        let mut bot = DragapultTakeuchiBot::new(CardFacts::new());
        let mut r = rng();
        let mut me = player_with_hand(&[(91, "team-rocket-s-watchtower")]);
        me.active = Some(in_play(1, "drakloak"));
        me.bench = vec![in_play(2, "meowth-ex")];
        let mut s = state_with("me", me);
        s.turn = 5;
        s.opp.bench = vec![in_play(50, "meowth-ex")];
        let legal = vec![
            ActionDto::EndTurn,
            ActionDto::PlayCard {
                entity_id: EntityId(91),
                target: None,
            },
        ];
        let a = bot
            .choose_action(&request_with(s, legal), &mut r)
            .expect("action");
        assert_eq!(a, ActionDto::EndTurn);
    }

    #[test]
    fn rare_candy_case1_targets_energized_dreepy() {
        // 炎+超 dreepy が場・手札に dragapult-ex・場に dragapult-ex 不在 → rare-candy を出し、
        // 対象 (ChooseTargetPokemon) は その dreepy。
        let mut bot = DragapultTakeuchiBot::new(CardFacts::new());
        let mut r = rng();
        let mut me = player_with_hand(&[(70, "rare-candy"), (71, "dragapult-ex")]);
        me.active = Some(in_play(1, "drakloak"));
        me.bench = vec![in_play_with_energy(
            2,
            "dreepy",
            &["fire-energy", "psychic-energy"],
        )];
        let legal = vec![
            ActionDto::EndTurn,
            ActionDto::PlayCard {
                entity_id: EntityId(70),
                target: None,
            },
        ];
        let a = bot
            .choose_action(&request_on_turn(me, legal, 5), &mut r)
            .expect("action");
        assert!(
            matches!(a, ActionDto::PlayCard { entity_id, .. } if entity_id == EntityId(70)),
            "rare-candy を出す (got {a:?})"
        );
        assert!(bot.pending_rare_candy, "対象は §5.4 で選ぶフラグ");
        // 対象選択: 炎+超 dreepy(2) を狙う。
        let p = prompt_with_state(PromptDto::ChooseTargetPokemon { targets: vec![2] }, {
            let mut me2 = crate::bots::testutil::empty_player();
            me2.active = Some(in_play(1, "drakloak"));
            me2.bench = vec![in_play_with_energy(
                2,
                "dreepy",
                &["fire-energy", "psychic-energy"],
            )];
            me2.hand = vec![EntityDto {
                entity_id: 71,
                card: Some("dragapult-ex".into()),
            }];
            state_with("me", me2)
        });
        let c = bot.choose_prompt(&p, &mut r);
        assert_eq!(c.selected, vec![2], "炎+超 dreepy を進化対象に");
    }

    #[test]
    fn rare_candy_not_used_when_dragapult_already_in_play() {
        // 場に dragapult-ex がいる (ドロンチ止め) & duskull/dusknoir も無し → rare-candy 使わず。
        let mut bot = DragapultTakeuchiBot::new(CardFacts::new());
        let mut r = rng();
        let mut me = player_with_hand(&[(70, "rare-candy"), (71, "dragapult-ex")]);
        me.active = Some(in_play(1, "dragapult-ex"));
        me.bench = vec![in_play_with_energy(
            2,
            "dreepy",
            &["fire-energy", "psychic-energy"],
        )];
        let legal = vec![
            ActionDto::EndTurn,
            ActionDto::PlayCard {
                entity_id: EntityId(70),
                target: None,
            },
        ];
        let a = bot
            .choose_action(&request_on_turn(me, legal, 5), &mut r)
            .expect("action");
        assert_eq!(a, ActionDto::EndTurn, "ドラパルト在 → rare-candy 使わない");
    }

    #[test]
    fn rare_candy_case2_targets_duskull() {
        // case1 不成立・duskull 場 & 手札 dusknoir → rare-candy で duskull を対象。
        let mut bot = DragapultTakeuchiBot::new(CardFacts::new());
        let mut r = rng();
        let mut me = player_with_hand(&[(70, "rare-candy"), (71, "dusknoir")]);
        me.active = Some(in_play(1, "drakloak"));
        me.bench = vec![in_play(2, "duskull")];
        let legal = vec![
            ActionDto::EndTurn,
            ActionDto::PlayCard {
                entity_id: EntityId(70),
                target: None,
            },
        ];
        let a = bot
            .choose_action(&request_on_turn(me, legal, 5), &mut r)
            .expect("action");
        assert!(matches!(a, ActionDto::PlayCard { entity_id, .. } if entity_id == EntityId(70)));
        let p = prompt_with_state(PromptDto::ChooseTargetPokemon { targets: vec![2] }, {
            let mut me2 = crate::bots::testutil::empty_player();
            me2.active = Some(in_play(1, "drakloak"));
            me2.bench = vec![in_play(2, "duskull")];
            me2.hand = vec![EntityDto {
                entity_id: 71,
                card: Some("dusknoir".into()),
            }];
            state_with("me", me2)
        });
        let c = bot.choose_prompt(&p, &mut r);
        assert_eq!(c.selected, vec![2], "duskull を進化対象に");
    }

    #[test]
    fn crispin_attaches_missing_type_to_priority_target() {
        // crispin 3 段: エネ search → keep-in-hand → attach-target。装着先優先 dragapult-ex>...、
        // バトル場 dragapult-ex が炎のみ → 超を装着 (!energyOverflow)、装着先は その dragapult-ex。
        let mut bot = DragapultTakeuchiBot::new(CardFacts::new());
        bot.crispin_stage = CrispinStage::None;
        bot.crispin_attach_pri = CRISPIN_ATTACH_PRIORITY_DIRECT;
        let mut r = rng();
        // 盤面: バトル場 dragapult-ex (炎のみ) / ベンチ drakloak (エネ無し)。
        let mut me = crate::bots::testutil::empty_player();
        me.active = Some(in_play_with_energy(1, "dragapult-ex", &["fire-energy"]));
        me.bench = vec![in_play(2, "drakloak")];
        let state = state_with("me", me);

        // 段1: エネ search (deck の 炎/超)。pending_search="crispin" を立てて投げる。
        bot.pending_search = Some("crispin".to_string());
        let opts_deck = vec![
            EntityDto {
                entity_id: 90,
                card: Some("fire-energy".into()),
            },
            EntityDto {
                entity_id: 91,
                card: Some("psychic-energy".into()),
            },
        ];
        let mut p1 = prompt_with_state(
            PromptDto::ChooseFromZone {
                zone: "deck".into(),
                options: opts_deck.clone(),
            },
            state.clone(),
        );
        p1.min = 0;
        p1.max = 2;
        let c1 = bot.choose_prompt(&p1, &mut r);
        assert_eq!(c1.selected.len(), 2, "超+炎 を取る");
        assert_eq!(bot.crispin_stage, CrispinStage::KeepInHand);

        // 段2: keep-in-hand (手札の 炎/超 から装着する 1 枚)。超 (91) を装着すべき (active が炎のみ)。
        let opts_hand = vec![
            EntityDto {
                entity_id: 90,
                card: Some("fire-energy".into()),
            },
            EntityDto {
                entity_id: 91,
                card: Some("psychic-energy".into()),
            },
        ];
        // 手札に 炎/超 を持たせた state (is_hand_discard 判定は crispin_stage が優先するので不要だが整合)。
        let mut me2 = crate::bots::testutil::empty_player();
        me2.active = Some(in_play_with_energy(1, "dragapult-ex", &["fire-energy"]));
        me2.bench = vec![in_play(2, "drakloak")];
        me2.hand = opts_hand.clone();
        let mut p2 = prompt_with_state(
            PromptDto::ChooseFromZone {
                zone: "deck".into(),
                options: opts_hand,
            },
            state_with("me", me2),
        );
        p2.min = 1;
        p2.max = 1;
        let c2 = bot.choose_prompt(&p2, &mut r);
        assert_eq!(c2.selected, vec![91], "超エネを装着 (炎炎を作らない)");
        assert_eq!(bot.crispin_stage, CrispinStage::AttachTarget);
        assert_eq!(
            bot.crispin_attach_target,
            Some(1),
            "装着先は active dragapult-ex"
        );

        // 段3: attach-target (own ポケ)。計画した dragapult-ex(1) を選ぶ。
        let opts_mon = vec![
            EntityDto {
                entity_id: 1,
                card: Some("dragapult-ex".into()),
            },
            EntityDto {
                entity_id: 2,
                card: Some("drakloak".into()),
            },
        ];
        let p3 = prompt_with_state(
            PromptDto::ChooseFromZone {
                zone: "in_play".into(),
                options: opts_mon,
            },
            state,
        );
        let c3 = bot.choose_prompt(&p3, &mut r);
        assert_eq!(c3.selected, vec![1], "装着先 = dragapult-ex");
        assert_eq!(bot.crispin_stage, CrispinStage::None);
    }

    #[test]
    fn meowth_okunote_refill_when_no_usable_supporter() {
        // 手札にサポート無し & 手札に meowth-ex → meowth-ex を場に出し pending_search="okunote"。
        let mut bot = DragapultTakeuchiBot::new(CardFacts::new());
        let mut r = rng();
        let mut me = player_with_hand(&[(85, "meowth-ex")]);
        me.active = Some(in_play(1, "drakloak"));
        let legal = vec![
            ActionDto::EndTurn,
            ActionDto::PlayCard {
                entity_id: EntityId(85),
                target: None,
            },
        ];
        let a = bot
            .choose_action(&request_on_turn(me, legal, 5), &mut r)
            .expect("action");
        assert!(
            matches!(a, ActionDto::PlayCard { entity_id, .. } if entity_id == EntityId(85)),
            "meowth-ex を場に出す (got {a:?})"
        );
        assert_eq!(bot.pending_search.as_deref(), Some("okunote"));
    }

    #[test]
    fn supporter_refill_needed_logic() {
        // 手札にサポート無し → 必要。lillie あり → 不要。boss のみ & !P_bossUse → 必要。
        let mut me = crate::bots::testutil::empty_player();
        me.active = Some(in_play(1, "drakloak"));
        me.prizes = prizes_of(6);
        assert!(supporter_refill_needed(&state_with("me", me.clone())));
        let mut me_l = me.clone();
        me_l.hand = vec![EntityDto {
            entity_id: 1,
            card: Some("lillie-s-determination".into()),
        }];
        assert!(!supporter_refill_needed(&state_with("me", me_l)));
        let mut me_b = me.clone();
        me_b.hand = vec![EntityDto {
            entity_id: 2,
            card: Some("boss-s-orders".into()),
        }];
        // P_bossUse 不成立 (phantom 不可) → boss のみは「使えない」扱い → 必要。
        assert!(supporter_refill_needed(&state_with("me", me_b)));
    }

    #[test]
    fn okunote_priority_crispin_when_dragapult_needs_energy() {
        // dragapult-ex が場 (エネ不足) → crispin が先頭。
        let mut me = crate::bots::testutil::empty_player();
        me.active = Some(in_play(1, "dragapult-ex")); // エネ無し
        me.prizes = prizes_of(6);
        let pri = okunote_supporter_priority(&state_with("me", me));
        assert_eq!(pri.first(), Some(&"crispin"));
        // dragapult-ex なし → crispin 無し、lillie が既定。
        let mut me2 = crate::bots::testutil::empty_player();
        me2.active = Some(in_play(1, "drakloak"));
        me2.prizes = prizes_of(6);
        let pri2 = okunote_supporter_priority(&state_with("me", me2));
        assert!(!pri2.contains(&"crispin"));
        assert_eq!(pri2.last(), Some(&"lillie-s-determination"));
    }

    #[test]
    fn jamming_tower_overrides_watchtower_for_okunote_boss() {
        // V2: 監視塔が場 & 補充必要 & P_bossUse & meowth-ex 出せる & 手札 jamming-tower →
        // jamming-tower を出して上書き (特性復活)。
        let mut bot = DragapultTakeuchiBot::new(CardFacts::new());
        let mut r = rng();
        // P_bossUse 成立のため撃てる dragapult-ex をバトル場に、相手 active ex 残HP≤200。
        let mut me = phantom_ready_me();
        me.hand = vec![
            EntityDto {
                entity_id: 85,
                card: Some("meowth-ex".into()),
            },
            EntityDto {
                entity_id: 86,
                card: Some("jamming-tower".into()),
            },
        ];
        let mut s = state_with("me", me);
        s.turn = 5; // V (3番目以降)
        s.stadium = Some(EntityDto {
            entity_id: 70,
            card: Some("team-rocket-s-watchtower".into()),
        });
        let mut ex = in_play(40, "dragapult-ex");
        ex.hp_max = 200;
        s.opp.active = Some(ex);
        let legal = vec![
            ActionDto::EndTurn,
            ActionDto::PlayCard {
                entity_id: EntityId(85),
                target: None,
            },
            ActionDto::PlayCard {
                entity_id: EntityId(86),
                target: None,
            },
        ];
        let a = bot
            .choose_action(&request_with(s, legal), &mut r)
            .expect("action");
        assert!(
            matches!(a, ActionDto::PlayCard { entity_id, .. } if entity_id == EntityId(86)),
            "jamming-tower を出して上書き (got {a:?})"
        );
    }

    #[test]
    fn t1_okunote_refills_lillie_even_with_crispin_in_hand() {
        // 最初の番 (T1.3): 手札に crispin があっても lillie が無ければ おくのてキャッチで補充。
        let mut bot = DragapultTakeuchiBot::new(CardFacts::new());
        let mut r = rng();
        let mut me = player_with_hand(&[(85, "meowth-ex"), (86, "crispin")]);
        me.active = Some(in_play(1, "drakloak"));
        let legal = vec![
            ActionDto::EndTurn,
            ActionDto::PlayCard {
                entity_id: EntityId(85),
                target: None,
            },
            ActionDto::PlayCard {
                entity_id: EntityId(86),
                target: None,
            },
        ];
        let a = bot
            .choose_action(&request_on_turn(me, legal, 1), &mut r)
            .expect("action");
        assert!(
            matches!(a, ActionDto::PlayCard { entity_id, .. } if entity_id == EntityId(85)),
            "lillie 不在なら crispin があっても meowth-ex を出す (got {a:?})"
        );
    }

    #[test]
    fn t1_no_okunote_refill_when_lillie_in_hand() {
        // 最初の番に lillie が手札にあれば補充しない。
        let mut bot = DragapultTakeuchiBot::new(CardFacts::new());
        let mut r = rng();
        let mut me = player_with_hand(&[(85, "meowth-ex"), (87, "lillie-s-determination")]);
        me.active = Some(in_play(1, "drakloak"));
        let legal = vec![
            ActionDto::EndTurn,
            ActionDto::PlayCard {
                entity_id: EntityId(85),
                target: None,
            },
            ActionDto::PlayCard {
                entity_id: EntityId(87),
                target: None,
            },
        ];
        let a = bot
            .choose_action(&request_on_turn(me, legal, 1), &mut r)
            .expect("action");
        assert_eq!(
            a,
            ActionDto::EndTurn,
            "lillie 在 → 補充せず (T1 は lillie も使わず保持)"
        );
    }

    #[test]
    fn no_jamming_override_when_okunote_would_fetch_crispin() {
        // 監視塔が場でも、dragapult-ex がエネ未充足で okunote が crispin を取る局面では
        // jamming-tower を出さず (boss を加えない)、そのまま meowth-ex を出す。
        let mut bot = DragapultTakeuchiBot::new(CardFacts::new());
        let mut r = rng();
        let mut me = crate::bots::testutil::empty_player();
        me.active = Some(in_play(1, "dragapult-ex")); // エネ未充足 → crispin が okunote 先頭
        me.hand = vec![
            EntityDto {
                entity_id: 85,
                card: Some("meowth-ex".into()),
            },
            EntityDto {
                entity_id: 86,
                card: Some("jamming-tower".into()),
            },
        ];
        me.prizes = prizes_of(6);
        let mut s = state_with("me", me);
        s.turn = 5;
        s.stadium = Some(EntityDto {
            entity_id: 70,
            card: Some("team-rocket-s-watchtower".into()),
        });
        let legal = vec![
            ActionDto::EndTurn,
            ActionDto::PlayCard {
                entity_id: EntityId(85),
                target: None,
            },
            ActionDto::PlayCard {
                entity_id: EntityId(86),
                target: None,
            },
        ];
        let a = bot
            .choose_action(&request_with(s, legal), &mut r)
            .expect("action");
        assert!(
            matches!(a, ActionDto::PlayCard { entity_id, .. } if entity_id == EntityId(85)),
            "crispin 取得局面では jamming を出さず meowth-ex (got {a:?})"
        );
    }

    #[test]
    fn crispin_stage_not_set_when_only_one_energy_fetched() {
        // crispin エネ search が 1 枚しか取れない → keep-in-hand prompt が来ないので
        // crispin_stage を進めない (stale 化による別 prompt 誤消費を防ぐ)。
        let mut bot = DragapultTakeuchiBot::new(CardFacts::new());
        bot.pending_search = Some("crispin".to_string());
        let mut r = rng();
        let opts = vec![EntityDto {
            entity_id: 90,
            card: Some("fire-energy".into()),
        }];
        let mut p = prompt_with_state(
            PromptDto::ChooseFromZone {
                zone: "deck".into(),
                options: opts,
            },
            state_with("me", crate::bots::testutil::empty_player()),
        );
        p.min = 0;
        p.max = 2;
        let c = bot.choose_prompt(&p, &mut r);
        assert_eq!(c.selected, vec![90]);
        assert_eq!(
            bot.crispin_stage,
            CrispinStage::None,
            "1 枚なら stage を進めない"
        );
    }

    #[test]
    fn no_jamming_override_without_watchtower() {
        // 監視塔が場に無ければ jamming-tower 上書きはしない (meowth okunote を直接行う)。
        let mut bot = DragapultTakeuchiBot::new(CardFacts::new());
        let mut r = rng();
        let mut me = phantom_ready_me();
        me.hand = vec![
            EntityDto {
                entity_id: 85,
                card: Some("meowth-ex".into()),
            },
            EntityDto {
                entity_id: 86,
                card: Some("jamming-tower".into()),
            },
        ];
        let mut s = state_with("me", me);
        s.turn = 5;
        // stadium 無し。
        let mut ex = in_play(40, "dragapult-ex");
        ex.hp_max = 200;
        s.opp.active = Some(ex);
        let legal = vec![
            ActionDto::EndTurn,
            ActionDto::PlayCard {
                entity_id: EntityId(85),
                target: None,
            },
            ActionDto::PlayCard {
                entity_id: EntityId(86),
                target: None,
            },
        ];
        let a = bot
            .choose_action(&request_with(s, legal), &mut r)
            .expect("action");
        // 監視塔が無いので jamming は出さず、meowth-ex を場に出す (おくのて補充)。
        assert!(
            matches!(a, ActionDto::PlayCard { entity_id, .. } if entity_id == EntityId(85)),
            "監視塔が無ければ meowth-ex を出す (got {a:?})"
        );
    }

    #[test]
    fn no_recon_directive_falls_back() {
        // drakloak が居ない (UseAbility なし) → fallback (random) で合法手を返す。
        let mut bot = DragapultTakeuchiBot::new(CardFacts::new());
        let mut r = rng();
        let me = PlayerView {
            active: Some(in_play(1, "budew")),
            ..crate::bots::testutil::empty_player()
        };
        let legal = vec![ActionDto::EndTurn];
        let req = request_with(state_with("me", me), legal);
        let a = bot.choose_action(&req, &mut r).expect("action");
        assert_eq!(a, ActionDto::EndTurn);
    }

    #[test]
    fn delegates_to_random_for_now() {
        let mut bot = DragapultTakeuchiBot::new(CardFacts::new());
        let legal = vec![ActionDto::EndTurn, ActionDto::Concede];
        let mut r = rng();
        let a = bot
            .choose_action(&dummy_request(legal.clone()), &mut r)
            .expect("legal");
        assert!(legal.contains(&a));
    }
}
