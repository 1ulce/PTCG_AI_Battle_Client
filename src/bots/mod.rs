//! Dragapult ex 参照 BOT の方策モジュール (engine 非依存)。
//!
//! サーバとは protocol JSON ([`crate::wire`]) だけでやり取りし、カード事実は
//! [`crate::cards::CardFacts`] から引く。各 bot は [`BotPolicy`] を実装する。
//!
//! ## bot の足し方
//!
//! 1. `src/bots/<name>.rs` に [`BotPolicy`] を実装した struct を 1 つ書く
//! 2. [`build`] と [`available`] のレジストリに 1 行ずつ登録する
//!
//! ## 現状の bot
//!
//! - [`random::RandomPolicy`] (`"random"`) — 合法手・選択肢から一様ランダム
//! - [`dragapult_takeuchi::DragapultTakeuchiBot`] (`"dragapult-takeuchi"`) — `decks/dragapult-ex.yaml`
//!   固定戦略・クチート竹内版 (仕様: `docs/bots/dragapult-ex.takeuchi.machine.md`)
//! - [`dragapult_yopifutto::DragapultYopifuttoBot`] (`"dragapult-yopifutto"`) — `decks/dragapult-ex.yaml`
//!   固定戦略・よぴふっと博士版 (仕様: `docs/bots/dragapult-ex.yopifutto.machine.md`)

use crate::cards::CardFacts;
use crate::transport::TransportError;
use crate::wire::action::ActionDto;
use crate::wire::protocol::{PromptMsg, RequestMsg};
use crate::wire::state::{EntityDto, StateDto};

use rand::Rng;
use rand_chacha::ChaCha20Rng;

pub mod dragapult_takeuchi;
pub mod dragapult_yopifutto;
pub mod random;

pub use dragapult_takeuchi::DragapultTakeuchiBot;
pub use dragapult_yopifutto::DragapultYopifuttoBot;
pub use random::RandomPolicy;

/// prompt への応答に必要な構成要素 (`ChoiceMsg` に組み立てる素材)。
pub struct PromptChoice {
    pub selected: Vec<u32>,
    pub counts: Vec<u8>,
    pub yes: Option<bool>,
    pub branch_index: Option<u8>,
}

/// 内蔵 BOT の意思決定インターフェース。
///
/// `AiTransport` (ptcg-cli) が受信した Request/Prompt をこの trait に委譲して応答を作る。
///
/// `Send` を要求する: serve の `--vs-bot` モードでは `Box<dyn BotPolicy>` を対戦スレッドへ
/// move するため (各 bot 実装は `CardRegistry` 等の Send なフィールドしか持たない)。
pub trait BotPolicy: Send {
    /// 自分の番の能動アクションを 1 つ選ぶ。
    ///
    /// # Errors
    /// 合法手が空のとき [`TransportError::Unexpected`]。
    fn choose_action(
        &mut self,
        req: &RequestMsg,
        rng: &mut ChaCha20Rng,
    ) -> Result<ActionDto, TransportError>;

    /// 解決中の prompt に応答する。
    fn choose_prompt(&mut self, p: &PromptMsg, rng: &mut ChaCha20Rng) -> PromptChoice;
}

/// 利用可能な bot 名の一覧 (`--bot` のヘルプ・検証用)。
#[must_use]
pub fn available() -> &'static [&'static str] {
    &["random", "dragapult-takeuchi", "dragapult-yopifutto"]
}

/// 名前から bot を構築する。
///
/// `cards` はカード事実 (ex 判定・ワザ index) が必要な bot のために渡す
/// (必要な bot だけが内部で clone する。`random` は無視)。未知の名前なら `None`。
#[must_use]
pub fn build(name: &str, cards: &CardFacts) -> Option<Box<dyn BotPolicy>> {
    match name {
        "random" => Some(Box::new(RandomPolicy)),
        "dragapult-takeuchi" => Some(Box::new(DragapultTakeuchiBot::new(cards.clone()))),
        "dragapult-yopifutto" => Some(Box::new(DragapultYopifuttoBot::new(cards.clone()))),
        _ => None,
    }
}

/// プールから `[min, max]` 枚をランダムに選ぶ (重複なし)。
///
/// ボール系サーチ・ベンチ配置など「N 枚選ぶ」prompt のランダム応答に使う汎用ヘルパー。
#[must_use]
pub fn pick_random_subset(rng: &mut ChaCha20Rng, pool: &[u32], min: u8, max: u8) -> Vec<u32> {
    if pool.is_empty() {
        return Vec::new();
    }
    let lo = usize::from(min).min(pool.len());
    let hi = usize::from(max).min(pool.len()).max(lo);
    let n = if hi == lo { lo } else { rng.gen_range(lo..=hi) };
    let mut indices: Vec<usize> = (0..pool.len()).collect();
    for i in 0..n {
        let j = rng.gen_range(i..indices.len());
        indices.swap(i, j);
    }
    indices.into_iter().take(n).map(|i| pool[i]).collect()
}

