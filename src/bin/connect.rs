//! `connect` — Dragapult ex bot を WebSocket クライアントにしてリモート `serve` へ接続する。
//!
//! `wss://HOST/ai-battle/v1/connect` (TLS) または `ws://HOST:PORT` (平文) に繋ぎ、先頭 subscribe
//! を送ってから Request/Prompt を [`BotPolicy`](ptcg_dragapult_bots::bots::BotPolicy) で応答する。
//! `--games N` で N 局繰り返す (1 局 1 接続)。
//!
//! ## 使い方
//!
//! ```text
//! connect --server wss://arena.ptcgtools.com (--room ID | --vs NAME | --participant-id ID --auth-token TOK) \
//!         --bot dragapult-takeuchi --deck decks/dragapult-ex.yaml [--games N] [--seed S] \
//!         [--cards-dir data/pokemon-card-data/cards]
//! ```
//!
//! intent (接続ごと、いずれか必須):
//! - `--room ID`   = プライベートルーム (同じ room の 2 人を確実にペア)
//! - `--vs NAME`   = サーバ内蔵 bot を相手にリクエスト
//! - `--participant-id ID --auth-token TOK [--bucket B]` = ladder (要 ladder サーバ)
//!
//! intent 無指定はサーバに拒否される (open = 相手無指定の先着ペアは廃止)。

use std::fmt::Write as _;
use std::io::Write as _;

use rand::SeedableRng;
use rand_chacha::ChaCha20Rng;

use ptcg_dragapult_bots::bots::{self};
use ptcg_dragapult_bots::cards::CardFacts;
use ptcg_dragapult_bots::deck::DeckList;
use ptcg_dragapult_bots::transport::{TransportError, WsClient};
use ptcg_dragapult_bots::wire::event::{EventDto, WirePlayerId};
use ptcg_dragapult_bots::wire::protocol::{
    ChoiceMsg, ClientMessage, PongMsg, PromptMsg, RequestMsg, ResponseMsg, ServerMessage,
    SubscribeMsg,
};
use ptcg_dragapult_bots::wire::state::{PlayerView, PokemonInPlayDto, StateDto};

const DEFAULT_CARDS_DIR: &str = "data/pokemon-card-data/cards";
const DEFAULT_LOG_DIR: &str = "target/matches";

fn main() {
    if let Err(e) = run() {
        eprintln!("[connect] error: {e}");
        std::process::exit(1);
    }
}

/// コマンドライン引数をまとめた設定。
struct Config {
    server: String,
    bot: String,
    seed: u64,
    games: u32,
    deck_path: Option<String>,
    cards_dir: String,
    log_dir: String,
    participant_id: String,
    auth_token: String,
    bucket: String,
    room: String,
    vs: String,
}

/// 接続先 (parse_ws_server の結果)。
struct Endpoint {
    host: String,
    port: u16,
    secure: bool,
}

