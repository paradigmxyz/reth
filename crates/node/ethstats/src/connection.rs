//! Support for websockets
use alloy_pubsub::PubSubConnect;
use alloy_transport::{TransportErrorKind, TransportResult};

/// Simple connection info for the websocket.
#[derive(Clone, Debug)]
pub struct WsConnect {
    /// The URL to connect to.
    pub url: String,
}

impl WsConnect {
    /// Creates a new websocket connection configuration.
    pub fn new<S: Into<String>>(url: S) -> Self {
        Self { url: url.into() }
    }
}

impl PubSubConnect for WsConnect {
    fn is_local(&self) -> bool {
        alloy_transport::utils::guess_local_url(&self.url)
    }

    async fn connect(&self) -> TransportResult<alloy_pubsub::ConnectionHandle> {
        tokio_tungstenite::connect_async(&self.url).await.map_err(TransportErrorKind::custom)?;
        let (handle, _) = alloy_pubsub::ConnectionHandle::new();
        Ok(handle)
    }
}
