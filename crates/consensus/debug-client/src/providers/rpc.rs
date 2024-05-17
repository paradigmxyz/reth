use crate::BlockProvider;
use alloy_eips::BlockNumberOrTag;
use alloy_provider::{Provider, ProviderBuilder};
use futures::StreamExt;
use reth_node_core::rpc::types::RichBlock;
use tokio::sync::mpsc::Sender;

/// Block provider that fetches new blocks from an RPC endpoint using a websocket connection.
/// HTTP provider is used to fetch full blocks by hash and past blocks by number.
#[derive(Debug)]
pub struct RpcBlockProvider {
    http_rpc_url: String,
    ws_rpc_url: String,
}

impl RpcBlockProvider {
    /// Create a new RPC block provider with the given HTTP and WS RPC URLs.
    pub fn new(http_rpc_url: String, ws_rpc_url: String) -> Self {
        Self { http_rpc_url, ws_rpc_url }
    }
}

impl BlockProvider for RpcBlockProvider {
    async fn spawn(&self, tx: Sender<RichBlock>) {
        let http_provider = ProviderBuilder::new()
            .on_builtin(&self.http_rpc_url)
            .await
            .expect("failed to create HTTP provider");
        let ws_provider = ProviderBuilder::new()
            .on_builtin(&self.ws_rpc_url)
            .await
            .expect("failed to create WS provider");
        let mut stream = ws_provider
            .subscribe_blocks()
            .await
            .expect("failed to subscribe on new blocks")
            .into_stream();

        while let Some(block) = stream.next().await {
            let full_block = http_provider
                .get_block_by_hash(block.header.hash.unwrap(), true)
                .await
                .expect("failed to get block")
                .expect("block not found");
            tx.send(full_block.into()).await.unwrap();
        }
    }

    async fn get_block(&self, block_number: u64) -> eyre::Result<RichBlock> {
        let http_provider = ProviderBuilder::new()
            .on_builtin(&self.http_rpc_url)
            .await
            .expect("failed to create HTTP provider");
        let block: RichBlock = http_provider
            .get_block_by_number(BlockNumberOrTag::Number(block_number), true)
            .await?
            .ok_or_else(|| eyre::eyre!("block not found by number {}", block_number))?
            .into();
        Ok(block)
    }
}