fn run() -> Result<(), String> {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let Some(cfg) = parse_args(&argv)? else {
        return Ok(()); // --help を表示した
    };

    if !bots::available().contains(&cfg.bot.as_str()) {
        return Err(format!(
            "--bot must be one of {:?} (got {:?})",
            bots::available(),
            cfg.bot
        ));
    }
    // 相手無指定の自動マッチング (open) はサーバで廃止されたので、明示 intent を要求する。
    if cfg.vs.is_empty() && cfg.room.is_empty() && cfg.participant_id.is_empty() {
        return Err(
            "connect requires an intent: --room ID (private pair) / --vs NAME (built-in bot) / --participant-id ID --auth-token TOK (ladder)".to_string(),
        );
    }
    let (host, port, secure) = parse_ws_server(&cfg.server)?;
    let endpoint = Endpoint { host, port, secure };

    // カード事実 (ex 判定 / ワザ index) を pokemon-card-data から読む。
    let cards = CardFacts::load_from_dir(&cfg.cards_dir)
        .map_err(|e| format!("load card data from {}: {e}", cfg.cards_dir))?;

    // 持参デッキ (BYO)。subscribe にそのまま載せる (サーバが resolve する)。
    let decklist: Option<DeckList> = match cfg.deck_path.as_deref() {
        Some(p) => Some(DeckList::load(p).map_err(|e| format!("load deck {p}: {e}"))?),
        None => None,
    };

    let scheme = if endpoint.secure { "wss" } else { "ws" };
    eprintln!(
        "[connect] {scheme}://{}:{}/ai-battle/v1/connect  bot={}  intent={}  games={}",
        endpoint.host,
        endpoint.port,
        cfg.bot,
        intent_label(&cfg),
        cfg.games
    );

    // 棋譜ディレクトリの命名用。日時は「保存場所の名前」だけに使い、bot 判断や乱数には一切渡さない
    // (シード固定の再現性は壊さない)。
    let run_stamp = utc_stamp();
    for game in 0..cfg.games {
        let game_seed = cfg.seed.wrapping_add(u64::from(game));
        let match_dir = std::path::Path::new(&cfg.log_dir).join(format!(
            "{run_stamp}-{}-vs-{}-seed{game_seed}",
            sanitize(&cfg.bot),
            opp_label(&cfg)
        ));
        let mut logger = match MatchLogger::create(&match_dir) {
            Ok(l) => {
                eprintln!("[connect] logging to {}", match_dir.display());
                Some(l)
            }
            Err(e) => {
                eprintln!(
                    "[connect] warning: could not open log dir {}: {e}",
                    match_dir.display()
                );
                None
            }
        };
        let summary = run_one_game(
            &endpoint,
            &cfg.bot,
            &cards,
            game_seed,
            &Subscribe {
                participant_id: &cfg.participant_id,
                auth_token: &cfg.auth_token,
                bucket: &cfg.bucket,
                room: &cfg.room,
                vs_bot: &cfg.vs,
                decklist: decklist.as_ref(),
            },
            logger.as_mut(),
        )?;
        if let Some(l) = logger.as_mut() {
            l.human(&format!("\n=== RESULT: {summary} ==="));
        }
        eprintln!("[connect] game {} done — {summary}", game + 1);
    }
    Ok(())
}

/// 引数を `Config` にパースする。`--help` を見たら `Ok(None)` (呼び元は正常終了)。
fn parse_args(argv: &[String]) -> Result<Option<Config>, String> {
    let mut server: Option<String> = None;
    let mut bot = "random".to_string();
    let mut seed: u64 = 42;
    let mut games: u32 = 1;
    let mut deck_path: Option<String> = None;
    let mut cards_dir = DEFAULT_CARDS_DIR.to_string();
    let mut log_dir = DEFAULT_LOG_DIR.to_string();
    let mut participant_id = String::new();
    let mut auth_token = String::new();
    let mut bucket = String::new();
    let mut room = String::new();
    let mut vs = String::new();

    let mut iter = argv.iter();
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
            "--log-dir" => log_dir = next("--log-dir")?,
            "--room" => room = next("--room")?,
            "--vs" => vs = next("--vs")?,
            "--participant-id" => participant_id = next("--participant-id")?,
            "--auth-token" => auth_token = next("--auth-token")?,
            "--bucket" => bucket = next("--bucket")?,
            "-h" | "--help" => {
                eprintln!("{HELP}");
                return Ok(None);
            }
            other => return Err(format!("unknown arg: {other}")),
        }
    }

    let server = server.ok_or("connect requires --server wss://HOST (or ws://HOST:PORT)")?;
    Ok(Some(Config {
        server,
        bot,
        seed,
        games,
        deck_path,
        cards_dir,
        log_dir,
        participant_id,
        auth_token,
        bucket,
        room,
        vs,
    }))
}

/// GameEnd のサマリ表記。`winner=None` は引き分け (両者同時に終局条件達成) なので "Draw"。
fn game_end_summary(winner: Option<WirePlayerId>, reason: &str) -> String {
    let w = winner.map_or_else(|| "Draw".to_string(), |p| format!("{p:?}"));
    format!("winner={w} reason={reason}")
}

/// ログ表示用の intent ラベル (`vs-bot:NAME` / `room:ID` / `ladder`)。
/// 無指定は run() のガードで弾かれるのでここには来ない (防御的に "none")。
fn intent_label(cfg: &Config) -> String {
    if !cfg.vs.is_empty() {
        format!("vs-bot:{}", cfg.vs)
    } else if !cfg.room.is_empty() {
        format!("room:{}", cfg.room)
    } else if !cfg.participant_id.is_empty() {
        "ladder".to_string()
    } else {
        "none".to_string()
    }
}

