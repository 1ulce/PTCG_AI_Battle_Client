//! event ペイロード DTO (protocol §A.2)。
//!
//! 本体 `engine-runtime::event_dto::EventDto` の serde 形を engine 非依存で写経。
//! クライアントは event を**受信するだけ** (engine→wire 変換関数は本体側にしか無いので不要)。
//! 全 variant を持つのは adjacently-tagged (`tag="kind", content="data"`) のため未知 variant が
//! 来るとデシリアライズが失敗するから。bot のループが実際に分岐するのは `GameEnd` のみ。

use serde::{Deserialize, Serialize};

/// event ペイロード。`EventMsg.event` に flatten される。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
pub enum EventDto {
    GameStart {
        p1_deck_size: u8,
        p2_deck_size: u8,
    },
    DecideFirstPlayer {
        result: WirePlayerId,
    },
    SetupComplete,
    GameEnd {
        #[serde(skip_serializing_if = "Option::is_none")]
        winner: Option<WirePlayerId>,
        reason: String,
    },
    DealInitialHand {
        player: WirePlayerId,
        entities: Vec<u32>,
    },
    Mulligan {
        player: WirePlayerId,
        count: u8,
    },
    PlaceActive {
        player: WirePlayerId,
        entity: u32,
    },
    PlaceBench {
        player: WirePlayerId,
        entity: u32,
        index: u8,
    },
    PlacePrizes {
        player: WirePlayerId,
        entities: Vec<u32>,
    },
    TurnStart {
        turn: u32,
        active_player: WirePlayerId,
    },
    DrawCard {
        player: WirePlayerId,
        entity: u32,
        deck_size_after: u32,
    },
    TurnEnd,
    Checkup,
    PlayItem {
        player: WirePlayerId,
        entity: u32,
        card: u32,
    },
    PlaySupporter {
        player: WirePlayerId,
        entity: u32,
        card: u32,
    },
    PlayStadium {
        player: WirePlayerId,
        entity: u32,
        card: u32,
    },
    AttachEnergy {
        player: WirePlayerId,
        energy: u32,
        to: u32,
    },
    Evolve {
        player: WirePlayerId,
        from: u32,
        to: u32,
    },
    RetreatPokemon {
        player: WirePlayerId,
        from: u32,
        to_bench_index: u8,
    },
    UseAbility {
        player: WirePlayerId,
        entity: u32,
        ability_index: u8,
    },
    DeclareAttack {
        player: WirePlayerId,
        entity: u32,
        attack_index: u8,
    },
    ApplyDamage {
        target: u32,
        amount: u16,
    },
    KnockOut {
        entity: u32,
    },
    TakePrize {
        player: WirePlayerId,
        entity: u32,
    },
    ApplyStatus {
        entity: u32,
        status: String,
    },
    RemoveStatus {
        entity: u32,
        status: String,
    },
    CoinFlip {
        purpose: String,
        result: String,
    },
    /// engine-core 内部 op の粗粒度通知 (デバッグ用)。
    Internal {
        name: String,
    },
    /// 再接続の再生フェーズ完了マーカー (meta event)。
    LiveCaughtUp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WirePlayerId {
    P1,
    P2,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn game_start_serializes_with_kind_tag() {
        let dto = EventDto::GameStart {
            p1_deck_size: 60,
            p2_deck_size: 60,
        };
        let json = serde_json::to_value(&dto).unwrap();
        assert_eq!(json["kind"], "game_start");
        assert_eq!(json["data"]["p1_deck_size"], 60);
    }

    #[test]
    fn game_end_roundtrip() {
        let dto = EventDto::GameEnd {
            winner: Some(WirePlayerId::P1),
            reason: "prize_taken".to_string(),
        };
        let json = serde_json::to_string(&dto).unwrap();
        let back: EventDto = serde_json::from_str(&json).unwrap();
        assert_eq!(dto, back);
    }

    #[test]
    fn turn_end_no_data() {
        let dto = EventDto::TurnEnd;
        let json = serde_json::to_value(&dto).unwrap();
        assert_eq!(json["kind"], "turn_end");
    }

    #[test]
    fn live_caught_up_serializes_kind_only() {
        let dto = EventDto::LiveCaughtUp;
        let json = serde_json::to_value(&dto).unwrap();
        assert_eq!(json["kind"], "live_caught_up");
    }
}
