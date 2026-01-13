//! Mantle Eth API extension implementation.

use alloy_eips::BlockNumberOrTag;
use alloy_primitives::Bytes;
use jsonrpsee::types::ErrorObject;
use jsonrpsee_core::RpcResult;
use reth_primitives_traits::TxTy;
use reth_rpc_eth_api::{EthApiServer, EthApiTypes, MantleEthApiExtServer, PreconfTxEvent, RpcBlock, RpcNodeCore};
use reth_rpc_server_types::result::invalid_params_rpc_err;
use reth_storage_api::{BlockNumReader, BlockReaderIdExt, StateProviderFactory};
use std::sync::Arc;

use crate::{error::SequencerClientError, SequencerClient};

/// Mantle-specific `Eth` API extensions implementation.
///
/// This provides Mantle-specific RPC methods such as `getBlockRange` and
/// `sendRawTransactionWithPreconf`.
#[derive(Clone, Debug)]
pub struct MantleEthApiExt<Provider, EthApi> {
    /// The provider type used to interact with the node.
    provider: Provider,
    /// The Eth API used to fetch blocks.
    eth_api: Arc<EthApi>,
    /// Optional sequencer client for forwarding transactions.
    sequencer_client: Option<SequencerClient>,
}

impl<Provider, EthApi> MantleEthApiExt<Provider, EthApi>
where
    Provider: BlockReaderIdExt + StateProviderFactory + Clone + 'static,
{
    /// Creates a new [`MantleEthApiExt`].
    #[allow(clippy::missing_const_for_fn)] // Provider type is generic and cannot be const
    pub fn new(
        provider: Provider,
        eth_api: Arc<EthApi>,
        sequencer_client: Option<SequencerClient>,
    ) -> Self {
        Self { provider, eth_api, sequencer_client }
    }

    #[inline]
    #[allow(clippy::missing_const_for_fn)] // Provider type is generic and cannot be const
    fn provider(&self) -> &Provider {
        &self.provider
    }

    #[inline]
    fn eth_api(&self) -> &EthApi {
        &self.eth_api
    }
}

#[async_trait::async_trait]
impl<Provider, EthApi> MantleEthApiExtServer<RpcBlock<EthApi::NetworkTypes>>
    for MantleEthApiExt<Provider, EthApi>
where
    Provider: BlockReaderIdExt + BlockNumReader + StateProviderFactory + Clone + 'static,
    EthApi: RpcNodeCore
        + EthApiTypes
        + EthApiServer<
            reth_rpc_eth_api::RpcTxReq<EthApi::NetworkTypes>,
            reth_rpc_eth_api::RpcTransaction<EthApi::NetworkTypes>,
            RpcBlock<EthApi::NetworkTypes>,
            reth_rpc_eth_api::RpcReceipt<EthApi::NetworkTypes>,
            reth_rpc_eth_api::RpcHeader<EthApi::NetworkTypes>,
            TxTy<EthApi::Primitives>,
        > + Send
        + Sync
        + 'static,
{
    /// Returns a range of blocks.
    ///
    /// # Deprecation
    ///
    /// This method is deprecated and will be removed in the next network upgrade.
    #[method(name = "getBlockRange")]
    async fn get_block_range(
        &self,
        start_number: BlockNumberOrTag,
        end_number: BlockNumberOrTag,
        full_transactions: bool,
    ) -> RpcResult<Vec<RpcBlock<EthApi::NetworkTypes>>> {
        // Convert BlockNumberOrTag to actual block numbers
        let start = self
            .provider()
            .convert_block_number(start_number)
            .map_err(|e| {
                ErrorObject::owned(
                    -32000,
                    format!("Failed to convert start block number: {e}"),
                    None::<()>,
                )
            })?
            .ok_or_else(|| invalid_params_rpc_err("Start block number not found"))?;

        let end = self
            .provider()
            .convert_block_number(end_number)
            .map_err(|e| {
                ErrorObject::owned(
                    -32000,
                    format!("Failed to convert end block number: {e}"),
                    None::<()>,
                )
            })?
            .ok_or_else(|| invalid_params_rpc_err("End block number not found"))?;

        // Validate: start must be less than or equal to end
        if end < start {
            return Err(invalid_params_rpc_err(format!(
                "start of block range ({start}) is greater than end of block range ({end})"
            )));
        }

        // Validate: range cannot exceed 1000 blocks
        const MAX_BLOCK_RANGE: u64 = 1000;
        let range_size = end.saturating_sub(start).saturating_add(1);
        if range_size > MAX_BLOCK_RANGE {
            return Err(invalid_params_rpc_err(format!(
                "requested block range is too large (max is {MAX_BLOCK_RANGE}, requested {range_size} blocks)"
            )));
        }

        // Validate: end block must exist
        if self.eth_api().block_by_number(end_number, full_transactions).await?.is_none() {
            return Err(invalid_params_rpc_err(format!(
                "end of requested block range ({end}) does not exist"
            )));
        }

        // Collect all blocks in the range
        let mut blocks = Vec::new();
        for block_num in start..=end {
            let block = self
                .eth_api()
                .block_by_number(BlockNumberOrTag::Number(block_num), full_transactions)
                .await?
                .ok_or_else(|| ErrorObject::owned(
                    -32000,
                    format!("block in range not indexed, this should never happen: block {block_num}"),
                    None::<()>
                ))?;
            blocks.push(block);
        }

        Ok(blocks)
    }

    async fn send_raw_transaction_with_preconf(&self, bytes: Bytes) -> RpcResult<PreconfTxEvent> {
        // If we have a sequencer client, forward the transaction to it
        if let Some(sequencer) = self.sequencer_client.as_ref() {
            tracing::debug!(target: "rpc::eth", "forwarding raw transaction with preconf to sequencer");
            sequencer
                .forward_raw_transaction_with_preconf(bytes.as_ref())
                .await
                .map_err(|err| {
                    // Extract the original error message from the sequencer response
                    // SequencerClientError only has one variant (HttpError), so we can directly destructure
                    let SequencerClientError::HttpError(rpc_err) = &err;
                    let error_msg = rpc_err
                        .as_error_resp()
                        .map(|payload| payload.message.to_string())
                        .unwrap_or_else(|| err.to_string());
                    
                    ErrorObject::owned(
                        -32000,
                        format!("failed to forward tx to sequencer, please try again. Error message: '{error_msg}'"),
                        None::<()>,
                    )
                })
        } else {
            // If no sequencer client is available, return an error
            Err(ErrorObject::owned(
                -32000,
                "sendRawTransactionWithPreconf is not yet implemented: sequencer client not configured",
                None::<()>,
            ))
        }
    }
}
