// use tokio::sync::mpsc;
use tokio_tungstenite::{connect_async, tungstenite::{Message, Error, client::IntoClientRequest}, WebSocketStream};
use futures_util::{SinkExt, StreamExt};


/// WebSocket connection handler
#[derive(Debug)]
pub struct WsConnection {
    /// The WebSocket stream
    pub ws_stream: WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    // /// Channel for sending messages
    // tx: mpsc::Sender<Message>,
    // /// Channel for receiving messages
    // rx: mpsc::Receiver<Message>,
}

impl WsConnection {
    /// Creates a new WebSocket connection
    pub async fn new(url: &str) -> Result<Self, Error> {
        let request = String::from(url).into_client_request()?;

        let (ws_stream, _) = connect_async(request)
            .await?;
        // let (tx, rx) = mpsc::channel(100);

        Ok(Self { ws_stream })
    }

    /// Sends a message through the WebSocket connection
    pub async fn send(&mut self, msg: Message) -> Result<(), Error> {
        self.ws_stream
            .send(msg)
            .await
    }
}