/// 棋譜ディレクトリ名に使う相手ラベル (ファイル名安全に sanitize 済み)。
fn opp_label(cfg: &Config) -> String {
    if !cfg.vs.is_empty() {
        sanitize(&cfg.vs)
    } else if !cfg.room.is_empty() {
        format!("room-{}", sanitize(&cfg.room))
    } else if !cfg.participant_id.is_empty() {
        "ladder".to_string()
    } else {
        "none".to_string()
    }
}

/// ファイル名に使えない文字を `_` に潰す (bot 名 / room ID はほぼ英数だが念のため)。
fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// 現在時刻 (UTC) を `YYYY-MM-DDThhmmss` で返す。棋譜ディレクトリ名専用。
fn utc_stamp() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    fmt_utc(secs)
}

/// Unix 秒 (UTC) を `YYYY-MM-DDThhmmss` に整形する純粋関数。
/// 日付変換は Howard Hinnant の civil-from-days (依存なしで実装)。
fn fmt_utc(secs: u64) -> String {
    let days = i64::try_from(secs / 86_400).unwrap_or(0);
    let sod = secs % 86_400;
    let (hh, mm, ss) = (sod / 3600, (sod % 3600) / 60, sod % 60);
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}T{hh:02}{mm:02}{ss:02}")
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
    endpoint: &Endpoint,
    bot_name: &str,
    cards: &CardFacts,
    seed: u64,
    sub: &Subscribe<'_>,
    mut log: Option<&mut MatchLogger>,
) -> Result<String, String> {
    let mut tx = WsClient::connect(&endpoint.host, endpoint.port, endpoint.secure)
        .map_err(|e| format!("connect {}:{}: {e}", endpoint.host, endpoint.port))?;
    let mut policy = bots::build(bot_name, cards).ok_or("unknown bot")?;
    let mut seed_bytes = [0u8; 32];
    seed_bytes[..8].copy_from_slice(&seed.to_le_bytes());
    let mut rng = ChaCha20Rng::from_seed(seed_bytes);

    let subscribe = ClientMessage::Subscribe(SubscribeMsg {
        match_id: String::new(),
        session_token: String::new(),
        from_seq: 0,
        participant_id: sub.participant_id.to_string(),
        auth_token: sub.auth_token.to_string(),
        bucket: sub.bucket.to_string(),
        room: sub.room.to_string(),
        vs_bot: sub.vs_bot.to_string(),
        decklist: sub.decklist.cloned(),
    });
    if let Some(l) = &mut log {
        l.raw("send", &subscribe);
    }
    tx.send(&subscribe)
        .map_err(|e| format!("send subscribe: {e}"))?;

    let mut summary = "disconnected".to_string();
    loop {
        let msg = match tx.recv() {
            Ok(m) => m,
            Err(TransportError::Closed) => break,
            Err(e) => return Err(format!("recv: {e}")),
        };
        if let Some(l) = &mut log {
            l.raw("recv", &msg);
        }
        if let Some(s) = handle_server_msg(msg, &mut policy, &mut rng, &mut tx, log.as_deref_mut())?
        {
            summary = s;
            break;
        }
    }
    Ok(summary)
}

