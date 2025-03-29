use crate::BlockProvider;
use alloy_consensus::BlockHeader;
use alloy_eips::BlockNumberOrTag;
use reqwest::Client;
use reth_tracing::tracing::warn;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::{sync::Arc, time::Duration};
use tokio::{sync::mpsc, time::interval};

/// Block provider that fetches new blocks from Etherscan API.
#[derive(derive_more::Debug, Clone)]
pub struct EtherscanBlockProvider<RpcBlock, PrimitiveBlock> {
    http_client: Client,
    base_url: String,
    api_key: String,
    interval: Duration,
    #[debug(skip)]
    convert: Arc<dyn Fn(RpcBlock) -> PrimitiveBlock + Send + Sync>,
}

impl<RpcBlock, PrimitiveBlock> EtherscanBlockProvider<RpcBlock, PrimitiveBlock>
where
    RpcBlock: Serialize + DeserializeOwned,
{
    /// Create a new Etherscan block provider with the given base URL and API key.
    pub fn new(
        base_url: String,
        api_key: String,
        convert: impl Fn(RpcBlock) -> PrimitiveBlock + Send + Sync + 'static,
    ) -> Self {
        Self {
            http_client: Client::new(),
            base_url,
            api_key,
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
    pub async fn load_block(
        &self,
        block_number_or_tag: BlockNumberOrTag,
    ) -> eyre::Result<PrimitiveBlock> {
        let tag = match block_number_or_tag {
            BlockNumberOrTag::Number(num) => format!("{num:#02x}"),
            tag => tag.to_string(),
        };
        let block: EtherscanBlockResponse<RpcBlock> = self
            .http_client
            .get(&self.base_url)
            .query(&[
                ("module", "proxy"),
                ("action", "eth_getBlockByNumber"),
                ("tag", &tag),
                ("boolean", "true"),
                ("apikey", &self.api_key),
            ])
            .send()
            .await?
            .json()
            .await?;
        Ok((self.convert)(block.result))
    }
}

impl<RpcBlock, PrimitiveBlock> BlockProvider for EtherscanBlockProvider<RpcBlock, PrimitiveBlock>
where
    RpcBlock: Serialize + DeserializeOwned + 'static,
    PrimitiveBlock: reth_primitives_traits::Block + 'static,
{
    type Block = PrimitiveBlock;

    async fn subscribe_blocks(&self, tx: mpsc::Sender<Self::Block>) {
        let mut last_block_number: Option<u64> = None;
        let mut interval = interval(self.interval);
        loop {
            interval.tick().await;
            let block = match self.load_block(BlockNumberOrTag::Latest).await {
                Ok(block) => block,
                Err(err) => {
                    warn!(
                        target: "consensus::debug-client",
                        %err,
                        "Failed to fetch a block from Etherscan",
                    );
                    continue
                }
            };
            let block_number = block.header().number();
            if Some(block_number) == last_block_number {
                continue;
            }

            if tx.send(block).await.is_err() {
                // Channel closed.
                break;
            }

            last_block_number = Some(block_number);
        }
    }

    async fn get_block(&self, block_number: u64) -> eyre::Result<Self::Block> {
        self.load_block(BlockNumberOrTag::Number(block_number)).await
    }
}

#[derive(Deserialize, Debug)]
struct EtherscanBlockResponse<B> {
    result: B,
}
