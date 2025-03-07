use crate::BlockProvider;
use alloy_consensus::BlockHeader;
use alloy_provider::{Network, Provider, ProviderBuilder};
use futures::StreamExt;
use reth_node_api::Block;
use reth_tracing::tracing::warn;
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
            provider: Arc::new(
                ProviderBuilder::new()
                    .disable_recommended_fillers()
                    .network::<N>()
                    .connect(rpc_url)
                    .await?,
            ),
            url: rpc_url.to_string(),
            convert: Arc::new(convert),
        })
    }
}

impl<N: Network, PrimitiveBlock> BlockProvider for RpcBlockProvider<N, PrimitiveBlock>
where
    PrimitiveBlock: Block + 'static,
{
    type Block = PrimitiveBlock;

    async fn subscribe_blocks(&self, tx: Sender<Self::Block>) {
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
