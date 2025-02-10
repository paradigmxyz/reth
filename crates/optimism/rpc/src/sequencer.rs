//! Helpers for optimism specific RPC implementations.

use std::sync::{
    atomic::{self, AtomicUsize},
    Arc,
};

use alloy_primitives::hex;
use alloy_rpc_types_eth::erc4337::TransactionConditional;
use reqwest::Client;
use serde_json::{json, Value};
use tracing::warn;

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
        self.inner.id.fetch_add(1, atomic::Ordering::SeqCst)
    }

    /// Helper function to get body of the request
    fn body_rpc(&self, method: &str, params: Value) -> Value {
        json!({
                "jsonrpc": "2.0",
            "method": method,
            "params": params,
            "id": self.next_request_id()
        })
    }

    /// Sends a POST request to the sequencer endpoint.
    async fn post_request(
        &self,
        body: String,
        error_message: &str,
    ) -> Result<(), SequencerClientError> {
        self.http_client()
            .post(self.endpoint())
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(body)
            .send()
            .await
            .inspect_err(|_| {
                warn!("{}", error_message);
            })?;
        Ok(())
    }
    /// Forwards a transaction conditional to the sequencer endpoint.
    pub async fn forward_raw_transaction_conditional(
        &self,
        tx: &[u8],
        condition: TransactionConditional,
    ) -> Result<(), SequencerClientError> {
        let params = json!([format!("0x{}", hex::encode(tx)), condition]);
        let body = self.body_rpc("eth_sendRawTransaction", params);
        let body_str = serde_json::to_string(&body).map_err(|_| {
            warn!(
                target: "rpc::eth",
                "Failed to serialize transaction conditional for forwarding to sequencer"
            );
            SequencerClientError::InvalidSequencerTransaction
        })?;

        self.post_request(body_str, "Failed to forward transaction conditional to sequencer").await
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
