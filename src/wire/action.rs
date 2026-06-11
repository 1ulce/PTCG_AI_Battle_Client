//! アクション DTO (wire 形)。
//!
//! 本体 (`engine-core::actions::Action` / `ActionTarget`) の serde 表現を engine 非依存で
//! 再定義する。`tag = "id"` (Action) / `tag = "kind"` (ActionTarget)、`rename_all =
//! "snake_case"`。JSON 互換を 1 バイト単位で保つことが通信の前提。
//!
//! `EntityId` は `#[serde(transparent)]` な `u32` newtype。wire 上は素の数値として
//! (de)serialize され、bot コードは `entity_id.0` で u32 を取り出す。

use serde::{Deserialize, Serialize};

/// エンティティ識別子。wire 上は素の `u32`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EntityId(pub u32);

/// アクティブプレイヤーが Main で選ぶアクション (protocol §A.3)。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "id", rename_all = "snake_case")]
pub enum Action {
    /// 手札のカードを場/効果として使う
    PlayCard {
        entity_id: EntityId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target: Option<ActionTarget>,
    },
    /// 場のポケモンの起動型特性を発動
    UseAbility {
        entity_id: EntityId,
        ability_index: u8,
    },
    /// 手札のカードの起動型特性を発動 (`from_hand: true`)
    UseInHandAbility {
        entity_id: EntityId,
        ability_index: u8,
    },
    /// 場のスタジアムの起動型効果を発動
    UseStadiumEffect { stadium_entity_id: EntityId },
    /// にげる (アクティブ → ベンチ、ベンチ → アクティブ)
    Retreat {
        to_bench_index: u8,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        energy_to_discard: Vec<EntityId>,
    },
    /// アクティブポケモンのワザを使う (使うと番終了)
    UseAttack { attack_index: u8 },
    /// 明示的に番終了
    EndTurn,
    /// 場の化石を自分の番中に任意トラッシュ
    DiscardFossil { entity_id: EntityId },
    /// 投了
    Concede,
}

/// アクションが指す対象 (場のポケモン / スタジアム)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ActionTarget {
    OwnActive,
    OwnBench { index: u8 },
    OppActive,
    OppBench { index: u8 },
    Stadium,
}

/// wire 上のアクション DTO の別名 (本体での `ActionDto = Action` に対応)。
pub type ActionDto = Action;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_end_turn_serializes_with_id_tag() {
        let a = Action::EndTurn;
        let json = serde_json::to_value(&a).unwrap();
        assert_eq!(json["id"], "end_turn");
    }

    #[test]
    fn action_use_attack_serializes() {
        let a = Action::UseAttack { attack_index: 1 };
        let json = serde_json::to_value(&a).unwrap();
        assert_eq!(json["id"], "use_attack");
        assert_eq!(json["attack_index"], 1);
    }

    #[test]
    fn action_retreat_serializes() {
        let a = Action::Retreat {
            to_bench_index: 2,
            energy_to_discard: vec![],
        };
        let json = serde_json::to_value(&a).unwrap();
        assert_eq!(json["id"], "retreat");
        assert_eq!(json["to_bench_index"], 2);
        // energy_to_discard は空なら skip。
        assert!(json.get("energy_to_discard").is_none());
    }

    #[test]
    fn action_play_card_serializes_entity_id_as_plain_number() {
        let a = Action::PlayCard {
            entity_id: EntityId(42),
            target: None,
        };
        let json = serde_json::to_value(&a).unwrap();
        assert_eq!(json["entity_id"], 42);
        // target は None なら skip。
        assert!(json.get("target").is_none());
    }

    #[test]
    fn action_target_serializes_with_kind_tag() {
        let t = ActionTarget::OppBench { index: 3 };
        let json = serde_json::to_value(t).unwrap();
        assert_eq!(json["kind"], "opp_bench");
        assert_eq!(json["index"], 3);
    }

    #[test]
    fn action_roundtrip() {
        for a in [
            Action::EndTurn,
            Action::Concede,
            Action::UseAttack { attack_index: 0 },
            Action::PlayCard {
                entity_id: EntityId(7),
                target: Some(ActionTarget::OwnActive),
            },
            Action::Retreat {
                to_bench_index: 1,
                energy_to_discard: vec![EntityId(9)],
            },
        ] {
            let json = serde_json::to_string(&a).unwrap();
            let back: Action = serde_json::from_str(&json).unwrap();
            assert_eq!(a, back);
        }
    }
}
