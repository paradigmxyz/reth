use crate::BlockProvider;
use alloy_provider::{Network, Provider, ProviderBuilder};
use alloy_transport::TransportResult;
use futures::{Stream, StreamExt};
use reth_node_api::Block;
use reth_tracing::tracing::{debug, warn};
use std::sync::Arc;
use tokio::sync::mpsc::Sender;

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
            provider: Arc::new(ProviderBuilder::default().connect(rpc_url).await?),
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

impl<N: Network, PrimitiveBlock> BlockProvider for RpcBlockProvider<N, PrimitiveBlock>
where
    PrimitiveBlock: Block + 'static,
{
    type Block = PrimitiveBlock;

    async fn subscribe_blocks(&self, tx: Sender<Self::Block>) {
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
