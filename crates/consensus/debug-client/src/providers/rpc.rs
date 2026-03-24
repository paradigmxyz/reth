use crate::BlockProvider;
use alloy_provider::{ConnectionConfig, Network, Provider, ProviderBuilder, WebSocketConfig};
use alloy_pubsub::FallbackPubSubConnect;
use alloy_transport::TransportResult;
use alloy_transport_ws::{WsConnect, WebSocketConfig as WsNativeConfig};
use futures::{Stream, StreamExt};
use reth_node_api::Block;
use reth_tracing::tracing::{debug, info, warn};
use std::sync::Arc;
use tokio::sync::mpsc::Sender;

/// WebSocket config for large blocks (128 MiB frame/message).
fn ws_native_config() -> WsNativeConfig {
    let mut config = WsNativeConfig::default();
    config.max_frame_size = Some(128 * 1024 * 1024);
    config.max_message_size = Some(128 * 1024 * 1024);
    config
}

/// Block provider that fetches new blocks from an RPC endpoint using a connection that supports
/// RPC subscriptions.
#[derive(derive_more::Debug, Clone)]
pub struct RpcBlockProvider<N: Network, PrimitiveBlock> {
    #[debug(skip)]
    provider: Arc<dyn Provider<N>>,
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

    /// Create an RPC block provider with WS fallback across multiple URLs.
    ///
    /// Uses [`FallbackPubSubConnect`] to round-robin across WS endpoints on reconnection,
    /// providing transparent connection-level failover.
    pub async fn new_with_ws_fallback(
        ws_urls: &[String],
        convert: impl Fn(N::BlockResponse) -> PrimitiveBlock + Send + Sync + 'static,
    ) -> eyre::Result<Self> {
        if ws_urls.is_empty() {
            return Err(eyre::eyre!("at least one WS URL is required"));
        }

        let connectors: Vec<WsConnect> = ws_urls
            .iter()
            .map(|url| {
                WsConnect::new(url.as_str())
                    .with_config(ws_native_config())
                    .with_max_retries(u32::MAX)
            })
            .collect();

        let display_urls = ws_urls.join(", ");
        info!(
            target: "consensus::debug-client",
            urls=%display_urls,
            "Creating WS fallback provider with {} endpoints",
            ws_urls.len(),
        );

        let fallback = FallbackPubSubConnect::new(connectors);
        let provider = ProviderBuilder::default().connect_pubsub_with(fallback).await?;

        Ok(Self {
            provider: Arc::new(provider),
            url: display_urls,
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

impl<N: Network, PrimitiveBlock> BlockProvider for RpcBlockProvider<N, PrimitiveBlock>
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
                    "Failed to subscribe to blocks",
                );
            }) else {
                return
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
