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
use std::{sync::Arc, time::Duration};
use tokio::sync::mpsc::Sender;
use tracing::{debug, warn};

/// Block provider that fetches new blocks from an RPC endpoint using a connection that supports
/// RPC subscriptions.
///
/// Only the conversion into `ExecutionData` is generic. Everything that talks to the endpoint
/// lives in a non-generic client, so it is compiled once in this crate instead of in
/// every binary that instantiates the provider.
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
        let mut stream = None;
        // Exit if the receiver has been dropped (e.g. during shutdown) so we don't keep retrying
        // after the consumer is gone.
        while let Some((block, extras)) =
            self.client.next_block(&mut stream, &|| tx.is_closed()).await
        {
            if tx.send((self.convert)(block, extras)).await.is_err() {
                // Channel closed - receiver dropped, exit completely.
                return;
            }
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
/// here.
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
    async fn full_block_stream(&self) -> TransportResult<BlockStream> {
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
    }

    /// Returns the next block of `stream` with its payload extras, (re-)establishing the stream
    /// whenever it is missing or terminated.
    ///
    /// Returns `None` if the stream cannot be established and `is_closed` returns `true`.
    fn next_block<'a>(
        &'a self,
        stream: &'a mut Option<BlockStream>,
        is_closed: &'a (dyn Fn() -> bool + Sync),
    ) -> BoxFuture<'a, Option<(AnyRpcBlock, PayloadExtras)>> {
        Box::pin(async move {
            loop {
                let active = match stream {
                    Some(active) => active,
                    None => match self.full_block_stream().await {
                        Ok(active) => stream.insert(active),
                        Err(err) => {
                            warn!(
                                target: "consensus::debug-client",
                                %err,
                                url=%self.url,
                                "Failed to subscribe to blocks, retrying in 5s",
                            );
                            if is_closed() {
                                return None;
                            }
                            tokio::time::sleep(Duration::from_secs(5)).await;
                            continue
                        }
                    },
                };

                match active.next().await {
                    Some(Ok(block)) => {
                        let extras = self.payload_extras(block.header()).await;
                        return Some((block, extras));
                    }
                    Some(Err(err)) => {
                        warn!(
                            target: "consensus::debug-client",
                            %err,
                            url=%self.url,
                            "Failed to fetch a block",
                        );
                    }
                    None => {
                        // if stream terminated we want to re-establish it again
                        *stream = None;
                        debug!(
                            target: "consensus::debug-client",
                            url=%self.url,
                            "Re-establishing block subscription",
                        );
                    }
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
    async fn payload_extras(&self, header: &AnyRpcHeader) -> PayloadExtras {
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
    }
}
