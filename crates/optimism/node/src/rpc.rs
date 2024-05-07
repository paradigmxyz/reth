//! Helpers for optimism specific RPC implementations.

use jsonrpsee::types::ErrorObject;
use reqwest::Client;
use reth_rpc::eth::{
    error::{EthApiError, EthResult},
    traits::RawTransactionForwarder,
};
use reth_rpc_types::ToRpcError;
use std::sync::{atomic::AtomicUsize, Arc};

/// Error type when interacting with the Sequencer
#[derive(Debug, thiserror::Error)]
pub enum SequencerRpcError {
    /// Wrapper around a [hyper::Error].
    #[error(transparent)]
    HyperError(#[from] hyper::Error),
    /// Wrapper around an [reqwest::Error].
    #[error(transparent)]
    HttpError(#[from] reqwest::Error),
    /// Thrown when serializing transaction to forward to sequencer
    #[error("invalid sequencer transaction")]
    InvalidSequencerTransaction,
}

impl ToRpcError for SequencerRpcError {
    fn to_rpc_error(&self) -> ErrorObject<'static> {
        ErrorObject::owned(
            jsonrpsee::types::error::INTERNAL_ERROR_CODE,
            self.to_string(),
            None::<String>,
        )
    }
}

impl From<SequencerRpcError> for EthApiError {
    fn from(err: SequencerRpcError) -> Self {
        EthApiError::other(err)
    }
}

/// A client to interact with a Sequencer
#[derive(Debug, Clone)]
pub struct SequencerClient {
    inner: Arc<SequencerClientInner>,
}

impl SequencerClient {
    /// Creates a new [SequencerClient].
    pub fn new(sequencer_endpoint: impl Into<String>) -> Self {
        let client = Client::builder().use_rustls_tls().build().unwrap();
        Self::with_client(sequencer_endpoint, client)
    }

    /// Creates a new [SequencerClient].
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
    pub async fn forward_raw_transaction(&self, tx: &[u8]) -> Result<(), SequencerRpcError> {
        let body = serde_json::to_string(&serde_json::json!({
            "jsonrpc": "2.0",
            "method": "eth_sendRawTransaction",
            "params": [format!("0x{}", reth_primitives::hex::encode(tx))],
            "id": self.next_request_id()
        }))
        .map_err(|_| {
            tracing::warn!(
                target = "rpc::eth",
                "Failed to serialize transaction for forwarding to sequencer"
            );
            SequencerRpcError::InvalidSequencerTransaction
        })?;

        self.http_client()
            .post(self.endpoint())
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(body)
            .send()
            .await
            .map_err(SequencerRpcError::HttpError)?;

        Ok(())
    }
}

#[async_trait::async_trait]
impl RawTransactionForwarder for SequencerClient {
    async fn forward_raw_transaction(&self, tx: &[u8]) -> EthResult<()> {
        SequencerClient::forward_raw_transaction(self, tx).await?;
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
