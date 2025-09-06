use crate::BlockProvider;
use alloy_consensus::BlockHeader;
use alloy_provider::{Network, Provider, ProviderBuilder};
use futures::StreamExt;
use reth_node_api::Block;
use reth_tracing::tracing::warn;
use std::{sync::Arc, time::Duration};
use tokio::{sync::mpsc::Sender, time::interval};

/// Block provider that fetches new blocks from an RPC endpoint.
/// Supports both `WebSocket` (with subscriptions) and HTTP (with polling) connections.
#[derive(derive_more::Debug, Clone)]
pub struct RpcBlockProvider<N: Network, PrimitiveBlock> {
    #[debug(skip)]
    provider: Arc<dyn Provider<N>>,
    url: String,
    is_websocket: bool,
    interval: Duration,
    #[debug(skip)]
    convert: Arc<dyn Fn(N::BlockResponse) -> PrimitiveBlock + Send + Sync>,
}

impl<N: Network, PrimitiveBlock> RpcBlockProvider<N, PrimitiveBlock> {
    /// Create a new RPC block provider with the given RPC URL.
    pub async fn new(
        rpc_url: &str,
        convert: impl Fn(N::BlockResponse) -> PrimitiveBlock + Send + Sync + 'static,
    ) -> eyre::Result<Self> {
        Ok(Self {
            provider: Arc::new(ProviderBuilder::default().connect(rpc_url).await?),
            url: rpc_url.to_string(),
            is_websocket: rpc_url.starts_with("ws://") || rpc_url.starts_with("wss://"),
            interval: Duration::from_secs(3),
            convert: Arc::new(convert),
        })
    }

    /// Sets the polling interval for HTTP connections (ignored for `WebSocket` connections).
    pub const fn with_interval(mut self, interval: Duration) -> Self {
        self.interval = interval;
        self
    }
}

impl<N: Network, PrimitiveBlock> BlockProvider for RpcBlockProvider<N, PrimitiveBlock>
where
    PrimitiveBlock: Block + 'static,
{
    type Block = PrimitiveBlock;

    async fn subscribe_blocks(&self, tx: Sender<Self::Block>) {
        if self.is_websocket {
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
                match self.get_block(header.number()).await {
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
        } else {
            let mut last_block_number: Option<u64> = None;
            let mut interval = interval(self.interval);
            loop {
                interval.tick().await;
                let block_number = match self.provider.get_block_number().await {
                    Ok(number) => number,
                    Err(err) => {
                        warn!(
                            target: "consensus::debug-client",
                            %err,
                            url=%self.url,
                            "Failed to get latest block number",
                        );
                        continue;
                    }
                };

                if Some(block_number) == last_block_number {
                    continue;
                }

                let block = match self.get_block(block_number).await {
                    Ok(block) => block,
                    Err(err) => {
                        warn!(
                            target: "consensus::debug-client",
                            %err,
                            url=%self.url,
                            "Failed to fetch a block",
                        );
                        continue;
                    }
                };

                if tx.send(block).await.is_err() {
                    // Channel closed.
                    break;
                }

                last_block_number = Some(block_number);
            }
        }
    }

    async fn get_block(&self, block_number: u64) -> eyre::Result<Self::Block> {
        let block = self
            .provider
            .get_block_by_number(block_number.into())
            .full()
            .await?
            .ok_or_else(|| eyre::eyre!("block not found by number {}", block_number))?;
        Ok((self.convert)(block))
    }
}
