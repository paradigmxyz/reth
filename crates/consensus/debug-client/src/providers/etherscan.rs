use crate::PayloadProvider;
use alloy_eips::BlockNumberOrTag;
use alloy_json_rpc::{Response, ResponsePayload};
use futures::future::BoxFuture;
use reqwest::Client;
use reth_node_api::ExecutionPayload;
use serde::{de::DeserializeOwned, Serialize};
use std::{sync::Arc, time::Duration};
use tokio::{sync::mpsc, time::interval};
use tracing::{debug, warn};

/// Block provider that fetches new blocks from Etherscan API.
#[derive(derive_more::Debug)]
pub struct EtherscanBlockProvider<RpcBlock, ExecutionData> {
    http_client: Client,
    base_url: String,
    api_key: String,
    chain_id: u64,
    interval: Duration,
    #[debug(skip)]
    convert: Arc<dyn Fn(RpcBlock) -> ExecutionData + Send + Sync>,
}

impl<RpcBlock, ExecutionData> EtherscanBlockProvider<RpcBlock, ExecutionData>
where
    RpcBlock: Serialize + DeserializeOwned,
{
    /// Create a new Etherscan block provider with the given base URL and API key.
    pub fn new(
        base_url: String,
        api_key: String,
        chain_id: u64,
        convert: impl Fn(RpcBlock) -> ExecutionData + Send + Sync + 'static,
    ) -> Self {
        Self {
            http_client: Client::new(),
            base_url,
            api_key,
            chain_id,
            interval: Duration::from_secs(3),
            convert: Arc::new(convert),
        }
    }

    /// Sets the interval at which the provider fetches new blocks.
    pub const fn with_interval(mut self, interval: Duration) -> Self {
        self.interval = interval;
        self
    }

    /// Load block using Etherscan API. Note: only `BlockNumberOrTag::Latest`,
    /// `BlockNumberOrTag::Earliest`, `BlockNumberOrTag::Pending`, `BlockNumberOrTag::Number(u64)`
    /// are supported.
    pub async fn load_payload(
        &self,
        block_number_or_tag: BlockNumberOrTag,
    ) -> eyre::Result<ExecutionData> {
        let resp = fetch_block(
            &self.http_client,
            &self.base_url,
            &self.api_key,
            self.chain_id,
            block_number_or_tag,
        )
        .await?;

        let resp: Response<RpcBlock> = serde_json::from_str(&resp).inspect_err(|err| {
            warn!(target: "etherscan", "Failed to parse block response from etherscan: {}", err);
        })?;

        let payload = resp.payload;
        match payload {
            ResponsePayload::Success(block) => Ok((self.convert)(block)),
            ResponsePayload::Failure(err) => Err(eyre::eyre!("Failed to get block: {err}")),
        }
    }
}

// Implemented manually so cloning doesn't require `RpcBlock: Clone` and `ExecutionData: Clone`,
// which are only used by the shared conversion function.
impl<RpcBlock, ExecutionData> Clone for EtherscanBlockProvider<RpcBlock, ExecutionData> {
    fn clone(&self) -> Self {
        Self {
            http_client: self.http_client.clone(),
            base_url: self.base_url.clone(),
            api_key: self.api_key.clone(),
            chain_id: self.chain_id,
            interval: self.interval,
            convert: Arc::clone(&self.convert),
        }
    }
}

impl<RpcBlock, ExecutionData> PayloadProvider for EtherscanBlockProvider<RpcBlock, ExecutionData>
where
    RpcBlock: Serialize + DeserializeOwned + 'static,
    ExecutionData: ExecutionPayload,
{
    type ExecutionData = ExecutionData;

    async fn subscribe_payloads(&self, tx: mpsc::Sender<Self::ExecutionData>) {
        let mut last_block_number: Option<u64> = None;
        let mut interval = interval(self.interval);
        loop {
            interval.tick().await;
            let payload = match self.load_payload(BlockNumberOrTag::Latest).await {
                Ok(payload) => payload,
                Err(err) => {
                    warn!(
                        target: "consensus::debug-client",
                        %err,
                        "Failed to fetch a block from Etherscan",
                    );
                    continue
                }
            };
            let block_number = payload.block_number();
            if Some(block_number) == last_block_number {
                continue;
            }

            if tx.send(payload).await.is_err() {
                // Channel closed.
                break;
            }

            last_block_number = Some(block_number);
        }
    }

    async fn get_payload(&self, block_number: u64) -> eyre::Result<Self::ExecutionData> {
        self.load_payload(BlockNumberOrTag::Number(block_number)).await
    }
}

/// Requests a block from Etherscan and returns the raw response body.
///
/// It returns a boxed future because the body of an `async fn` is compiled in every crate that
/// polls it, which would be each binary that instantiates the provider.
fn fetch_block<'a>(
    http_client: &'a Client,
    base_url: &'a str,
    api_key: &'a str,
    chain_id: u64,
    block_number_or_tag: BlockNumberOrTag,
) -> BoxFuture<'a, eyre::Result<String>> {
    Box::pin(async move {
        let tag = match block_number_or_tag {
            BlockNumberOrTag::Number(num) => format!("{num:#x}"),
            tag => tag.to_string(),
        };

        let mut req = http_client.get(base_url).query(&[
            ("module", "proxy"),
            ("action", "eth_getBlockByNumber"),
            ("tag", &tag),
            ("boolean", "true"),
            ("apikey", api_key),
        ]);

        if !base_url.contains("chainid=") {
            // only append chainid if not part of the base url already
            req = req.query(&[("chainid", &chain_id.to_string())]);
        }

        let resp = req.send().await?.text().await?;

        debug!(target: "etherscan", %tag, response_bytes = resp.len(), "fetched block from etherscan");

        Ok(resp)
    })
}