/// 受信 1 メッセージを処理する。`request`/`prompt`/`ping` には応答を送り返す。
/// `GameEnd` を受けたら `Some(summary)` を返し、呼び元のループを終わらせる。
fn handle_server_msg(
    msg: ServerMessage,
    policy: &mut Box<dyn bots::BotPolicy>,
    rng: &mut ChaCha20Rng,
    tx: &mut WsClient,
    mut log: Option<&mut MatchLogger>,
) -> Result<Option<String>, String> {
    match msg {
        ServerMessage::Subscribed(s) => {
            if let Some(l) = &mut log {
                l.human(&format!(
                    "=== MATCH {} — you are {:?} vs {} ===",
                    s.match_id, s.your_player, s.opponent.ai_id
                ));
            }
        }
        ServerMessage::Error(err) => {
            eprintln!("[connect] server error: {err:?}");
            if let Some(l) = &mut log {
                l.human(&format!("[error] {err:?}"));
            }
        }
        ServerMessage::Event(ev) => {
            // 流れログ: 受信した event の seq / actor / 中身を 1 行で表示する。
            eprintln!("[event] seq={} actor={:?} {:?}", ev.seq, ev.actor, ev.event);
            if let Some(l) = &mut log {
                l.human(&format!(
                    "[event] seq={} actor={:?} {:?}",
                    ev.seq, ev.actor, ev.event
                ));
            }
            if let EventDto::GameEnd { winner, reason } = &ev.event {
                return Ok(Some(game_end_summary(*winner, reason)));
            }
        }
        ServerMessage::Request(req) => {
            if let Some(l) = &mut log {
                l.human(&format_request(&req));
            }
            let action = policy
                .choose_action(&req, rng)
                .map_err(|e| format!("choose_action: {e}"))?;
            if let Some(l) = &mut log {
                l.human(&format!("  >> chose: {}", compact(&action)));
            }
            let resp = ClientMessage::Response(ResponseMsg {
                request_id: req.request_id,
                action,
            });
            if let Some(l) = &mut log {
                l.raw("send", &resp);
            }
            tx.send(&resp).map_err(|e| format!("send response: {e}"))?;
        }
        ServerMessage::Prompt(p) => {
            if let Some(l) = &mut log {
                l.human(&format_prompt(&p));
            }
            let choice = policy.choose_prompt(&p, rng);
            if let Some(l) = &mut log {
                l.human(&format!(
                    "  >> choice: selected={:?} counts={:?} yes={:?} branch={:?}",
                    choice.selected, choice.counts, choice.yes, choice.branch_index
                ));
            }
            let chc = ClientMessage::Choice(ChoiceMsg {
                request_id: p.request_id,
                selected: choice.selected,
                counts: choice.counts,
                yes: choice.yes,
                branch_index: choice.branch_index,
            });
            if let Some(l) = &mut log {
                l.raw("send", &chc);
            }
            tx.send(&chc).map_err(|e| format!("send choice: {e}"))?;
        }
        ServerMessage::Ping(_) => {
            let pong = ClientMessage::Pong(PongMsg { last_seen_seq: 0 });
            if let Some(l) = &mut log {
                l.raw("send", &pong);
            }
            tx.send(&pong).map_err(|e| format!("send pong: {e}"))?;
        }
    }
    Ok(None)
}

/// マッチ棋譜を 2 ファイルに残す: `raw.jsonl` (送受信 JSON 全文) と `match.log` (人間可読)。
struct MatchLogger {
    raw: std::fs::File,
    human: std::fs::File,
}

impl MatchLogger {
    fn create(dir: &std::path::Path) -> std::io::Result<Self> {
        std::fs::create_dir_all(dir)?;
        let raw = std::fs::File::create(dir.join("raw.jsonl"))?;
        let human = std::fs::File::create(dir.join("match.log"))?;
        Ok(Self { raw, human })
    }

    /// 送受信したメッセージを完全な JSON で 1 行記録する (`{"t":"recv"|"send","msg":{...}}`)。
    fn raw<T: serde::Serialize>(&mut self, dir: &str, msg: &T) {
        let line = serde_json::json!({ "t": dir, "msg": msg });
        let _ = writeln!(self.raw, "{line}");
    }

    /// 人間可読ログに 1 行追記する。
    fn human(&mut self, s: &str) {
        let _ = writeln!(self.human, "{s}");
    }
}

/// `request` を人間可読に整形 (盤面要約 + 選択肢一覧)。
fn format_request(req: &RequestMsg) -> String {
    let mut out = format!(
        "\n== request {} (turn {}, phase {}, active {}){} ==\n{}",
        req.request_id,
        req.state.turn,
        req.state.phase,
        req.state.active_player,
        if req.resent { " [resent]" } else { "" },
        format_state(&req.state),
    );
    out.push_str("\n  legal actions:");
    for (i, a) in req.legal_actions.iter().enumerate() {
        let _ = write!(out, "\n    [{i}] {}", compact(a));
    }
    out
}

/// `prompt` を人間可読に整形 (種別 + 選ぶ個数 + 中身)。
fn format_prompt(p: &PromptMsg) -> String {
    format!(
        "\n-- prompt {} (parent {}, choose {}..{}){} --\n  kind: {}",
        p.request_id,
        p.parent_request_id,
        p.min,
        p.max,
        if p.resent { " [resent]" } else { "" },
        compact(&p.kind),
    )
}

/// 盤面 state を数行に要約 (両者の場・手札枚数・サイド・スタジアム)。
fn format_state(s: &StateDto) -> String {
    let mut out = String::new();
    if let Some(st) = &s.stadium {
        let _ = writeln!(out, "  stadium: {}", st.card.as_deref().unwrap_or("?"));
    }
    out.push_str(&format_player("me ", &s.me));
    out.push('\n');
    out.push_str(&format_player("opp", &s.opp));
    out
}

