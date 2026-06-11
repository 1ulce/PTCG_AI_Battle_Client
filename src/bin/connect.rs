//! `connect` — Dragapult ex bot を WebSocket クライアントにしてリモート `serve` へ接続する。
//!
//! `ws://HOST:PORT/ai-battle/v1/connect` に繋ぎ、先頭 subscribe を送ってから Request/Prompt
//! を [`BotPolicy`](ptcg_dragapult_bots::bots::BotPolicy) で応答する。`--games N` で N 局
//! 繰り返す (1 局 1 接続)。
//!
//! ## 使い方
//!
//! ```text
//! connect --server ws://HOST:8765 [--room ID | --vs NAME] \
//!         --bot dragapult-takeuchi --deck decks/dragapult-ex.yaml [--games N] [--seed S] \
//!         [--cards-dir data/pokemon-card-data/cards]
//! ```
//!
//! intent (接続ごと):
//! - 無指定        = open match (誰でも先着 2 人ペア)
//! - `--room ID`   = プライベートルーム (同じ room の 2 人を確実にペア)
//! - `--vs NAME`   = サーバ内蔵 bot を相手にリクエスト
//! - `--participant-id ID --auth-token TOK [--bucket B]` = ladder (要 ladder サーバ)

use rand::SeedableRng;
use rand_chacha::ChaCha20Rng;

use ptcg_dragapult_bots::bots::{self};
use ptcg_dragapult_bots::cards::CardFacts;
use ptcg_dragapult_bots::deck::DeckList;
use ptcg_dragapult_bots::transport::{TransportError, WsClient};
use ptcg_dragapult_bots::wire::event::EventDto;
use ptcg_dragapult_bots::wire::protocol::{
    ChoiceMsg, ClientMessage, PongMsg, ResponseMsg, ServerMessage, SubscribeMsg,
};

const DEFAULT_CARDS_DIR: &str = "data/pokemon-card-data/cards";

fn main() {
    if let Err(e) = run() {
        eprintln!("[connect] error: {e}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut server: Option<String> = None;
    let mut bot = "random".to_string();
    let mut seed: u64 = 42;
    let mut games: u32 = 1;
    let mut deck_path: Option<String> = None;
    let mut cards_dir = DEFAULT_CARDS_DIR.to_string();
    let mut participant_id = String::new();
    let mut auth_token = String::new();
    let mut bucket = String::new();
    let mut room = String::new();
    let mut vs = String::new();

    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        let mut next = |name: &str| {
            iter.next()
                .cloned()
                .ok_or_else(|| format!("missing value for {name}"))
        };
        match arg.as_str() {
            "--server" => server = Some(next("--server")?),
            "--bot" => bot = next("--bot")?,
            "--seed" => {
                seed = next("--seed")?
                    .parse()
                    .map_err(|e: std::num::ParseIntError| e.to_string())?;
            }
            "--games" => {
                games = next("--games")?
                    .parse()
                    .map_err(|e: std::num::ParseIntError| e.to_string())?;
            }
            "--deck" => deck_path = Some(next("--deck")?),
            "--cards-dir" => cards_dir = next("--cards-dir")?,
            "--room" => room = next("--room")?,
            "--vs" => vs = next("--vs")?,
            "--participant-id" => participant_id = next("--participant-id")?,
            "--auth-token" => auth_token = next("--auth-token")?,
            "--bucket" => bucket = next("--bucket")?,
            "-h" | "--help" => {
                eprintln!("{HELP}");
                return Ok(());
            }
            other => return Err(format!("unknown arg: {other}")),
        }
    }

    let server = server.ok_or("connect requires --server ws://HOST:PORT")?;
    if !bots::available().contains(&bot.as_str()) {
        return Err(format!(
            "--bot must be one of {:?} (got {bot:?})",
            bots::available()
        ));
    }
    let (host, port) = parse_ws_server(&server)?;

    // カード事実 (ex 判定 / ワザ index) を pokemon-card-data から読む。
    let cards = CardFacts::load_from_dir(&cards_dir)
        .map_err(|e| format!("load card data from {cards_dir}: {e}"))?;

    // 持参デッキ (BYO)。subscribe にそのまま載せる (サーバが resolve する)。
    let decklist: Option<DeckList> = match deck_path.as_deref() {
        Some(p) => Some(DeckList::load(p).map_err(|e| format!("load deck {p}: {e}"))?),
        None => None,
    };

    let intent = if !vs.is_empty() {
        format!("vs-bot:{vs}")
    } else if !room.is_empty() {
        format!("room:{room}")
    } else if !participant_id.is_empty() {
        "ladder".to_string()
    } else {
        "open".to_string()
    };
    eprintln!(
        "[connect] ws://{host}:{port}/ai-battle/v1/connect  bot={bot}  intent={intent}  games={games}"
    );

    for game in 0..games {
        let game_seed = seed.wrapping_add(u64::from(game));
        let summary = run_one_game(
            &host,
            port,
            &bot,
            &cards,
            game_seed,
            &Subscribe {
                participant_id: &participant_id,
                auth_token: &auth_token,
                bucket: &bucket,
                room: &room,
                vs_bot: &vs,
                decklist: decklist.as_ref(),
            },
        )?;
        eprintln!("[connect] game {} done — {summary}", game + 1);
    }
    Ok(())
}

/// subscribe に載せる intent / 認証 / 持参デッキ一式。
struct Subscribe<'a> {
    participant_id: &'a str,
    auth_token: &'a str,
    bucket: &'a str,
    room: &'a str,
    vs_bot: &'a str,
    decklist: Option<&'a DeckList>,
}

