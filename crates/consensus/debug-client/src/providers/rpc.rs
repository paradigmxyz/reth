use crate::RawBlockProvider;
use alloy_eips::BlockNumberOrTag;
use alloy_provider::{ConnectionConfig, Network, Provider, ProviderBuilder, WebSocketConfig};
use alloy_transport::TransportResult;
use futures::{Stream, StreamExt};
use reth_node_api::Block;
use reth_tracing::tracing::{debug, warn};
use std::sync::Arc;
use tokio::sync::mpsc::Sender;

/// Block provider that fetches new blocks from an RPC endpoint using a connection that supports
/// RPC subscriptions.
#[derive(derive_more::Debug)]
pub struct RpcBlockProvider<N: Network, PrimitiveBlock> {
    #[debug(skip)]
    provider: Arc<dyn Provider<N>>,
    url: String,
    #[debug(skip)]
    convert: Arc<dyn Fn(N::BlockResponse) -> PrimitiveBlock + Send + Sync>,
}

impl<N: Network, PrimitiveBlock> Clone for RpcBlockProvider<N, PrimitiveBlock> {
    fn clone(&self) -> Self {
        Self {
            provider: self.provider.clone(),
            url: self.url.clone(),
            convert: self.convert.clone(),
        }
    }
}

impl<N: Network, PrimitiveBlock> RpcBlockProvider<N, PrimitiveBlock> {
    /// Create a new RPC block provider with the given RPC URL.
    pub async fn new(
        rpc_url: &str,
        convert: impl Fn(N::BlockResponse) -> PrimitiveBlock + Send + Sync + 'static,
    ) -> eyre::Result<Self> {
        Ok(Self {
            provider: Arc::new(
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
            ),
            url: rpc_url.to_string(),
            convert: Arc::new(convert),
        })
    }

    /// Obtains a full block stream.
    ///
    /// This first attempts to obtain an `eth_subscribe` subscription, if that fails because the
    /// connection is not a websocket, this falls back to poll based subscription.
    async fn full_block_stream(
        &self,
    ) -> TransportResult<impl Stream<Item = TransportResult<N::BlockResponse>>> {
        // first try to obtain a regular subscription
        match self.provider.subscribe_full_blocks().full().into_stream().await {
            Ok(sub) => Ok(sub.left_stream()),
            Err(err) => {
                debug!(
                    target: "consensus::debug-client",
                    %err,
                    url=%self.url,
                    "Failed to establish block subscription",
                );
                Ok(self.provider.watch_full_blocks().await?.full().into_stream().right_stream())
            }
        }
    }
}

impl<N: Network, PrimitiveBlock> RawBlockProvider for RpcBlockProvider<N, PrimitiveBlock>
where
    PrimitiveBlock: Block + 'static,
{
    type Block = PrimitiveBlock;

    async fn subscribe_blocks(&self, tx: Sender<Self::Block>) {
        loop {
            let Ok(mut stream) = self.full_block_stream().await.inspect_err(|err| {
                warn!(
                    target: "consensus::debug-client",
                    %err,
                    url=%self.url,
                    "Failed to subscribe to blocks, retrying in 1s",
                );
            }) else {
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                continue
            };

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
            // if stream terminated we want to re-establish it again
            debug!(
                target: "consensus::debug-client",
                url=%self.url,
                "Re-establishing block subscription",
            );
        }
    }

    async fn get_block(&self, block: BlockNumberOrTag) -> eyre::Result<Self::Block> {
        let block = self
            .provider
            .get_block_by_number(block)
            .full()
            .await?
            .ok_or_else(|| eyre::eyre!("block not found for {block}"))?;
        Ok((self.convert)(block))
    }
}
