/// Abstractions for managing `WebSocket` connections in the ethstats service.
use crate::error::ConnectionError;
use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use std::sync::Arc;
use tokio::{net::TcpStream, sync::Mutex};
use tokio_tungstenite::{
    tungstenite::protocol::{frame::Utf8Bytes, Message},
    MaybeTlsStream, WebSocketStream,
};

/// Type alias for a `WebSocket` stream that may be TLS or plain TCP
pub(crate) type WsStream = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// Wrapper for a thread-safe, asynchronously accessible `WebSocket` connection
#[derive(Debug, Clone)]
pub struct ConnWrapper {
    /// Shared, mutex-protected `WebSocket` stream
    stream: Arc<Mutex<WsStream>>,
}

impl ConnWrapper {
    /// Create a new connection wrapper from a `WebSocket` stream
    pub fn new(stream: WsStream) -> Self {
        Self { stream: Arc::new(Mutex::new(stream)) }
    }

    /// Write a JSON string as a text message to the `WebSocket`
    pub async fn write_json(&self, value: &str) -> Result<(), ConnectionError> {
        let mut stream = self.stream.lock().await;
        stream.send(Message::Text(Utf8Bytes::from(value))).await?;

        Ok(())
    }

    /// Read the next JSON text message from the `WebSocket`
    ///
    /// Waits for the next text message, parses it as JSON, and returns the value.
    /// Ignores non-text messages. Returns an error if the connection is closed or if parsing fails.
    pub async fn read_json(&self) -> Result<Value, ConnectionError> {
        let mut stream = self.stream.lock().await;
        while let Some(msg) = stream.next().await {
            match msg? {
                Message::Text(text) => return Ok(serde_json::from_str(&text)?),
                Message::Close(_) => return Err(ConnectionError::ConnectionClosed),
                _ => {} // Ignore non-text messages
            }
        }

        Err(ConnectionError::ConnectionClosed)
    }

    /// Close the `WebSocket` connection gracefully
    pub async fn close(&self) -> Result<(), ConnectionError> {
        let mut stream = self.stream.lock().await;
        stream.close(None).await?;

        Ok(())
    }
}