fn format_player(label: &str, pv: &PlayerView) -> String {
    let active = pv
        .active
        .as_ref()
        .map_or_else(|| "(none)".to_string(), format_pokemon);
    let bench = if pv.bench.is_empty() {
        "(empty)".to_string()
    } else {
        pv.bench
            .iter()
            .map(format_pokemon)
            .collect::<Vec<_>>()
            .join(" | ")
    };
    format!(
        "  {label} active: {active}\n  {label} bench : {bench}\n  {label} hand={} deck={} prizes={} discard={} lost={}",
        pv.hand.len(),
        pv.deck_size,
        pv.prizes.len(),
        pv.discard.len(),
        pv.lost_zone.len(),
    )
}

fn format_pokemon(p: &PokemonInPlayDto) -> String {
    let name = p.card.as_deref().unwrap_or("?");
    let hp = p.hp_max.saturating_sub(p.damage);
    let mut s = format!("{name}#{} HP{hp}/{}", p.entity_id, p.hp_max);
    if !p.energy_attached.is_empty() {
        let _ = write!(s, " E×{}", p.energy_attached.len());
    }
    if let Some(t) = &p.tool_attached {
        let _ = write!(s, " tool:{}", t.card.as_deref().unwrap_or("?"));
    }
    if !p.status_conditions.is_empty() {
        let _ = write!(s, " [{}]", p.status_conditions.join(","));
    }
    s
}

/// 任意の Serialize 値をコンパクト JSON 文字列にする (選択肢の表示用)。
fn compact<T: serde::Serialize>(v: &T) -> String {
    serde_json::to_string(v).unwrap_or_else(|_| "<unserializable>".to_string())
}

/// `ws://host:port` / `wss://host` を `(host, port, secure)` に分解する。
/// scheme が `wss://` なら `secure=true`。ポート省略時は既定 (wss=443 / ws=80) を補う。
fn parse_ws_server(s: &str) -> Result<(String, u16, bool), String> {
    let (rest, secure) = if let Some(r) = s.strip_prefix("wss://") {
        (r, true)
    } else if let Some(r) = s.strip_prefix("ws://") {
        (r, false)
    } else {
        (s, false)
    };
    let authority = rest.split('/').next().unwrap_or(rest);
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, port_str)) => {
            let p: u16 = port_str
                .parse()
                .map_err(|e: std::num::ParseIntError| format!("bad port in --server {s:?}: {e}"))?;
            (h, p)
        }
        None => (authority, if secure { 443 } else { 80 }),
    };
    if host.is_empty() {
        return Err(format!("--server has empty host (got {s:?})"));
    }
    Ok((host.to_string(), port, secure))
}

const HELP: &str = "\
connect — Dragapult ex bot を WebSocket クライアントにしてリモート serve へ接続する

USAGE:
    connect --server wss://HOST [OPTIONS]

OPTIONS:
    --server URL              接続先 (必須)。wss://HOST (TLS, 既定 443) / ws://HOST:PORT (平文)
    --bot NAME                random | dragapult-takeuchi | dragapult-yopifutto (既定 random)
    --deck PATH               持参デッキ YAML (BYO)
    --games N                 繰り返し試合数 (既定 1)
    --seed S                  乱数シード (既定 42)
    --cards-dir DIR           pokemon-card-data の cards ディレクトリ (既定 data/pokemon-card-data/cards)
    --log-dir DIR             棋譜の保存先 (既定 target/matches)。各局を 1 ディレクトリに保存
    --room ID                 プライベートルーム intent
    --vs NAME                 サーバ内蔵 bot を相手に指定
    --participant-id ID --auth-token TOK [--bucket B]   ladder intent";

#[cfg(test)]
mod tests {
    use super::{
        fmt_utc, format_pokemon, game_end_summary, intent_label, opp_label, parse_ws_server,
        sanitize, Config,
    };
    use ptcg_dragapult_bots::wire::event::WirePlayerId;
    use ptcg_dragapult_bots::wire::state::PokemonInPlayDto;

    #[test]
    fn game_end_summary_renders_draw_for_none() {
        // 勝者ありは P1/P2、winner=None は引き分けなので "Draw"。
        assert_eq!(
            game_end_summary(Some(WirePlayerId::P1), "PrizeTaken"),
            "winner=P1 reason=PrizeTaken"
        );
        assert_eq!(
            game_end_summary(Some(WirePlayerId::P2), "DeckOut"),
            "winner=P2 reason=DeckOut"
        );
        assert_eq!(
            game_end_summary(None, "PrizeTaken"),
            "winner=Draw reason=PrizeTaken"
        );
    }