/// 1 局分: WS 接続 → subscribe → Request/Prompt を bot で応答 → GameEnd で終了。
fn run_one_game(
    host: &str,
    port: u16,
    bot_name: &str,
    cards: &CardFacts,
    seed: u64,
    sub: &Subscribe<'_>,
) -> Result<String, String> {
    let mut tx =
        WsClient::connect(host, port).map_err(|e| format!("connect ws://{host}:{port}: {e}"))?;
    let mut policy = bots::build(bot_name, cards).ok_or("unknown bot")?;
    let mut seed_bytes = [0u8; 32];
    seed_bytes[..8].copy_from_slice(&seed.to_le_bytes());
    let mut rng = ChaCha20Rng::from_seed(seed_bytes);

    tx.send(&ClientMessage::Subscribe(SubscribeMsg {
        match_id: String::new(),
        session_token: String::new(),
        from_seq: 0,
        participant_id: sub.participant_id.to_string(),
        auth_token: sub.auth_token.to_string(),
        bucket: sub.bucket.to_string(),
        room: sub.room.to_string(),
        vs_bot: sub.vs_bot.to_string(),
        decklist: sub.decklist.cloned(),
    }))
    .map_err(|e| format!("send subscribe: {e}"))?;

    let mut summary = "disconnected".to_string();
    loop {
        let msg = match tx.recv() {
            Ok(m) => m,
            Err(TransportError::Closed) => break,
            Err(e) => return Err(format!("recv: {e}")),
        };
        match msg {
            ServerMessage::Subscribed(_) => {}
            ServerMessage::Error(err) => {
                eprintln!("[connect] server error: {err:?}");
            }
            ServerMessage::Event(ev) => {
                if let EventDto::GameEnd { winner, reason } = &ev.event {
                    summary = format!("winner={winner:?} reason={reason}");
                    break;
                }
            }
            ServerMessage::Request(req) => {
                let action = policy
                    .choose_action(&req, &mut rng)
                    .map_err(|e| format!("choose_action: {e}"))?;
                tx.send(&ClientMessage::Response(ResponseMsg {
                    request_id: req.request_id,
                    action,
                }))
                .map_err(|e| format!("send response: {e}"))?;
            }
            ServerMessage::Prompt(p) => {
                let choice = policy.choose_prompt(&p, &mut rng);
                tx.send(&ClientMessage::Choice(ChoiceMsg {
                    request_id: p.request_id,
                    selected: choice.selected,
                    counts: choice.counts,
                    yes: choice.yes,
                    branch_index: choice.branch_index,
                }))
                .map_err(|e| format!("send choice: {e}"))?;
            }
            ServerMessage::Ping(_) => {
                tx.send(&ClientMessage::Pong(PongMsg { last_seen_seq: 0 }))
                    .map_err(|e| format!("send pong: {e}"))?;
            }
        }
    }
    Ok(summary)
}

/// `ws://host:port` (scheme/path は任意) を `(host, port)` に分解する。port は必須。
fn parse_ws_server(s: &str) -> Result<(String, u16), String> {
    let rest = s
        .strip_prefix("ws://")
        .or_else(|| s.strip_prefix("wss://"))
        .unwrap_or(s);
    let authority = rest.split('/').next().unwrap_or(rest);
    let (host, port_str) = authority
        .rsplit_once(':')
        .ok_or_else(|| format!("--server must include a port, e.g. ws://HOST:PORT (got {s:?})"))?;
    if host.is_empty() {
        return Err(format!("--server has empty host (got {s:?})"));
    }
    let port: u16 = port_str
        .parse()
        .map_err(|e: std::num::ParseIntError| format!("bad port in --server {s:?}: {e}"))?;
    Ok((host.to_string(), port))
}

const HELP: &str = "\
connect — Dragapult ex bot を WebSocket クライアントにしてリモート serve へ接続する

USAGE:
    connect --server ws://HOST:PORT [OPTIONS]

OPTIONS:
    --server ws://HOST:PORT   接続先 (必須)
    --bot NAME                random | dragapult-takeuchi | dragapult-yopifutto (既定 random)
    --deck PATH               持参デッキ YAML (BYO)
    --games N                 繰り返し試合数 (既定 1)
    --seed S                  乱数シード (既定 42)
    --cards-dir DIR           pokemon-card-data の cards ディレクトリ (既定 data/pokemon-card-data/cards)
    --room ID                 プライベートルーム intent
    --vs NAME                 サーバ内蔵 bot を相手に指定
    --participant-id ID --auth-token TOK [--bucket B]   ladder intent";

#[cfg(test)]
mod tests {
    use super::parse_ws_server;

    #[test]
    fn parses_ws_url() {
        assert_eq!(
            parse_ws_server("ws://127.0.0.1:8765/x").unwrap(),
            ("127.0.0.1".to_string(), 8765)
        );
        assert_eq!(
            parse_ws_server("example.com:80").unwrap(),
            ("example.com".to_string(), 80)
        );
    }

    #[test]
    fn rejects_missing_port() {
        assert!(parse_ws_server("ws://host").is_err());
    }
}
