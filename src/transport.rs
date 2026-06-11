//! WebSocket クライアント transport (tungstenite 直叩き、engine 非依存)。
//!
//! 本体 `engine-runtime::websocket` のクライアント経路 (`connect_ws_tcp` +
//! `send_client` / `recv_server`) を最小化したもの。`ws://` (平文) と `wss://` (TLS、rustls +
//! OS ルート証明書) の両方で `HOST:PORT/ai-battle/v1/connect` に繋ぎ、WS テキストフレーム
//! 1 = JSON 1 で送受信する。
//!
//! フレームレベル ping/pong は tungstenite が自動応答するので recv で読み飛ばす
//! (アプリ層の `ServerMessage::Ping` / `ClientMessage::Pong` とは別物)。Close は
//! [`TransportError::Closed`]。

use std::net::TcpStream;

use tungstenite::{
    client, client_tls,
    error::Error as WsError,
    handshake::{client::Request, HandshakeError, HandshakeRole},
    protocol::Message,
    stream::MaybeTlsStream,
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

/// 同期 WebSocket クライアント (平文 / TLS 両対応)。
pub struct WsClient {
    socket: WebSocket<MaybeTlsStream<TcpStream>>,
}

impl WsClient {
    /// `host:port/ai-battle/v1/connect` に接続して WebSocket ハンドシェイクする。
    /// `secure=true` なら `wss://` (TLS、rustls + OS ルート証明書で証明書を検証)。
    ///
    /// # Errors
    /// TCP 接続失敗 / TLS ハンドシェイク失敗 (証明書不正等) / WebSocket ハンドシェイク失敗。
    pub fn connect(host: &str, port: u16, secure: bool) -> Result<Self, TransportError> {
        let stream = TcpStream::connect((host, port))?;
        let scheme = if secure { "wss" } else { "ws" };
        // 既定ポート (wss=443 / ws=80) のときは Host/URI に :port を付けない
        // (TLS の証明書検証はホスト名で行われるため、bare host にする)。
        let default_port = if secure { 443 } else { 80 };
        let authority = if port == default_port {
            host.to_string()
        } else {
            format!("{host}:{port}")
        };
        let request = build_ws_request(scheme, &authority, CONNECT_PATH)?;
        let socket = if secure {
            install_crypto_provider();
            let (socket, _response) =
                client_tls(request, stream).map_err(handshake_err_to_transport)?;
            socket
        } else {
            let (socket, _response) = client(request, MaybeTlsStream::Plain(stream))
                .map_err(handshake_err_to_transport)?;
            socket
        };
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

/// rustls 0.23 の process-default CryptoProvider を 1 度だけ ring で設定する。
/// `client_tls` (tungstenite + rustls) はこの default を使う。
fn install_crypto_provider() {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        // 既に設定済みなら Err が返るが無視してよい (他クレートが設定した default を使う)。
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
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