// ============================================================================
// 盤面読みヘルパー (ペルソナ非依存、各 bot で共有)
// ============================================================================

/// 自分視点の場 (バトル場 + ベンチ) + 手札から entity の slug を引く。
#[must_use]
pub(crate) fn my_slug_of(state: &StateDto, entity_id: u32) -> Option<&str> {
    if let Some(a) = &state.me.active {
        if a.entity_id == entity_id {
            return a.card.as_deref();
        }
    }
    if let Some(b) = state.me.bench.iter().find(|b| b.entity_id == entity_id) {
        return b.card.as_deref();
    }
    state
        .me
        .hand
        .iter()
        .find(|e| e.entity_id == entity_id)
        .and_then(|e| e.card.as_deref())
}

/// 自分の何番目の手番か (turn は 1始まり・手番毎 +1 → ceil(turn/2))。
/// 1=最初の番 / 2=2番目の番 / ≥3=3番目以降。setup 中 (turn=0) は 0。両ペルソナ共通。
#[must_use]
pub(crate) fn my_turn_number(state: &StateDto) -> u32 {
    state.turn.div_ceil(2)
}

/// slug が「ルールを持つ (ex 等、サイド 2 枚)」ポケモンか (registry の `prize_value` で判定)。
/// 両ペルソナ共通 (registry に無い slug は false)。
#[must_use]
pub(crate) fn slug_is_ex(cards: &CardFacts, slug: &str) -> bool {
    cards.is_ex(slug)
}

/// 自分の場 (バトル場 + ベンチ) にある `slug` のポケモン数。
#[must_use]
pub(crate) fn count_in_play(state: &StateDto, slug: &str) -> usize {
    let active = usize::from(
        state
            .me
            .active
            .as_ref()
            .is_some_and(|p| p.card.as_deref() == Some(slug)),
    );
    let bench = state
        .me
        .bench
        .iter()
        .filter(|p| p.card.as_deref() == Some(slug))
        .count();
    active + bench
}

/// `PlayCard` アクションが指す手札カードの slug (それ以外のアクションは `None`)。
#[must_use]
pub(crate) fn play_card_slug<'a>(state: &'a StateDto, a: &ActionDto) -> Option<&'a str> {
    match a {
        ActionDto::PlayCard { entity_id, .. } => my_slug_of(state, entity_id.0),
        _ => None,
    }
}

/// `priority` の slug 順に `eligible` を走査し、最初に一致した entity を返す。
/// 優先表に無い候補しか無ければ先頭の eligible を返す (eligible 非空前提)。
#[must_use]
pub(crate) fn pick_by_slug_priority(
    state: &StateDto,
    eligible: &[u32],
    priority: &[&str],
) -> Option<u32> {
    for want in priority {
        for &eid in eligible {
            if my_slug_of(state, eid) == Some(*want) {
                return Some(eid);
            }
        }
    }
    eligible.first().copied()
}

/// 合法手の中から「ていさつしれい」(drakloak の起動特性) を探す。
///
/// drakloak の起動特性はていさつしれいのみのため、drakloak への `UseAbility` を
/// そのまま採用してよい (ability_index 不問)。両ペルソナ共通。
#[must_use]
pub(crate) fn find_recon_directive(state: &StateDto, legal: &[ActionDto]) -> Option<ActionDto> {
    legal
        .iter()
        .find(|a| {
            matches!(
                a,
                ActionDto::UseAbility { entity_id, .. }
                    if my_slug_of(state, entity_id.0) == Some("drakloak")
            )
        })
        .cloned()
}

