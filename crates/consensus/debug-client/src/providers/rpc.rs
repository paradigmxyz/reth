use crate::BlockProvider;
use alloy_eips::BlockNumberOrTag;
use alloy_provider::{Provider, ProviderBuilder};
use futures::StreamExt;
use reth_node_core::rpc::types::RichBlock;
use reth_rpc_types::BlockTransactionsKind;
use tokio::sync::mpsc::Sender;

/// Block provider that fetches new blocks from an RPC endpoint using a websocket connection.
#[derive(Debug, Clone)]
pub struct RpcBlockProvider {
    ws_rpc_url: String,
}

impl RpcBlockProvider {
    /// Create a new RPC block provider with the given WS RPC URL.
    pub const fn new(ws_rpc_url: String) -> Self {
        Self { ws_rpc_url }
    }
}

impl BlockProvider for RpcBlockProvider {
    async fn subscribe_blocks(&self, tx: Sender<RichBlock>) {
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
            let full_block = ws_provider
                .get_block_by_hash(block.header.hash.unwrap(), BlockTransactionsKind::Full)
                .await
                .expect("failed to get block")
                .expect("block not found");
            if tx.send(full_block.into()).await.is_err() {
                // channel closed
                break;
            }
        }
    }

    async fn get_block(&self, block_number: u64) -> eyre::Result<RichBlock> {
        let ws_provider = ProviderBuilder::new()
            .on_builtin(&self.ws_rpc_url)
            .await
            .expect("failed to create WS provider");
        let block: RichBlock = ws_provider
            .get_block_by_number(BlockNumberOrTag::Number(block_number), true)
            .await?
            .ok_or_else(|| eyre::eyre!("block not found by number {}", block_number))?
            .into();
        Ok(block)
    }
}
