// use tokio::sync::mpsc;
use tokio_tungstenite::{connect_async, tungstenite::{Message, Error, client::IntoClientRequest}, WebSocketStream};
use futures_util::{SinkExt, StreamExt};
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::net::TcpStream;
use tokio_tungstenite::MaybeTlsStream;
use serde_json::Value;

pub type WsStream = WebSocketStream<MaybeTlsStream<TcpStream>>;

#[derive(Debug)]
pub struct ConnWrapper {
    stream: Arc<Mutex<WsStream>>, 
}

impl ConnWrapper {
    pub fn new(stream: WsStream) -> Self {
        ConnWrapper {
            stream: Arc::new(Mutex::new(stream)),
        }
    }

    pub async fn write_json(&self, value: &str) -> tokio_tungstenite::tungstenite::Result<()> {
        let mut stream = self.stream.lock().await;
        stream.send(Message::Text(value.to_string())).await
    }

    pub async fn read_json(&self) -> tokio_tungstenite::tungstenite::Result<Value> {
        let mut stream = self.stream.lock().await;
        while let Some(msg) = stream.next().await {
            let msg = msg?;
            if let Message::Text(text) = msg { 
                return Ok(serde_json::from_str(&text)?);
            }
        }
        Err(tokio_tungstenite::tungstenite::Error::ConnectionClosed)
    }

    pub async fn close(&self) -> tokio_tungstenite::tungstenite::Result<()> {
        let mut stream = self.stream.lock().await;
        stream.close(None).await
    }
}