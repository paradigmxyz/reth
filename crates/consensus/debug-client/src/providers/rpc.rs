use crate::BlockProvider;
use alloy_provider::{Provider, ProviderBuilder};
use alloy_rpc_types_eth::Block;
use futures::StreamExt;
use reth_tracing::tracing::warn;
use std::{fmt, sync::Arc};
use tokio::sync::mpsc::Sender;

/// Block provider that fetches new blocks from an RPC endpoint using a connection that supports
/// RPC subscriptions.
#[derive(Clone)]
pub struct RpcBlockProvider {
    provider: Arc<dyn Provider>,
    url: String,
}

impl fmt::Debug for RpcBlockProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RpcBlockProvider").field("url", &self.url).finish()
    }
}

impl RpcBlockProvider {
    /// Create a new RPC block provider with the given RPC URL.
    pub async fn new(rpc_url: &str) -> eyre::Result<Self> {
        Ok(Self {
            provider: Arc::new(ProviderBuilder::new().on_builtin(rpc_url).await?),
            url: rpc_url.to_string(),
        })
    }
}

impl BlockProvider for RpcBlockProvider {
    async fn subscribe_blocks(&self, tx: Sender<Block>) {
        let mut stream = match self.provider.subscribe_blocks().await {
            Ok(sub) => sub.into_stream(),
            Err(err) => {
                warn!(
                    target: "consensus::debug-client",
                    %err,
                    url=%self.url,
                    "Failed to subscribe to blocks",
                );
                return;
            }
        };
        while let Some(header) = stream.next().await {
            match self.get_block(header.number).await {
                Ok(block) => {
                    if tx.send(block).await.is_err() {
                        // Channel closed.
                        break;
                    }
                }
                Err(err) => {
                    warn!(
                        target: "consensus::debug-client",
                        %err,
                        url=%self.url,
                        "Failed to fetch a block",
                    );
                }
            }
        }
    }

    async fn get_block(&self, block_number: u64) -> eyre::Result<Block> {
        self.provider
            .get_block_by_number(block_number.into(), true.into())
            .await?
            .ok_or_else(|| eyre::eyre!("block not found by number {}", block_number))
    }
}
