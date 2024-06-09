use crate::BlockProvider;
use alloy_eips::BlockNumberOrTag;
use reqwest::Client;
use reth_node_core::rpc::types::RichBlock;
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
}

impl BlockProvider for EtherscanBlockProvider {
    async fn subscribe_blocks(&self, tx: mpsc::Sender<RichBlock>) {
        let mut last_block_number: Option<u64> = None;
        let mut interval = interval(self.interval);
        loop {
            interval.tick().await;
            let block = match load_etherscan_block(
                &self.http_client,
                &self.base_url,
                &self.api_key,
                BlockNumberOrTag::Latest,
            )
            .await
            {
                Ok(block) => block,
                Err(err) => {
                    warn!(target: "consensus::debug-client", %err, "failed to fetch a block from Etherscan");
                    continue
                }
            };
            let block_number = block.header.number.unwrap();
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

    async fn get_block(&self, block_number: u64) -> eyre::Result<RichBlock> {
        load_etherscan_block(
            &self.http_client,
            &self.base_url,
            &self.api_key,
            BlockNumberOrTag::Number(block_number),
        )
        .await
    }
}

#[derive(Deserialize, Debug)]
struct EtherscanBlockResponse {
    result: RichBlock,
}

/// Load block using Etherscan API. Note: only `BlockNumberOrTag::Latest`,
/// `BlockNumberOrTag::Earliest`, `BlockNumberOrTag::Pending`, `BlockNumberOrTag::Number(u64)` are
/// supported.
async fn load_etherscan_block(
    http_client: &Client,
    base_url: &str,
    api_key: &str,
    block_number_or_tag: BlockNumberOrTag,
) -> eyre::Result<RichBlock> {
    let block: EtherscanBlockResponse = http_client
        .get(base_url)
        .query(&[
            ("module", "proxy"),
            ("action", "eth_getBlockByNumber"),
            ("tag", &block_number_or_tag.to_string()),
            ("boolean", "true"),
            ("apikey", api_key),
        ])
        .send()
        .await?
        .json()
        .await?;
    Ok(block.result)
}
