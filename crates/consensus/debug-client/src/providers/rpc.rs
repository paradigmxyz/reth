use crate::PayloadProvider;
use alloy_consensus::BlockHeader;
use alloy_eips::BlockId;
use alloy_primitives::{BlockHash, Bytes, B256};
use alloy_provider::{
    network::{primitives::HeaderResponse, BlockResponse, Network},
    ConnectionConfig, Provider, ProviderBuilder, WebSocketConfig,
};
use alloy_transport::TransportResult;
use eyre::WrapErr;
use futures::{Stream, StreamExt};
use reth_node_api::ExecutionPayload;
use reth_tracing::tracing::{debug, warn};
use std::sync::Arc;
use tokio::sync::mpsc::Sender;

/// Block provider that fetches new blocks from an RPC endpoint using a connection that supports
/// RPC subscriptions.
#[derive(derive_more::Debug, Clone)]
pub struct RpcBlockProvider<N: Network, ExecutionData> {
    #[debug(skip)]
    provider: Arc<dyn Provider<N>>,
    url: String,
    fetch_block_access_list: bool,
    #[debug(skip)]
    convert: Arc<dyn Fn(N::BlockResponse, Option<Bytes>) -> ExecutionData + Send + Sync>,
}

impl<N: Network, ExecutionData> RpcBlockProvider<N, ExecutionData> {
    /// Create a new RPC block provider with the given RPC URL.
    pub async fn new(
        rpc_url: &str,
        convert: impl Fn(N::BlockResponse) -> ExecutionData + Send + Sync + 'static,
    ) -> eyre::Result<Self> {
        Self::new_with_payload_side_data(rpc_url, move |block, _| convert(block)).await
    }
    /// Create a new RPC block provider with the given RPC URL and payload conversion function.
    pub async fn new_with_payload_side_data(
        rpc_url: &str,
        convert: impl Fn(N::BlockResponse, Option<Bytes>) -> ExecutionData + Send + Sync + 'static,
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
            fetch_block_access_list: true,
            convert: Arc::new(convert),
        })
    }

    /// Disables fetching raw block access list bytes.
    pub const fn without_block_access_lists(mut self) -> Self {
        self.fetch_block_access_list = false;
        self
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

    async fn get_payload_from_block(&self, block: N::BlockResponse) -> eyre::Result<ExecutionData> {
        let block_access_list = self.get_block_access_list(block.header()).await?;
        Ok((self.convert)(block, block_access_list))
    }

    async fn get_block_access_list(
        &self,
        header: &N::HeaderResponse,
    ) -> eyre::Result<Option<Bytes>> {
        let block_hash = header.hash();
        let Some(block_access_list_hash) = block_access_list_hash_to_fetch(
            block_hash,
            header.block_access_list_hash(),
            self.fetch_block_access_list,
        )?
        else {
            return Ok(None)
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
            .wrap_err_with(|| {
                format!("failed to fetch block access list for Amsterdam block {block_hash}")
            })?;

        Ok(Some(required_block_access_list(block_hash, block_access_list_hash, block_access_list)?))
    }
}

fn block_access_list_hash_to_fetch(
    block_hash: BlockHash,
    block_access_list_hash: Option<B256>,
    fetch_block_access_list: bool,
) -> eyre::Result<Option<B256>> {
    let Some(block_access_list_hash) = block_access_list_hash else { return Ok(None) };

    if !fetch_block_access_list {
        eyre::bail!(
            "block {block_hash} requires block access list {block_access_list_hash}, but block access list fetching is disabled"
        );
    }

    Ok(Some(block_access_list_hash))
}

fn required_block_access_list(
    block_hash: BlockHash,
    block_access_list_hash: B256,
    block_access_list: Option<Bytes>,
) -> eyre::Result<Bytes> {
    block_access_list.ok_or_else(|| {
        eyre::eyre!(
            "missing block access list for Amsterdam block {block_hash} with block access list hash {block_access_list_hash}"
        )
    })
}

impl<N: Network, ExecutionData> PayloadProvider for RpcBlockProvider<N, ExecutionData>
where
    ExecutionData: ExecutionPayload,
{
    type ExecutionData = ExecutionData;

    async fn subscribe_payloads(&self, tx: Sender<Self::ExecutionData>) {
        loop {
            let Ok(mut stream) = self.full_block_stream().await.inspect_err(|err| {
                warn!(
                    target: "consensus::debug-client",
                    %err,
                    url=%self.url,
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
                    Ok(block) => match self.get_payload_from_block(block).await {
                        Ok(payload) => {
                            if tx.send(payload).await.is_err() {
                                // Channel closed - receiver dropped, exit completely.
                                return;
                            }
                        }
                        Err(err) => {
                            warn!(
                                target: "consensus::debug-client",
                                %err,
                                url=%self.url,
                                "Failed to convert block into execution payload",
                            );
                        }
                    },
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

    async fn get_payload(&self, block_number: u64) -> eyre::Result<Self::ExecutionData> {
        let block = self
            .provider
            .get_block_by_number(block_number.into())
            .full()
            .await?
            .ok_or_else(|| eyre::eyre!("block not found by number {}", block_number))?;
        self.get_payload_from_block(block).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_access_list_hash_to_fetch_rejects_opted_out_fetching_for_amsterdam_blocks() {
        let err = block_access_list_hash_to_fetch(B256::ZERO, Some(B256::with_last_byte(1)), false)
            .unwrap_err();

        assert!(err.to_string().contains("block access list fetching is disabled"));
    }

    #[test]
    fn block_access_list_hash_to_fetch_ignores_pre_amsterdam_blocks() {
        assert_eq!(block_access_list_hash_to_fetch(B256::ZERO, None, false).unwrap(), None);
    }

    #[test]
    fn required_block_access_list_rejects_missing_bal_bytes() {
        let err =
            required_block_access_list(B256::ZERO, B256::with_last_byte(1), None).unwrap_err();

        assert!(err.to_string().contains("missing block access list"));
    }

    #[test]
    fn required_block_access_list_accepts_bal_bytes() {
        let block_access_list = Bytes::from_static(&[0xc0]);

        assert_eq!(
            required_block_access_list(
                B256::ZERO,
                B256::with_last_byte(1),
                Some(block_access_list.clone())
            )
            .unwrap(),
            block_access_list
        );
    }
}
