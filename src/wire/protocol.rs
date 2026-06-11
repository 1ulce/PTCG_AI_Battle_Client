//! プロトコル §4 の wire メッセージ DTO。
//!
//! 本体 `engine-runtime::protocol` を engine 非依存で写経。サーバ → AI は [`ServerMessage`]、
//! AI → サーバは [`ClientMessage`]。JSON 上は `"type"` フィールドで識別される
//! (`#[serde(tag = "type", rename_all = "snake_case")]`)。

use serde::{Deserialize, Serialize};

use crate::wire::action::ActionDto;
use crate::wire::event::EventDto;
use crate::wire::state::{EntityDto, StateDto};

/// プロトコルバージョン。
pub const PROTOCOL_VERSION: &str = "0.1.0";

// ============================================================================
// 共通サブ型
// ============================================================================

/// 視点識別子。`"me"` / `"opp"` / `"system"`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WireActor {
    Me,
    Opp,
    System,
}

/// プレイヤー識別子 (subscribed.your_player 用)。`"p1"` / `"p2"`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WirePlayer {
    P1,
    P2,
}

/// 時計スナップショット (protocol §7.2)。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClockSnapshot {
    pub my_remaining_ms: u64,
    pub opp_remaining_ms: u64,
    pub running_for: WireClockOwner,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub my_deadline_unix_ms: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WireClockOwner {
    Me,
    Opp,
    None,
}

/// タイムコントロール設定 (subscribed メッセージで AI に通知)。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimeControl {
    pub kind: TimeControlKind,
    pub main_ms: u64,
    #[serde(default)]
    pub increment_ms: u64,
    #[serde(default)]
    pub byoyomi_ms: u64,
    #[serde(default)]
    pub hard_per_response_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TimeControlKind {
    SuddenDeath,
    Increment,
    Byoyomi,
    Correspondence,
}

impl Default for TimeControl {
    fn default() -> Self {
        Self {
            kind: TimeControlKind::Correspondence,
            main_ms: 0,
            increment_ms: 0,
            byoyomi_ms: 0,
            hard_per_response_ms: 60_000,
        }
    }
}

/// 相手 AI の表示情報 (subscribed メッセージ用)。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpponentInfo {
    pub ai_id: String,
    pub display_name: String,
}

