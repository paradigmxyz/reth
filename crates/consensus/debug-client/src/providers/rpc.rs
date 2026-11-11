use crate::PayloadProvider;
use alloy_consensus::BlockHeader;
use alloy_eips::BlockId;
use alloy_provider::{
    network::{primitives::HeaderResponse, AnyNetwork, AnyRpcBlock, AnyRpcHeader, BlockResponse},
    ConnectionConfig, Provider, ProviderBuilder, WebSocketConfig,
};
use alloy_rpc_types_engine::PayloadExtras;
use alloy_transport::TransportResult;
use futures::{future::BoxFuture, stream::BoxStream, StreamExt};
use reth_node_api::ExecutionPayload;
use std::sync::Arc;
use tokio::sync::mpsc::Sender;
use tracing::{debug, warn};

/// Block provider that fetches new blocks from an RPC endpoint using a connection that supports
/// RPC subscriptions.
#[derive(derive_more::Debug, Clone)]
pub struct RpcBlockProvider<ExecutionData> {
    client: AnyRpcClient,
    #[debug(skip)]
    convert: Arc<dyn Fn(AnyRpcBlock, PayloadExtras) -> ExecutionData + Send + Sync>,
}

impl<ExecutionData> RpcBlockProvider<ExecutionData> {
    /// Create a new RPC block provider with the given RPC URL.
    pub async fn new(
        rpc_url: &str,
        convert: impl Fn(AnyRpcBlock, PayloadExtras) -> ExecutionData + Send + Sync + 'static,
    ) -> eyre::Result<Self> {
        Ok(Self { client: AnyRpcClient::connect(rpc_url).await?, convert: Arc::new(convert) })
    }

    /// Disables fetching raw block access list bytes.
    pub const fn without_block_access_lists(mut self) -> Self {
        self.client.fetch_block_access_list = false;
        self
    }
}

impl<ExecutionData> PayloadProvider for RpcBlockProvider<ExecutionData>
where
    ExecutionData: ExecutionPayload,
{
    type ExecutionData = ExecutionData;

    async fn subscribe_payloads(&self, tx: Sender<Self::ExecutionData>) {
        loop {
            let Ok(mut stream) = self.client.full_block_stream().await.inspect_err(|err| {
                warn!(
                    target: "consensus::debug-client",
                    %err,
                    url=%self.client.url,
                    "Failed to subscribe to blocks, retrying in 5s",
                );
            }) else {
                // Exit if the receiver has been dropped (e.g. during shutdown) so we
                // don't keep retrying after the consumer is gone.
                if tx.is_closed() {
                    return;
                }
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                continue
            };

            while let Some(res) = stream.next().await {
                match res {
                    Ok(block) => {
                        let extras = self.client.payload_extras(block.header()).await;
                        let payload = (self.convert)(block, extras);
                        if tx.send(payload).await.is_err() {
                            // Channel closed - receiver dropped, exit completely.
                            return;
                        }
                    }
                    Err(err) => {
                        warn!(
                            target: "consensus::debug-client",
                            %err,
                            url=%self.client.url,
                            "Failed to fetch a block",
                        );
                    }
                }
            }
            // if stream terminated we want to re-establish it again
            debug!(
                target: "consensus::debug-client",
                url=%self.client.url,
                "Re-establishing block subscription",
            );
        }
    }

    async fn get_payload(&self, block_number: u64) -> eyre::Result<Self::ExecutionData> {
        let (block, extras) = self.client.get_block(block_number).await?;
        Ok((self.convert)(block, extras))
    }
}

type BlockStream = BoxStream<'static, TransportResult<AnyRpcBlock>>;

/// Non-generic connection to an RPC endpoint that returns [`AnyNetwork`] blocks.
///
/// The methods called from generic code return boxed futures: the body of an `async fn` is
/// compiled in every crate that polls it, whereas a boxed future is polled through a vtable built
/// here. This keeps the alloy provider, pubsub, and transport code out of downstream binaries.
#[derive(derive_more::Debug, Clone)]
struct AnyRpcClient {
    #[debug(skip)]
    provider: Arc<dyn Provider<AnyNetwork>>,
    url: String,
    fetch_block_access_list: bool,
}

impl AnyRpcClient {
    fn connect(rpc_url: &str) -> BoxFuture<'_, eyre::Result<Self>> {
        Box::pin(async move {
            Ok(Self {
                provider: Arc::new(
                    ProviderBuilder::<_, _, AnyNetwork>::default()
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
                fetch_block_access_list: true,
            })
        })
    }

    /// Obtains a full block stream.
    ///
    /// This first attempts to obtain an `eth_subscribe` subscription, if that fails because the
    /// connection is not a websocket, this falls back to poll based subscription.
    fn full_block_stream(&self) -> BoxFuture<'_, TransportResult<BlockStream>> {
        Box::pin(async move {
            // first try to obtain a regular subscription
            match self.provider.subscribe_full_blocks().full().into_stream().await {
                Ok(sub) => Ok(sub.boxed()),
                Err(err) => {
                    debug!(
                        target: "consensus::debug-client",
                        %err,
                        url=%self.url,
                        "Failed to establish block subscription",
                    );
                    Ok(self.provider.watch_full_blocks().await?.full().into_stream().boxed())
                }
            }
        })
    }

    fn get_block(
        &self,
        block_number: u64,
    ) -> BoxFuture<'_, eyre::Result<(AnyRpcBlock, PayloadExtras)>> {
        Box::pin(async move {
            let block = self
                .provider
                .get_block_by_number(block_number.into())
                .full()
                .await?
                .ok_or_else(|| eyre::eyre!("block not found by number {}", block_number))?;
            let extras = self.payload_extras(block.header()).await;
            Ok((block, extras))
        })
    }

    /// Fetches optional payload side data for blocks that advertise a block access list hash.
    ///
    /// Block access lists are best effort here: RPC providers may not support
    /// `eth_getBlockAccessListByHash`, so failed or missing responses fall back to empty extras.
    fn payload_extras<'a>(&'a self, header: &'a AnyRpcHeader) -> BoxFuture<'a, PayloadExtras> {
        Box::pin(async move {
            if !self.fetch_block_access_list {
                return PayloadExtras::default()
            }

            let block_hash = header.hash();
            if header.block_access_list_hash().is_none() {
                return PayloadExtras::default()
            };

            let block_access_list = self
                .provider
                .get_block_access_list_raw(BlockId::from(block_hash))
                .await
                .inspect_err(|err| {
                    warn!(
                        target: "consensus::debug-client",
                        %err,
                        url=%self.url,
                        %block_hash,
                        "Failed to fetch block access list",
                    );
                })
                .ok()
                .flatten();

            PayloadExtras::from(block_access_list)
        })
    }
}
