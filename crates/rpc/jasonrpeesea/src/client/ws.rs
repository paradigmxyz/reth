use super::{Client, ClientBuilder, Error};
use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use std::{future::ready, time::Duration};
use tokio_tungstenite::tungstenite::{
    client::IntoClientRequest,
    http::HeaderMap,
    protocol::{Message, WebSocketConfig},
    Error as WsError,
};

/// Builds a `WebSocket` [`Client`].
#[derive(Clone, Debug)]
pub struct WsClientBuilder {
    headers: HeaderMap,
    client: ClientBuilder,
    max_message_size: Option<usize>,
}

impl Default for WsClientBuilder {
    fn default() -> Self {
        Self { headers: HeaderMap::new(), client: ClientBuilder::default(), max_message_size: None }
    }
}

impl WsClientBuilder {
    /// Sets headers sent with the handshake request.
    pub fn set_headers(mut self, headers: HeaderMap) -> Self {
        self.headers = headers;
        self
    }

    /// Sets the request timeout. Default is 60 seconds.
    pub const fn request_timeout(mut self, timeout: Duration) -> Self {
        self.client = self.client.request_timeout(timeout);
        self
    }

    /// Sets the maximum size of a received message. Default is 64 MiB.
    pub const fn max_message_size(mut self, size: usize) -> Self {
        self.max_message_size = Some(size);
        self
    }

    /// Connects to the given URL.
    pub async fn build(self, url: impl AsRef<str>) -> Result<Client, Error> {
        let mut request =
            url.as_ref().into_client_request().map_err(|err| Error::Transport(err.into()))?;
        request.headers_mut().extend(self.headers);
        let config = WebSocketConfig::default()
            .max_message_size(self.max_message_size)
            .max_frame_size(self.max_message_size);
        let (ws, _) = tokio_tungstenite::connect_async_with_config(request, Some(config), true)
            .await
            .map_err(|err| Error::Transport(err.into()))?;
        let (sink, stream) = ws.split();
        let reader = stream.filter_map(|msg| {
            ready(match msg {
                Ok(Message::Text(text)) => Some(Ok(Bytes::from(text))),
                Ok(Message::Binary(bytes)) => Some(Ok(bytes)),
                Ok(_) => None,
                Err(err) => Some(Err(err)),
            })
        });
        let writer = sink.with(|msg: String| ready(Ok::<_, WsError>(Message::text(msg))));
        Ok(self.client.build(Box::pin(reader), Box::pin(writer)))
    }
}

/// A `WebSocket` client.
pub type WsClient = Client;
