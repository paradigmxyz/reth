use crate::BlockProvider;
use alloy_eips::BlockNumberOrTag;
use alloy_rpc_types::Block;
use reqwest::Client;
use reth_tracing::tracing::warn;
use serde::Deserialize;
use std::time::Duration;
use tokio::{sync::mpsc, time::interval};

/// Block provider that fetches new blocks from Etherscan API.
#[derive(Debug, Clone)]
pub struct EtherscanBlockProvider {
    http_client: Client,
    base_url: String,
    api_key: String,
    interval: Duration,
}

impl EtherscanBlockProvider {
    /// Create a new Etherscan block provider with the given base URL and API key.
    pub fn new(base_url: String, api_key: String) -> Self {
        Self { http_client: Client::new(), base_url, api_key, interval: Duration::from_secs(3) }
    }

    /// Sets the interval at which the provider fetches new blocks.
    pub const fn with_interval(mut self, interval: Duration) -> Self {
        self.interval = interval;
        self
    }

    /// Load block using Etherscan API. Note: only `BlockNumberOrTag::Latest`,
    /// `BlockNumberOrTag::Earliest`, `BlockNumberOrTag::Pending`, `BlockNumberOrTag::Number(u64)`
    /// are supported.
    pub async fn load_block(&self, block_number_or_tag: BlockNumberOrTag) -> eyre::Result<Block> {
        let block: EtherscanBlockResponse = self
            .http_client
            .get(&self.base_url)
            .query(&[
                ("module", "proxy"),
                ("action", "eth_getBlockByNumber"),
                ("tag", &block_number_or_tag.to_string()),
                ("boolean", "true"),
                ("apikey", &self.api_key),
            ])
            .send()
            .await?
            .json()
            .await?;
        Ok(block.result)
    }
}

impl BlockProvider for EtherscanBlockProvider {
    async fn subscribe_blocks(&self, tx: mpsc::Sender<Block>) {
        let mut last_block_number: Option<u64> = None;
        let mut interval = interval(self.interval);
        loop {
            interval.tick().await;
            let block = match self.load_block(BlockNumberOrTag::Latest).await {
                Ok(block) => block,
                Err(err) => {
                    warn!(target: "consensus::debug-client", %err, "failed to fetch a block from Etherscan");
                    continue
                }
            };
            let block_number = block.header.number;
            if Some(block_number) == last_block_number {
                continue;
            }

            if tx.send(block).await.is_err() {
                // channel closed
                break;
            }

            last_block_number = Some(block_number);
        }
    }

    async fn get_block(&self, block_number: u64) -> eyre::Result<Block> {
        self.load_block(BlockNumberOrTag::Number(block_number)).await
    }
}

#[derive(Deserialize, Debug)]
struct EtherscanBlockResponse {
    result: Block,
}
