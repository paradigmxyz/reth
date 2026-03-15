use crate::{BlockProvider, RpcBlockProvider};
use alloy_provider::Network;
use reth_node_api::Block;
use reth_tracing::tracing::{info, warn};
use std::{sync::Arc, time::Duration};
use tokio::sync::mpsc::Sender;

/// Block provider with ordered failover across multiple RPC endpoints.
///
/// Cycles through URLs in priority order, creating a fresh [`RpcBlockProvider`] for each
/// attempt. A per-block timeout detects stale upstreams whose `WebSocket` connections
/// linger without delivering new blocks.
#[derive(derive_more::Debug, Clone)]
pub struct FallbackBlockProvider<N: Network, PrimitiveBlock> {
    urls: Vec<String>,
    #[debug(skip)]
    convert: Arc<dyn Fn(N::BlockResponse) -> PrimitiveBlock + Send + Sync>,
    block_timeout: Duration,
}

impl<N: Network, PrimitiveBlock> FallbackBlockProvider<N, PrimitiveBlock> {
    /// Create a new fallback block provider with the given RPC URLs, block staleness
    /// timeout, and block conversion function.
    pub fn new(
        urls: Vec<String>,
        block_timeout: Duration,
        convert: impl Fn(N::BlockResponse) -> PrimitiveBlock + Send + Sync + 'static,
    ) -> Self {
        Self { urls, convert: Arc::new(convert), block_timeout }
    }
}

impl<N: Network, PrimitiveBlock> BlockProvider for FallbackBlockProvider<N, PrimitiveBlock>
where
    PrimitiveBlock: Block + 'static,
{
    type Block = PrimitiveBlock;

    async fn subscribe_blocks(&self, tx: Sender<Self::Block>) {
        if self.urls.is_empty() {
            warn!(target: "consensus::debug-client", "FallbackBlockProvider has no URLs configured");
            return;
        }

        const RETRY_DELAY: Duration = Duration::from_secs(2);
        let total = self.urls.len();

        loop {
            for (i, url) in self.urls.iter().enumerate() {
                let convert = self.convert.clone();
                let provider =
                    match RpcBlockProvider::<N, PrimitiveBlock>::new(url.as_str(), move |block| {
                        (convert)(block)
                    })
                    .await
                    {
                        Ok(p) => p,
                        Err(err) => {
                            warn!(
                                target: "consensus::debug-client",
                                %err,
                                "[{}/{total}] failed to connect to {url}",
                                i + 1,
                            );
                            tokio::time::sleep(RETRY_DELAY).await;
                            continue;
                        }
                    };

                info!(
                    target: "consensus::debug-client",
                    "[{}/{total}] connected to {url}",
                    i + 1,
                );

                let (inner_tx, mut inner_rx) = tokio::sync::mpsc::channel::<PrimitiveBlock>(64);
                let task = tokio::spawn(async move {
                    provider.subscribe_blocks(inner_tx).await;
                });

                loop {
                    match tokio::time::timeout(self.block_timeout, inner_rx.recv()).await {
                        Ok(Some(block)) => {
                            if tx.send(block).await.is_err() {
                                task.abort();
                                return;
                            }
                        }
                        Ok(None) => {
                            warn!(
                                target: "consensus::debug-client",
                                "[{}/{total}] block subscription ended for {url}",
                                i + 1,
                            );
                            task.abort();
                            break;
                        }
                        Err(_) => {
                            warn!(
                                target: "consensus::debug-client",
                                "[{}/{total}] no blocks in {}s from {url}, failing over",
                                i + 1,
                                self.block_timeout.as_secs(),
                            );
                            task.abort();
                            tokio::time::sleep(RETRY_DELAY).await;
                            break;
                        }
                    }
                }
            }
        }
    }

    async fn get_block(&self, block_number: u64) -> eyre::Result<Self::Block> {
        let total = self.urls.len();
        for (i, url) in self.urls.iter().enumerate() {
            let convert = self.convert.clone();
            let result = tokio::time::timeout(Duration::from_secs(10), async {
                let provider =
                    RpcBlockProvider::<N, PrimitiveBlock>::new(url.as_str(), move |block| {
                        (convert)(block)
                    })
                    .await?;
                provider.get_block(block_number).await
            })
            .await;

            match result {
                Ok(Ok(block)) => return Ok(block),
                Ok(Err(err)) => {
                    warn!(
                        target: "consensus::debug-client",
                        %err,
                        "[{}/{total}] failed to get block {block_number} from {url}",
                        i + 1,
                    );
                }
                Err(_) => {
                    warn!(
                        target: "consensus::debug-client",
                        "[{}/{total}] timed out getting block {block_number} from {url}",
                        i + 1,
                    );
                }
            }
        }
        Err(eyre::eyre!("all {} RPC endpoints failed to get block {}", total, block_number))
    }
}