// ============================================================================
// ServerMessage
// ============================================================================

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[allow(clippy::large_enum_variant)]
pub enum ServerMessage {
    Subscribed(SubscribedMsg),
    Event(EventMsg),
    Request(RequestMsg),
    Prompt(PromptMsg),
    Ping(PingMsg),
    Error(ErrorMsg),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubscribedMsg {
    pub protocol_version: String,
    pub match_id: String,
    pub your_player: WirePlayer,
    pub server_current_seq: u64,
    pub opponent: OpponentInfo,
    pub time_control: TimeControl,
    /// 再接続用セッショントークン (protocol §5.2)。旧クライアント互換のため `#[serde(default)]`。
    #[serde(default)]
    pub session_token: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventMsg {
    pub seq: u64,
    pub actor: WireActor,
    pub timestamp_unix_ms: u64,
    pub replayed: bool,
    #[serde(flatten)]
    pub event: EventDto,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clock: Option<ClockSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestMsg {
    pub request_id: String,
    pub resent: bool,
    pub state: StateDto,
    pub legal_actions: Vec<ActionDto>,
    pub clock: ClockSnapshot,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptMsg {
    pub request_id: String,
    pub parent_request_id: String,
    pub resent: bool,
    #[serde(flatten)]
    pub kind: PromptDto,
    pub min: u8,
    pub max: u8,
    /// prompt 時点の視点固有マスク state (protocol §8)。旧クライアント互換のため optional。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<StateDto>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub shuffle_after: bool,
    pub clock: ClockSnapshot,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PingMsg {
    pub server_current_seq: u64,
    pub server_time_unix_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorMsg {
    pub code: ErrorCode,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub related_request_id: Option<String>,
    #[serde(default)]
    pub fatal: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    IllegalAction,
    IllegalChoice,
    UnknownRequestId,
    FromSeqTooLarge,
    InvalidSessionToken,
    ProtocolViolation,
    InternalError,
}

// ============================================================================
// ClientMessage
// ============================================================================

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    Subscribe(SubscribeMsg),
    Response(ResponseMsg),
    Choice(ChoiceMsg),
    Pong(PongMsg),
    Resync(ResyncMsg),
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubscribeMsg {
    pub match_id: String,
    pub session_token: String,
    pub from_seq: u64,
    /// ライブ ladder の登録アカウント ID。self-play / 再接続 / 旧クライアントは空。
    #[serde(default)]
    pub participant_id: String,
    /// 登録時に発行された認証トークン (平文)。
    #[serde(default)]
    pub auth_token: String,
    /// 入りたい待機列のキー。空ならサーバ既定バケット。
    #[serde(default)]
    pub bucket: String,
    /// プライベートルームID。同じ room を指定した 2 接続を確実にペアにする。
    #[serde(default)]
    pub room: String,
    /// 内蔵 bot 相手をリクエストする場合の bot 名 (空=なし)。
    #[serde(default)]
    pub vs_bot: String,
    /// 持参デッキ (BYO)。slug+枚数で registry 非依存。サーバが自分の registry で resolve する。
    #[serde(default)]
    pub decklist: Option<crate::deck::DeckList>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResponseMsg {
    pub request_id: String,
    pub action: ActionDto,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChoiceMsg {
    pub request_id: String,
    pub selected: Vec<u32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub counts: Vec<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub yes: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch_index: Option<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PongMsg {
    pub last_seen_seq: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResyncMsg {
    pub missing_from: u64,
    pub missing_to: u64,
}

// ============================================================================
// PromptDto
// ============================================================================

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PromptDto {
    ChooseFromZone {
        zone: String,
        options: Vec<EntityDto>,
    },
    ChooseTargetPokemon {
        targets: Vec<u32>,
    },
    DistributeDamage {
        eligible: Vec<u32>,
        total: u8,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        per_target_max: Option<u8>,
    },
    AttachEnergyTo {
        energy_options: Vec<u32>,
        pokemon_eligible: Vec<u32>,
    },
    DiscardFromAttached {
        eligible: Vec<u32>,
        kind_filter: String,
    },
    ReorderCards {
        cards: Vec<u32>,
        destination: String,
    },
    ReplaceActiveAfterKo {
        bench_options: Vec<u32>,
    },
    ChooseYesNo {
        prompt_text: String,
    },
    /// Setup: コイン勝者が先攻/後攻を選ぶ。応答は `yes` (true=自分が先攻)。
    ChooseFirstOrSecond,
    ChooseInitialActive {
        eligible: Vec<u32>,
    },
    PlaceInitialBench {
        eligible: Vec<u32>,
        bench_max: u8,
    },
    SelectAbilityOrder {
        entries: Vec<u32>,
    },
    ChooseOneBranch {
        branch_count: u8,
        labels: Vec<String>,
    },
    PeekAndReorder {
        peeked: Vec<u32>,
        destination: String,
    },
    AssignEnergyToTargets {
        energies: Vec<u32>,
        pokemon_eligible: Vec<u32>,
    },
    ChooseOpponentAttack {
        attack_count: u8,
        labels: Vec<String>,
    },
    PrizeHandSwapChoice {
        prize_options: Vec<u32>,
        hand_options: Vec<u32>,
    },
    PickAmountFromEach {
        /// (source_entity, source の現在のカウンタ最大数)
        sources: Vec<(u32, u8)>,
        /// 合算先 entity
        dest: u32,
    },
    ChooseStatusToRemove {
        /// 対象 entity_id
        target: u32,
        /// 候補 status の名前
        statuses: Vec<String>,
    },
    PickAttackToCopy {
        /// (pokemon entity_id, ワザ名一覧) のペア群
        candidates: Vec<(u32, Vec<String>)>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip<T: Serialize + for<'de> Deserialize<'de> + PartialEq + std::fmt::Debug>(v: &T) {
        let json = serde_json::to_string(v).expect("serialize");
        let back: T = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(*v, back);
    }

    #[test]
    fn server_subscribed_roundtrip() {
        let msg = ServerMessage::Subscribed(SubscribedMsg {
            protocol_version: "0.1.0".to_string(),
            match_id: "m-1".to_string(),
            your_player: WirePlayer::P1,
            server_current_seq: 0,
            opponent: OpponentInfo {
                ai_id: "ai-2".to_string(),
                display_name: "Bot 2".to_string(),
            },
            time_control: TimeControl::default(),
            session_token: "tok-abc".to_string(),
        });
        roundtrip(&msg);
    }

    #[test]
    fn client_subscribe_roundtrip() {
        let msg = ClientMessage::Subscribe(SubscribeMsg {
            match_id: "m-1".to_string(),
            session_token: "tok".to_string(),
            from_seq: 0,
            ..Default::default()
        });
        roundtrip(&msg);
    }

    #[test]
    fn client_choice_roundtrip() {
        let msg = ClientMessage::Choice(ChoiceMsg {
            request_id: "p-0001".to_string(),
            selected: vec![1, 2, 3],
            counts: vec![],
            yes: None,
            branch_index: None,
        });
        roundtrip(&msg);
    }

    #[test]
    fn prompt_dto_choose_from_zone_serializes_with_kind_tag() {
        let p = PromptDto::ChooseFromZone {
            zone: "my_deck".to_string(),
            options: vec![
                EntityDto {
                    entity_id: 1,
                    card: Some("budew".to_string()),
                },
                EntityDto {
                    entity_id: 2,
                    card: None,
                },
            ],
        };
        let json = serde_json::to_value(&p).unwrap();
        assert_eq!(json["kind"], "choose_from_zone");
        assert_eq!(json["options"][0]["entity_id"], 1);
        assert_eq!(json["options"][0]["card"], "budew");
    }

    #[test]
    fn type_tag_uses_snake_case() {
        let json = serde_json::to_value(ServerMessage::Ping(PingMsg {
            server_current_seq: 1,
            server_time_unix_ms: 1000,
        }))
        .unwrap();
        assert_eq!(json["type"], "ping");
    }

    #[test]
    fn error_code_serializes_snake_case() {
        let e = ErrorMsg {
            code: ErrorCode::IllegalAction,
            message: "bad".to_string(),
            related_request_id: Some("r-1".to_string()),
            fatal: false,
        };
        let json = serde_json::to_value(&e).unwrap();
        assert_eq!(json["code"], "illegal_action");
    }
}
