//! RFC 6455 WebSocket client, built on `tokio-tungstenite` with the same
//! rustls/ring TLS configuration as the rest of the stack.
//!
//! The split into [`WsTx`] / [`WsRx`] halves lets one task own receives
//! while sends stay available to the owner (page scripts) at any time.

use crate::NetError;
use futures_util::{SinkExt, StreamExt};
use std::sync::Arc;
use std::time::Duration;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::{http::HeaderValue, Message};

type WsStream =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// Sending half of a WebSocket connection. Cloneable: every clone shares the
/// same sink (sends are serialized through an async mutex).
#[derive(Clone)]
pub struct WsTx(Arc<tokio::sync::Mutex<futures_util::stream::SplitSink<WsStream, Message>>>);

/// Receiving half of a WebSocket connection.
pub struct WsRx(futures_util::stream::SplitStream<WsStream>);

/// A connected WebSocket, split into independent send/receive halves.
pub struct WsConnection {
    pub tx: WsTx,
    pub rx: WsRx,
}

/// One inbound WebSocket event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WsIncoming {
    Text(String),
    Binary(Vec<u8>),
    /// Peer closed the connection (or the stream ended).
    Closed,
}

impl WsTx {
    /// Queue a text frame for sending.
    pub async fn send_text(&self, text: impl Into<String>) -> Result<(), NetError> {
        self.0.lock().await.send(Message::Text(text.into().into())).await.map_err(ws_err)
    }

    /// Queue a binary frame for sending.
    pub async fn send_binary(&self, data: Vec<u8>) -> Result<(), NetError> {
        self.0.lock().await.send(Message::Binary(data.into())).await.map_err(ws_err)
    }

    /// Begin the closing handshake.
    pub async fn close(&self) -> Result<(), NetError> {
        self.0.lock().await.send(Message::Close(None)).await.map_err(ws_err)
    }
}

impl WsRx {
    /// Await the next inbound event. Returns `Ok(Closed)` on graceful end.
    pub async fn next(&mut self) -> Result<WsIncoming, NetError> {
        match self.0.next().await {
            Some(Ok(Message::Text(t))) => Ok(WsIncoming::Text(t.to_string())),
            Some(Ok(Message::Binary(b))) => Ok(WsIncoming::Binary(b.to_vec())),
            Some(Ok(Message::Ping(_)) | Ok(Message::Pong(_)) | Ok(Message::Frame(_))) => {
                // Control frames are answered by tungstenite internally.
                Ok(WsIncoming::Closed)
            }
            Some(Ok(Message::Close(_))) => Ok(WsIncoming::Closed),
            Some(Err(e)) => Err(ws_err(e)),
            None => Ok(WsIncoming::Closed),
        }
    }
}

/// Open a `ws://` or `wss://` connection with optional sub-protocols.
pub async fn connect(
    url: &str,
    protocols: &[String],
    user_agent: &str,
    timeout: Duration,
) -> Result<WsConnection, NetError> {
    let mut request =
        url.into_client_request().map_err(|e| NetError::InvalidUrl(format!("{url}: {e}")))?;
    let headers = request.headers_mut();
    headers.insert(
        "user-agent",
        HeaderValue::from_str(user_agent).unwrap_or(HeaderValue::from_static("Brows12")),
    );
    if !protocols.is_empty() {
        let joined = protocols.join(", ");
        let value = HeaderValue::from_str(&joined)
            .map_err(|_| NetError::InvalidUrl(format!("bad sub-protocol: {joined}")))?;
        headers.insert("sec-websocket-protocol", value);
    }

    let (stream, _response) =
        tokio::time::timeout(timeout, tokio_tungstenite::connect_async(request))
            .await
            .map_err(|_| NetError::Http("websocket connect timeout".into()))?
            .map_err(ws_err)?;

    let (tx, rx) = stream.split();
    Ok(WsConnection { tx: WsTx(Arc::new(tokio::sync::Mutex::new(tx))), rx: WsRx(rx) })
}

fn ws_err(e: tokio_tungstenite::tungstenite::Error) -> NetError {
    NetError::Http(format!("websocket: {e}"))
}
