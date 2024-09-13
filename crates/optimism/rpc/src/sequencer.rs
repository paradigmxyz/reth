//! Helpers for optimism specific RPC implementations.

use std::sync::{atomic::AtomicUsize, Arc};

use reqwest::Client;

use crate::SequencerClientError;

/// A client to interact with a Sequencer
#[derive(Debug, Clone)]
pub struct SequencerClient {
    inner: Arc<SequencerClientInner>,
}

impl SequencerClient {
    /// Creates a new [`SequencerClient`].
    pub fn new(sequencer_endpoint: impl Into<String>) -> Self {
        let client = Client::builder().use_rustls_tls().build().unwrap();
        Self::with_client(sequencer_endpoint, client)
    }

    /// Creates a new [`SequencerClient`].
    pub fn with_client(sequencer_endpoint: impl Into<String>, http_client: Client) -> Self {
        let inner = SequencerClientInner {
            sequencer_endpoint: sequencer_endpoint.into(),
            http_client,
            id: AtomicUsize::new(0),
        };
        Self { inner: Arc::new(inner) }
    }

    /// Returns the network of the client
    pub fn endpoint(&self) -> &str {
        &self.inner.sequencer_endpoint
    }

    /// Returns the client
    pub fn http_client(&self) -> &Client {
        &self.inner.http_client
    }

    /// Returns the next id for the request
    fn next_request_id(&self) -> usize {
        self.inner.id.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    }

    /// Forwards a transaction to the sequencer endpoint.
    pub async fn forward_raw_transaction(&self, tx: &[u8]) -> Result<(), SequencerClientError> {
        let body = serde_json::to_string(&serde_json::json!({
            "jsonrpc": "2.0",
            "method": "eth_sendRawTransaction",
            "params": [format!("0x{}", alloy_primitives::hex::encode(tx))],
            "id": self.next_request_id()
        }))
        .map_err(|_| {
            tracing::warn!(
                target = "rpc::eth",
                "Failed to serialize transaction for forwarding to sequencer"
            );
            SequencerClientError::InvalidSequencerTransaction
        })?;

        self.http_client()
            .post(self.endpoint())
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(body)
            .send()
            .await
            .inspect_err(|err| {
                tracing::warn!(
                    target = "rpc::eth",
                    %err,
                    "Failed to forward transaction to sequencer",
                );
            })?;

        Ok(())
    }
}

#[derive(Debug, Default)]
struct SequencerClientInner {
    /// The endpoint of the sequencer
    sequencer_endpoint: String,
    /// The HTTP client
    http_client: Client,
    /// Keeps track of unique request ids
    id: AtomicUsize,
}
