//! WebSocket クライアント transport (tungstenite 直叩き、engine 非依存)。
//!
//! 本体 `engine-runtime::websocket` のクライアント経路 (`connect_ws_tcp` +
//! `send_client` / `recv_server`) を最小化したもの。`ws://HOST:PORT/ai-battle/v1/connect`
//! に平文で繋ぎ、WS テキストフレーム 1 = JSON 1 で送受信する。
//!
//! フレームレベル ping/pong は tungstenite が自動応答するので recv で読み飛ばす
//! (アプリ層の `ServerMessage::Ping` / `ClientMessage::Pong` とは別物)。Close は
//! [`TransportError::Closed`]。

use std::net::TcpStream;

use tungstenite::{
    client,
    error::Error as WsError,
    handshake::{client::Request, HandshakeError, HandshakeRole},
    protocol::Message,
    WebSocket,
};

use crate::wire::protocol::{ClientMessage, ServerMessage};

/// 接続先のエンドポイント path。
pub const CONNECT_PATH: &str = "/ai-battle/v1/connect";

/// transport エラー (bot は `Unexpected` のみ構築する)。
#[derive(Debug)]
pub enum TransportError {
    Io(std::io::Error),
    Closed,
    Unexpected(String),
}

impl std::fmt::Display for TransportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "io: {e}"),
            Self::Closed => write!(f, "connection closed"),
            Self::Unexpected(s) => write!(f, "unexpected: {s}"),
        }
    }
}

impl std::error::Error for TransportError {}

impl From<std::io::Error> for TransportError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<serde_json::Error> for TransportError {
    fn from(e: serde_json::Error) -> Self {
        Self::Unexpected(format!("json: {e}"))
    }
}

/// 同期 WebSocket クライアント。
pub struct WsClient {
    socket: WebSocket<TcpStream>,
}

impl WsClient {
    /// `ws://host:port/ai-battle/v1/connect` に TCP 接続して WebSocket ハンドシェイクする。
    ///
    /// # Errors
    /// TCP 接続失敗 / ハンドシェイク失敗。
    pub fn connect(host: &str, port: u16) -> Result<Self, TransportError> {
        let stream = TcpStream::connect((host, port))?;
        let host_header = stream
            .peer_addr()
            .map_or_else(|_| host.to_string(), |a| a.to_string());
        let request = build_ws_request("ws", &host_header, CONNECT_PATH)?;
        let (socket, _response) = client(request, stream).map_err(handshake_err_to_transport)?;
        Ok(Self { socket })
    }

    /// `ClientMessage` (subscribe / response / choice / pong) を送る。
    ///
    /// # Errors
    /// 送信失敗。
    pub fn send(&mut self, msg: &ClientMessage) -> Result<(), TransportError> {
        let line = serde_json::to_string(msg)?;
        self.socket
            .send(Message::Text(line.into()))
            .map_err(ws_err_to_transport)
    }

    /// サーバからの `ServerMessage` を 1 つ受け取る。フレーム ping/pong は読み飛ばす。
    ///
    /// # Errors
    /// 受信失敗 / 切断 ([`TransportError::Closed`])。
    pub fn recv(&mut self) -> Result<ServerMessage, TransportError> {
        loop {
            match self.socket.read() {
                Ok(Message::Text(text)) => return Ok(serde_json::from_str(text.as_str())?),
                Ok(Message::Binary(bytes)) => return Ok(serde_json::from_slice(&bytes)?),
                Ok(Message::Ping(_) | Message::Pong(_) | Message::Frame(_)) => {}
                Ok(Message::Close(_)) | Err(WsError::ConnectionClosed | WsError::AlreadyClosed) => {
                    return Err(TransportError::Closed);
                }
                Err(other) => return Err(ws_err_to_transport(other)),
            }
        }
    }
}

/// WebSocket アップグレード要求を組み立てる。サブプロトコルヘッダは本体 serve が negotiate
/// していないため付けない (付けるとハンドシェイクが失敗しうる)。
fn build_ws_request(scheme: &str, host: &str, path: &str) -> Result<Request, TransportError> {
    Request::builder()
        .method("GET")
        .uri(format!("{scheme}://{host}{path}"))
        .header("Host", host)
        .header("Connection", "Upgrade")
        .header("Upgrade", "websocket")
        .header("Sec-WebSocket-Version", "13")
        .header(
            "Sec-WebSocket-Key",
            tungstenite::handshake::client::generate_key(),
        )
        .body(())
        .map_err(|e| TransportError::Unexpected(format!("request build: {e}")))
}

fn ws_err_to_transport(e: WsError) -> TransportError {
    match e {
        WsError::Io(io) => TransportError::Io(io),
        WsError::ConnectionClosed | WsError::AlreadyClosed => TransportError::Closed,
        other => TransportError::Unexpected(format!("ws: {other}")),
    }
}

fn handshake_err_to_transport<S: HandshakeRole>(e: HandshakeError<S>) -> TransportError {
    match e {
        HandshakeError::Failure(err) => ws_err_to_transport(err),
        HandshakeError::Interrupted(_) => {
            TransportError::Unexpected("handshake interrupted".to_string())
        }
    }
}
