/// !Abstractions for managing WebSocket connections in the ethstats service.
use crate::error::ConnectionError;
use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use std::sync::Arc;
use tokio::{net::TcpStream, sync::Mutex};
use tokio_tungstenite::{
    tungstenite::protocol::{frame::Utf8Bytes, Message},
    MaybeTlsStream, WebSocketStream,
};

pub type WsStream = WebSocketStream<MaybeTlsStream<TcpStream>>;
#[derive(Debug, Clone)]
pub struct ConnWrapper {
    stream: Arc<Mutex<WsStream>>,
}

impl ConnWrapper {
    pub fn new(stream: WsStream) -> Self {
        ConnWrapper { stream: Arc::new(Mutex::new(stream)) }
    }

    pub async fn write_json(&self, value: &str) -> Result<(), ConnectionError> {
        let mut stream = self.stream.lock().await;
        stream.send(Message::Text(Utf8Bytes::from(value))).await?;

        Ok(())
    }

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

    pub async fn close(&self) -> Result<(), ConnectionError> {
        let mut stream = self.stream.lock().await;
        stream.close(None).await?;

        Ok(())
    }
}
