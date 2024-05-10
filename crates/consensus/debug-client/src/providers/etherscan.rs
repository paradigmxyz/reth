use crate::BlockProvider;
use alloy_eips::BlockNumberOrTag;
use reqwest::Client;
use reth_node_core::rpc::types::RichBlock;
use serde::Deserialize;
use std::time::Duration;
use tokio::{sync::mpsc, time::sleep};

/// Block provider that fetches new blocks from Etherscan API.
#[derive(Debug)]
pub struct EtherscanBlockProvider {
    http_client: Client,
    base_url: String,
    api_key: String,
}

impl EtherscanBlockProvider {
    /// Create a new Etherscan block provider with the given base URL and API key.
    pub fn new(base_url: String, api_key: String) -> Self {
        EtherscanBlockProvider { http_client: Client::new(), base_url, api_key }
    }
}

impl BlockProvider for EtherscanBlockProvider {
    async fn spawn(&self, tx: mpsc::Sender<RichBlock>) {
        let mut last_block_number: Option<u64> = None;
        loop {
            let block = load_etherscan_block(
                &self.http_client,
                &self.base_url,
                &self.api_key,
                BlockNumberOrTag::Latest,
            )
            .await;
            let block_number = block.header.number.unwrap();
            if Some(block_number) == last_block_number {
                // TODO: Make configurable
                sleep(Duration::from_secs(3)).await;
                continue;
            }

            tx.send(block).await.unwrap();
            sleep(Duration::from_secs(3)).await;
            last_block_number = Some(block_number);
        }
    }

    async fn get_block(&self, block_number: u64) -> RichBlock {
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

/// Load block using Etherscan API. Note: only BlockNumberOrTag::Latest, BlockNumberOrTag::Earliest,
/// BlockNumberOrTag::Pending, BlockNumberOrTag::Number(u64) are supported.
async fn load_etherscan_block(
    http_client: &Client,
    base_url: &str,
    api_key: &str,
    block_number_or_tag: BlockNumberOrTag,
) -> RichBlock {
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
        .await
        .unwrap()
        .json()
        .await
        // TODO: Handle errors gracefully and do not stop the loop
        .unwrap();
    block.result
}