/// `priority` 順に進化先 slug に一致する `PlayCard` を選ぶ。意思決定ポリシー (どの進化を
/// どの順で・dragapult-ex を含めるか) は各ペルソナが原文から導いた `priority` で渡す。
///
/// ドロンチ止め guard: `priority` に `dragapult-ex` が含まれる場合に限り、場に `dragapult-ex` が
/// いる間は `drakloak`→`dragapult-ex` 進化を保留する (priority に含めないペルソナでは無効)。
#[must_use]
pub(crate) fn find_evolution(
    state: &StateDto,
    legal: &[ActionDto],
    priority: &[&str],
) -> Option<ActionDto> {
    for want in priority {
        // ドロンチ止め: dragapult-ex が既に場にいるなら進化しない。
        if *want == "dragapult-ex" && count_in_play(state, "dragapult-ex") >= 1 {
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

/// `priority` 順に、たね slug に一致する `PlayCard` を選ぶ。
#[must_use]
pub(crate) fn find_basic_placement(
    state: &StateDto,
    legal: &[ActionDto],
    priority: &[&str],
) -> Option<ActionDto> {
    for want in priority {
        if let Some(a) = legal
            .iter()
            .find(|a| play_card_slug(state, a) == Some(*want))
        {
            return Some(a.clone());
        }
    }
    None
}

/// 自分の手札にある `slug` の枚数。両ペルソナ共通。
#[must_use]
pub(crate) fn count_in_hand(state: &StateDto, slug: &str) -> usize {
    state
        .me
        .hand
        .iter()
        .filter(|e| e.card.as_deref() == Some(slug))
        .count()
}

/// `ChooseFromZone` が「手札トラッシュ cost」か (候補が全て自分の手札にある)。
/// engine は zone を deck 固定で返すため、候補の手札メンバーシップで判別する
/// (デッキ探索の候補は手札に無い)。両ペルソナ共通。
#[must_use]
pub(crate) fn is_hand_discard(p: &PromptMsg, options: &[EntityDto]) -> bool {
    let Some(state) = p.state.as_ref() else {
        return false;
    };
    if options.is_empty() {
        return false;
    }
    let hand: std::collections::HashSet<u32> = state.me.hand.iter().map(|e| e.entity_id).collect();
    options.iter().all(|o| hand.contains(&o.entity_id))
}

/// `slugs` を無作為順に並べ替える (原文がカテゴリ内の順を定めない箇所 = G1/G3b 用)。両ペルソナ共通。
#[must_use]
pub(crate) fn shuffle_strs(slugs: &[&'static str], rng: &mut ChaCha20Rng) -> Vec<&'static str> {
    use rand::seq::SliceRandom;
    let mut v: Vec<&'static str> = slugs.to_vec();
    v.shuffle(rng);
    v
}

/// デッキ探索候補 (`ChooseFromZone`) から、候補の `card` を見て `priority` 順に `max` 枚まで
/// 選ぶ。`min` に満たなければ残り候補で埋める (over/under 選択で illegal にしない)。両ペルソナ共通。
#[must_use]
pub(crate) fn pick_from_zone(
    options: &[EntityDto],
    priority: &[&str],
    min: u8,
    max: u8,
) -> PromptChoice {
    let max = usize::from(max);
    let mut selected: Vec<u32> = Vec::new();
    for want in priority {
        for o in options {
            if selected.len() >= max {
                break;
            }
            if o.card.as_deref() == Some(*want) && !selected.contains(&o.entity_id) {
                selected.push(o.entity_id);
            }
        }
    }
    // min を満たすよう残り候補で埋める (優先表に無いカードしか無い探索への保険)。
    if selected.len() < usize::from(min) {
        for o in options {
            if selected.len() >= usize::from(min) {
                break;
            }
            if !selected.contains(&o.entity_id) {
                selected.push(o.entity_id);
            }
        }
    }
    PromptChoice {
        selected,
        counts: vec![],
        yes: None,
        branch_index: None,
    }
}

// ============================================================================
// テスト用ヘルパー (crate 内共有)
// ============================================================================

#[cfg(test)]
pub(crate) mod testutil {
    use crate::wire::action::ActionDto;
    use crate::wire::protocol::{ClockSnapshot, PromptDto, PromptMsg, RequestMsg, WireClockOwner};
    use crate::wire::state::{EntityDto, PlayerView, PokemonInPlayDto, StateDto};
    use rand::SeedableRng;
    use rand_chacha::ChaCha20Rng;

    pub fn rng() -> ChaCha20Rng {
        ChaCha20Rng::from_seed([7u8; 32])
    }

    pub fn empty_player() -> PlayerView {
        PlayerView {
            active: None,
            bench: vec![],
            hand: vec![],
            deck_size: 60,
            discard: vec![],
            lost_zone: vec![],
            prizes: vec![],
            energy_attached_this_turn: false,
            supporter_played_this_turn: false,
            mulligan_count: 0,
            had_ko_last_turn: false,
        }
    }

    /// `(entity_id, slug)` 列から、その手札を持つ自分視点の `PlayerView` を作る。
    pub fn player_with_hand(hand: &[(u32, &str)]) -> PlayerView {
        let mut pv = empty_player();
        pv.hand = hand
            .iter()
            .map(|(id, slug)| EntityDto {
                entity_id: *id,
                card: Some((*slug).to_string()),
            })
            .collect();
        pv
    }

    /// `active_player` ("me"/"opp") と自分の `PlayerView` から `StateDto` を作る。
    pub fn state_with(active_player: &str, me: PlayerView) -> StateDto {
        StateDto {
            turn: 0,
            phase: "setup".to_string(),
            active_player: active_player.to_string(),
            stadium: None,
            me,
            opp: empty_player(),
        }
    }

    /// state を載せた prompt を作る (setup 選択のテスト用)。
    pub fn prompt_with_state(kind: PromptDto, state: StateDto) -> PromptMsg {
        let mut p = dummy_prompt(kind);
        p.state = Some(state);
        p
    }

    /// 場のポケモン 1 体の最小 DTO (slug 以外は既定値)。
    pub fn in_play(entity_id: u32, slug: &str) -> PokemonInPlayDto {
        PokemonInPlayDto {
            entity_id,
            card: Some(slug.to_string()),
            stage: "basic".to_string(),
            evolution_stack: vec![],
            hp_max: 60,
            damage: 0,
            energy_attached: vec![],
            tool_attached: None,
            status_conditions: vec![],
            abilities_used_this_turn: vec![],
            is_terastallized: false,
            turn_in_play: 1,
        }
    }

    /// 任意 state + 合法手から `RequestMsg` を作る (choose_action のテスト用)。
    pub fn request_with(state: StateDto, legal: Vec<ActionDto>) -> RequestMsg {
        let mut req = dummy_request(legal);
        req.state = state;
        req
    }

    pub fn dummy_request(legal: Vec<ActionDto>) -> RequestMsg {
        RequestMsg {
            request_id: "r-1".to_string(),
            resent: false,
            state: StateDto {
                turn: 1,
                phase: "main".to_string(),
                active_player: "me".to_string(),
                stadium: None,
                me: empty_player(),
                opp: empty_player(),
            },
            legal_actions: legal,
            clock: clock(),
        }
    }

    pub fn dummy_prompt(kind: PromptDto) -> PromptMsg {
        PromptMsg {
            request_id: "p-1".to_string(),
            parent_request_id: "r-1".to_string(),
            resent: false,
            kind,
            min: 0,
            max: 0,
            state: None,
            shuffle_after: false,
            clock: clock(),
        }
    }

    fn clock() -> ClockSnapshot {
        ClockSnapshot {
            my_remaining_ms: 0,
            opp_remaining_ms: 0,
            running_for: WireClockOwner::None,
            my_deadline_unix_ms: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cards::CardFacts;

    #[test]
    fn build_known_names_returns_some() {
        let reg = CardFacts::new();
        assert!(build("random", &reg).is_some());
        assert!(build("dragapult-takeuchi", &reg).is_some());
        assert!(build("dragapult-yopifutto", &reg).is_some());
    }

    #[test]
    fn build_unknown_name_returns_none() {
        let reg = CardFacts::new();
        assert!(build("nope", &reg).is_none());
    }

    #[test]
    fn available_lists_buildable_names() {
        let reg = CardFacts::new();
        for name in available() {
            assert!(build(name, &reg).is_some(), "{name} should build");
        }
    }

    #[test]
    fn pick_random_subset_in_range() {
        let mut rng = testutil::rng();
        let pool = vec![1, 2, 3, 4, 5];
        let v = pick_random_subset(&mut rng, &pool, 1, 3);
        assert!((1..=3).contains(&v.len()));
        for e in &v {
            assert!(pool.contains(e));
        }
    }

    #[test]
    fn pick_random_subset_empty_pool() {
        let mut rng = testutil::rng();
        let v = pick_random_subset(&mut rng, &[], 0, 3);
        assert!(v.is_empty());
    }

    #[test]
    fn my_turn_number_maps_turn_to_player_turn() {
        // turn は 1始まり・手番毎 +1 → ceil(turn/2)。1,2→1番目 / 3,4→2番目 / 5,6→3番目。
        use super::testutil::{empty_player, state_with};
        let mut s = state_with("me", empty_player());
        for (turn, expected) in [(0u32, 0u32), (1, 1), (2, 1), (3, 2), (4, 2), (5, 3), (6, 3)] {
            s.turn = turn;
            assert_eq!(my_turn_number(&s), expected, "turn={turn}");
        }
    }
}
