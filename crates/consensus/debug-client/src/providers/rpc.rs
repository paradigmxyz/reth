use crate::BlockProvider;
use alloy_provider::{ConnectionConfig, Network, Provider, ProviderBuilder, WebSocketConfig};
use alloy_transport::TransportResult;
use futures::{Stream, StreamExt};
use reth_node_api::Block;
use reth_tracing::tracing::{debug, warn};
use std::sync::Arc;
use tokio::sync::{mpsc::Sender, RwLock};

/// Block provider that fetches new blocks from an RPC endpoint using a connection that supports
/// RPC subscriptions.
#[derive(derive_more::Debug, Clone)]
pub struct RpcBlockProvider<N: Network, PrimitiveBlock> {
    #[debug(skip)]
    provider: Arc<RwLock<Arc<dyn Provider<N>>>>,
    url: String,
    #[debug(skip)]
    convert: Arc<dyn Fn(N::BlockResponse) -> PrimitiveBlock + Send + Sync>,
}

impl<N: Network, PrimitiveBlock> RpcBlockProvider<N, PrimitiveBlock> {
    /// Create a new RPC block provider with the given RPC URL.
    pub async fn new(
        rpc_url: &str,
        convert: impl Fn(N::BlockResponse) -> PrimitiveBlock + Send + Sync + 'static,
    ) -> eyre::Result<Self> {
        let provider = Self::connect(rpc_url).await?;
        Ok(Self {
            provider: Arc::new(RwLock::new(provider)),
            url: rpc_url.to_string(),
            convert: Arc::new(convert),
        })
    }

    /// Establishes a new connection to the RPC endpoint.
    async fn connect(rpc_url: &str) -> eyre::Result<Arc<dyn Provider<N>>> {
        Ok(Arc::new(
            ProviderBuilder::default()
                .connect_with_config(
                    rpc_url,
                    ConnectionConfig::default().with_max_retries(u32::MAX).with_ws_config(
                        WebSocketConfig::default()
                            // allow larger messages/frames for big blocks
                            .max_frame_size(Some(128 * 1024 * 1024))
                            .max_message_size(Some(128 * 1024 * 1024)),
                    ),
                )
                .await?,
        ))
    }

    /// Reconnects to the RPC endpoint, replacing the internal provider with a fresh connection.
    async fn reconnect(&self) -> bool {
        match Self::connect(&self.url).await {
            Ok(new_provider) => {
                *self.provider.write().await = new_provider;
                true
            }
            Err(err) => {
                warn!(
                    target: "consensus::debug-client",
                    %err,
                    url=%self.url,
                    "Failed to reconnect to RPC endpoint",
                );
                false
            }
        }
    }

    /// Obtains a full block stream.
    ///
    /// This first attempts to obtain an `eth_subscribe` subscription, if that fails because the
    /// connection is not a websocket, this falls back to poll based subscription.
    async fn full_block_stream(
        &self,
    ) -> TransportResult<impl Stream<Item = TransportResult<N::BlockResponse>>> {
        let provider = self.provider.read().await.clone();
        // first try to obtain a regular subscription
        match provider.subscribe_full_blocks().full().into_stream().await {
            Ok(sub) => Ok(sub.left_stream()),
            Err(err) => {
                debug!(
                    target: "consensus::debug-client",
                    %err,
                    url=%self.url,
                    "Failed to establish block subscription",
                );
                Ok(provider.watch_full_blocks().await?.full().into_stream().right_stream())
            }
        }
    }
}

impl<N: Network, PrimitiveBlock> BlockProvider for RpcBlockProvider<N, PrimitiveBlock>
where
    PrimitiveBlock: Block + 'static,
{
    type Block = PrimitiveBlock;

    async fn subscribe_blocks(&self, tx: Sender<Self::Block>) {
        let mut retry_delay = std::time::Duration::from_secs(1);
        const MAX_RETRY_DELAY: std::time::Duration = std::time::Duration::from_secs(30);

        loop {
            let Ok(mut stream) = self.full_block_stream().await.inspect_err(|err| {
                warn!(
                    target: "consensus::debug-client",
                    %err,
                    url=%self.url,
                    "Failed to subscribe to blocks, retrying in {:?}",
                    retry_delay,
                );
            }) else {
                // Reconnect with a fresh provider before retrying
                tokio::time::sleep(retry_delay).await;
                retry_delay = (retry_delay * 2).min(MAX_RETRY_DELAY);

                if !self.reconnect().await {
                    continue;
                }
                // Reset delay on successful reconnect
                retry_delay = std::time::Duration::from_secs(1);
                continue;
            };

            // Successfully subscribed — reset retry delay
            retry_delay = std::time::Duration::from_secs(1);

            while let Some(res) = stream.next().await {
                match res {
                    Ok(block) => {
                        if tx.send((self.convert)(block)).await.is_err() {
                            // Channel closed - receiver dropped, exit completely.
                            return;
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
            // Stream terminated (e.g. WS disconnect) — reconnect with a fresh provider
            debug!(
                target: "consensus::debug-client",
                url=%self.url,
                "Block subscription stream ended, reconnecting",
            );
            self.reconnect().await;
        }
    }

    async fn get_block(&self, block_number: u64) -> eyre::Result<Self::Block> {
        let block = self
            .provider
            .read()
            .await
            .get_block_by_number(block_number.into())
            .full()
            .await?
            .ok_or_else(|| eyre::eyre!("block not found by number {}", block_number))?;
        Ok((self.convert)(block))
    }
}
