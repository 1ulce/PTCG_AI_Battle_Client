//! wire 契約テスト: サーバ (本体 PTCG AI Battle Platform) と同一 JSON になることを固定する。
//!
//! 本体の `engine-runtime` が出す JSON とバイト一致しないと通信できない。ここでは代表的な
//! メッセージについて「サーバが送ってくる JSON 文字列」をそのままデシリアライズできること、
//! および「クライアントが送る JSON」が期待形になることを検証する。型を共有しない契約の砦。

use ptcg_dragapult_bots::wire::action::{Action, ActionTarget, EntityId};
use ptcg_dragapult_bots::wire::event::{EventDto, WirePlayerId};
use ptcg_dragapult_bots::wire::protocol::{ClientMessage, PromptDto, ServerMessage};

/// サーバが送る `request` メッセージ (state + legal_actions + clock) を受信できる。
#[test]
fn deserializes_server_request() {
    let json = r#"{
        "type": "request",
        "request_id": "r-1",
        "resent": false,
        "state": {
            "turn": 3,
            "phase": "main",
            "active_player": "me",
            "me": {
                "active": {"entity_id": 10, "card": "dragapult-ex", "stage": "stage_2",
                    "evolution_stack": [8,9], "hp_max": 320, "damage": 0,
                    "energy_attached": [{"entity_id": 20, "card": "fire-energy"}],
                    "status_conditions": [], "abilities_used_this_turn": [],
                    "is_terastallized": false, "turn_in_play": 2},
                "bench": [], "hand": [{"entity_id": 30, "card": "boss-s-orders"}],
                "deck_size": 40, "discard": [], "lost_zone": [], "prizes": [],
                "energy_attached_this_turn": false, "supporter_played_this_turn": false,
                "mulligan_count": 0
            },
            "opp": {
                "bench": [], "hand": [{"entity_id": 99}], "deck_size": 40,
                "discard": [], "lost_zone": [], "prizes": [],
                "energy_attached_this_turn": false, "supporter_played_this_turn": false,
                "mulligan_count": 0
            }
        },
        "legal_actions": [
            {"id": "use_attack", "attack_index": 1},
            {"id": "end_turn"}
        ],
        "clock": {"my_remaining_ms": 0, "opp_remaining_ms": 0, "running_for": "me"}
    }"#;
    let msg: ServerMessage = serde_json::from_str(json).expect("deserialize request");
    let ServerMessage::Request(req) = msg else {
        panic!("expected request");
    };
    assert_eq!(req.state.turn, 3);
    assert_eq!(req.legal_actions.len(), 2);
    // 相手の hand は card: null (未公開) → None。
    assert!(req.state.opp.hand[0].card.is_none());
    // 自分側 active の装着エネ slug が読める。
    assert_eq!(
        req.state.me.active.as_ref().unwrap().energy_attached[0]
            .card
            .as_deref(),
        Some("fire-energy")
    );
}

/// サーバが送る `prompt` (ChooseFromZone) を受信できる。
#[test]
fn deserializes_server_prompt() {
    let json = r#"{
        "type": "prompt",
        "request_id": "p-1",
        "parent_request_id": "r-1",
        "resent": false,
        "kind": "choose_from_zone",
        "zone": "my_deck",
        "options": [{"entity_id": 1, "card": "budew"}, {"entity_id": 2}],
        "min": 1,
        "max": 1,
        "clock": {"my_remaining_ms": 0, "opp_remaining_ms": 0, "running_for": "none"}
    }"#;
    let msg: ServerMessage = serde_json::from_str(json).expect("deserialize prompt");
    let ServerMessage::Prompt(p) = msg else {
        panic!("expected prompt");
    };
    assert!(matches!(p.kind, PromptDto::ChooseFromZone { .. }));
    assert_eq!(p.min, 1);
}

/// サーバが送る `event` (game_end) を受信し winner/reason を読める。
#[test]
fn deserializes_game_end_event() {
    let json = r#"{
        "type": "event",
        "seq": 90,
        "actor": "system",
        "timestamp_unix_ms": 0,
        "replayed": false,
        "kind": "game_end",
        "data": {"winner": "p1", "reason": "prize_taken"}
    }"#;
    let msg: ServerMessage = serde_json::from_str(json).expect("deserialize event");
    let ServerMessage::Event(ev) = msg else {
        panic!("expected event");
    };
    assert_eq!(
        ev.event,
        EventDto::GameEnd {
            winner: Some(WirePlayerId::P1),
            reason: "prize_taken".to_string(),
        }
    );
}

/// 未知の event kind が来ても (サーバが新 variant を足しても) デシリアライズ自体は通り、
/// GameEnd 判定に影響しない。
#[test]
fn deserializes_unknown_internal_event() {
    let json = r#"{
        "type": "event", "seq": 1, "actor": "me", "timestamp_unix_ms": 0,
        "replayed": false, "kind": "turn_start", "data": {"turn": 1, "active_player": "p1"}
    }"#;
    let msg: ServerMessage = serde_json::from_str(json).expect("deserialize turn_start");
    assert!(matches!(msg, ServerMessage::Event(_)));
}

/// クライアントが送る `response` (use_attack) が期待 JSON になる。
#[test]
fn serializes_client_response_use_attack() {
    use ptcg_dragapult_bots::wire::protocol::ResponseMsg;
    let msg = ClientMessage::Response(ResponseMsg {
        request_id: "r-1".to_string(),
        action: Action::UseAttack { attack_index: 1 },
    });
    let v = serde_json::to_value(&msg).unwrap();
    assert_eq!(v["type"], "response");
    assert_eq!(v["action"]["id"], "use_attack");
    assert_eq!(v["action"]["attack_index"], 1);
}

/// PlayCard の entity_id は素の数値、target は kind タグ付き。
#[test]
fn serializes_play_card_with_target() {
    let a = Action::PlayCard {
        entity_id: EntityId(30),
        target: Some(ActionTarget::OppActive),
    };
    let v = serde_json::to_value(&a).unwrap();
    assert_eq!(v["id"], "play_card");
    assert_eq!(v["entity_id"], 30);
    assert_eq!(v["target"]["kind"], "opp_active");
}
