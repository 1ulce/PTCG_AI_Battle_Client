//! state スナップショットの DTO (protocol §A.1)。
//!
//! 本体 `engine-runtime::state_dto` を engine 非依存で写経したもの。サーバが視点ごとに
//! マスクした「自分視点の縮約 state」。bot はこの DTO だけを読んで意思決定する。
//!
//! ## 視点ごとの違い
//!
//! - 自分側の `hand` は entity 列 + `card` 名入り
//! - 相手側の `hand` は `card: null`
//! - `deck` はサイズのみ (順序非公開)
//! - `discard` / `lost_zone` / 場 (`active` / `bench`) / `stadium` は全公開

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StateDto {
    pub turn: u32,
    pub phase: String,
    pub active_player: String, // "me" / "opp"
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stadium: Option<EntityDto>,
    pub me: PlayerView,
    pub opp: PlayerView,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlayerView {
    /// バトル場のポケモン (居なければ `None`)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active: Option<PokemonInPlayDto>,
    pub bench: Vec<PokemonInPlayDto>,
    pub hand: Vec<EntityDto>,
    pub deck_size: u32,
    pub discard: Vec<EntityDto>,
    pub lost_zone: Vec<EntityDto>,
    pub prizes: Vec<EntityDto>,
    pub energy_attached_this_turn: bool,
    pub supporter_played_this_turn: bool,
    pub mulligan_count: u8,
    /// 前の相手の番に、このプレイヤーのポケモンが 1 匹以上きぜつしたか。後方互換のため default false。
    #[serde(default)]
    pub had_ko_last_turn: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EntityDto {
    pub entity_id: u32,
    /// 公開済みなら slug、未公開なら `None`
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub card: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PokemonInPlayDto {
    pub entity_id: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub card: Option<String>,
    pub stage: String,
    pub evolution_stack: Vec<u32>,
    pub hp_max: u16,
    pub damage: u16,
    pub energy_attached: Vec<EntityDto>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_attached: Option<EntityDto>,
    pub status_conditions: Vec<String>,
    pub abilities_used_this_turn: Vec<u8>,
    pub is_terastallized: bool,
    pub turn_in_play: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_dto_roundtrip_minimal() {
        let empty = PlayerView {
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
        };
        let s = StateDto {
            turn: 1,
            phase: "main".to_string(),
            active_player: "me".to_string(),
            stadium: None,
            me: empty.clone(),
            opp: empty,
        };
        let json = serde_json::to_string(&s).unwrap();
        let back: StateDto = serde_json::from_str(&json).unwrap();
        assert_eq!(s, back);
    }

    #[test]
    fn entity_dto_skips_null_card() {
        let e = EntityDto {
            entity_id: 1,
            card: None,
        };
        let json = serde_json::to_string(&e).unwrap();
        assert!(!json.contains("card"), "card should be skipped when None");
    }
}
