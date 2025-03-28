//! Helpers for optimism specific RPC implementations.

use std::sync::Arc;

use alloy_primitives::hex;
use alloy_rpc_client::RpcClient as Client;
use alloy_rpc_types_eth::erc4337::TransactionConditional;
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
        let sequencer_endpoint: String = sequencer_endpoint.into();
        let client = Client::new_http(reqwest::Url::parse(&sequencer_endpoint).unwrap());
        Self::with_client(sequencer_endpoint, client)
    }

    /// Creates a new [`SequencerClient`].
    pub fn with_client(sequencer_endpoint: impl Into<String>, http_client: Client) -> Self {
        let inner =
            SequencerClientInner { sequencer_endpoint: sequencer_endpoint.into(), http_client };
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

    /// Sends a [`RpcCall`] request to the sequencer endpoint.
    async fn send_rpc_call(&self, method: &str, params: Value) -> Result<(), SequencerClientError> {
        self.http_client().request::<Value, ()>(method.to_string(), params).await.inspect_err(
            |err| {
                warn!(
                    target: "rpc::sequencer",
                    %err,
                    "HTTP request to sequencer failed",
                );
            },
        )?;
        Ok(())
    }

    /// Forwards a transaction to the sequencer endpoint.
    pub async fn forward_raw_transaction(&self, tx: &[u8]) -> Result<(), SequencerClientError> {
        self.send_rpc_call("eth_sendRawTransaction", json!([format!("0x{}", hex::encode(tx))]))
            .await
            .inspect_err(|err| {
                warn!(
                    target: "rpc::eth",
                    %err,
                    "Failed to forward transaction to sequencer",
                );
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

        self.send_rpc_call("eth_sendRawTransaction", params).await.inspect_err(|err| {
            warn!(
                target: "rpc::eth",
                %err,
                "Failed to forward transaction conditional for sequencer",
            );
        })?;
        Ok(())
    }
}

#[derive(Debug)]
struct SequencerClientInner {
    /// The endpoint of the sequencer
    sequencer_endpoint: String,
    /// The HTTP client
    http_client: Client,
}

impl Default for SequencerClientInner {
    fn default() -> Self {
        Self {
            sequencer_endpoint: String::new(),
            http_client: Client::new_http(reqwest::Url::parse("http://localhost:8545").unwrap()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_body_str() {
        let client = SequencerClient::new("http://localhost:8545");
        let params = json!(["0x1234", {"block_number":10}]);

        let request = client
            .http_client()
            .make_request("eth_getBlockByNumber", params)
            .serialize()
            .unwrap()
            .take_request();
        let body = request.get();

        assert_eq!(
            body,
            r#"{"method":"eth_getBlockByNumber","params":["0x1234",{"block_number":10}],"id":0,"jsonrpc":"2.0"}"#
        );

        let condition = TransactionConditional::default();
        let params = json!([format!("0x{}", hex::encode("abcd")), condition]);

        let request = client
            .http_client()
            .make_request("eth_sendRawTransactionConditional", params)
            .serialize()
            .unwrap()
            .take_request();
        let body = request.get();

        assert_eq!(
            body,
            r#"{"method":"eth_sendRawTransactionConditional","params":["0x61626364",{"knownAccounts":{}}],"id":1,"jsonrpc":"2.0"}"#
        );
    }
}