    /// テスト用の最小 Config (intent 系フィールドだけ後で上書きする)。
    fn base_cfg() -> Config {
        Config {
            server: "wss://h".to_string(),
            bot: "dragapult-takeuchi".to_string(),
            seed: 42,
            games: 1,
            deck_path: None,
            cards_dir: String::new(),
            log_dir: String::new(),
            participant_id: String::new(),
            auth_token: String::new(),
            bucket: String::new(),
            room: String::new(),
            vs: String::new(),
        }
    }

    #[test]
    fn fmt_utc_known_timestamps() {
        assert_eq!(fmt_utc(0), "1970-01-01T000000");
        // 1_700_000_000 = 2023-11-14 22:13:20 UTC (広く知られた値)。
        assert_eq!(fmt_utc(1_700_000_000), "2023-11-14T221320");
        // 閏日 2024-02-29 00:00:00 UTC。
        assert_eq!(fmt_utc(1_709_164_800), "2024-02-29T000000");
    }

    #[test]
    fn sanitize_keeps_safe_chars_replaces_rest() {
        assert_eq!(sanitize("dragapult-takeuchi"), "dragapult-takeuchi");
        assert_eq!(sanitize("room_1"), "room_1");
        assert_eq!(sanitize("a/b c:d"), "a_b_c_d");
    }

    #[test]
    fn intent_and_opp_labels_follow_priority() {
        // vs-bot が最優先。
        let mut c = base_cfg();
        c.vs = "yopifutto".to_string();
        c.room = "r1".to_string(); // 同時指定でも vs が勝つ
        assert_eq!(intent_label(&c), "vs-bot:yopifutto");
        assert_eq!(opp_label(&c), "yopifutto");

        // room は opp_label で room- 接頭辞 + sanitize。
        let mut c = base_cfg();
        c.room = "my room".to_string();
        assert_eq!(intent_label(&c), "room:my room");
        assert_eq!(opp_label(&c), "room-my_room");

        // 何も無ければ "none" (open は廃止、run() のガードで弾かれる)。
        let c = base_cfg();
        assert_eq!(intent_label(&c), "none");
        assert_eq!(opp_label(&c), "none");

        // participant_id だけなら ladder。
        let mut c = base_cfg();
        c.participant_id = "p1".to_string();
        assert_eq!(intent_label(&c), "ladder");
        assert_eq!(opp_label(&c), "ladder");
    }

    #[test]
    fn format_pokemon_summarizes_hp_energy_status() {
        let p = PokemonInPlayDto {
            entity_id: 12,
            card: Some("dragapult-ex".to_string()),
            stage: "stage_2".to_string(),
            evolution_stack: vec![10, 11],
            hp_max: 320,
            damage: 60,
            energy_attached: vec![],
            tool_attached: None,
            status_conditions: vec!["burned".to_string()],
            abilities_used_this_turn: vec![],
            is_terastallized: false,
            turn_in_play: 4,
        };
        // HP は max-damage、状態異常は [..] で付く。エネ 0 なら E× は出ない。
        assert_eq!(format_pokemon(&p), "dragapult-ex#12 HP260/320 [burned]");
    }

    #[test]
    fn parses_ws_url() {
        assert_eq!(
            parse_ws_server("ws://127.0.0.1:8765/x").unwrap(),
            ("127.0.0.1".to_string(), 8765, false)
        );
        assert_eq!(
            parse_ws_server("example.com:80").unwrap(),
            ("example.com".to_string(), 80, false)
        );
    }

    #[test]
    fn parses_wss_default_port() {
        // wss:// はポート省略で 443、secure=true。
        assert_eq!(
            parse_ws_server("wss://arena.ptcgtools.com").unwrap(),
            ("arena.ptcgtools.com".to_string(), 443, true)
        );
        // wss:// で明示ポートも可。
        assert_eq!(
            parse_ws_server("wss://arena.ptcgtools.com:8443/x").unwrap(),
            ("arena.ptcgtools.com".to_string(), 8443, true)
        );
    }

    #[test]
    fn ws_without_port_defaults_80() {
        assert_eq!(
            parse_ws_server("ws://host").unwrap(),
            ("host".to_string(), 80, false)
        );
    }
}
