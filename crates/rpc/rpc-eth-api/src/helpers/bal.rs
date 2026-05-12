//! Helpers for `eth_blockAccessList` RPC method.
use alloy_consensus::BlockHeader;
use alloy_eips::eip7928::BlockAccessList;
use alloy_rpc_types_eth::BlockId;
use reth_chainspec::{ChainSpecProvider, EthereumHardforks};
use reth_rpc_eth_types::{bal::build_bal_for_block, error::FromEthApiError, EthApiError};
use reth_storage_api::BalProvider;

use crate::helpers::{Call, LoadBlock, Trace};

/// Helper trait for `eth_blockAccessList` RPC method.
pub trait GetBlockAccessList: Trace + Call + LoadBlock {
    /// Retrieves the block access list for a block identified by its hash.
    fn get_block_access_list(
        &self,
        block_id: BlockId,
    ) -> impl Future<Output = Result<Option<BlockAccessList>, Self::Error>> + Send {
        async move {
            let block = self
                .recovered_block(block_id)
                .await?
                .ok_or_else(|| EthApiError::HeaderNotFound(block_id))?;

            self.spawn_blocking_io(move |eth_api| {
                let timestamp = block.timestamp();
                if !eth_api.provider().chain_spec().is_amsterdam_active_at_timestamp(timestamp) {
                    return Ok(None)
                }

                if let Some(bal) = eth_api
                    .provider()
                    .bal_store()
                    .get_decoded_by_hash(block.hash())
                    .map_err(Self::Error::from_eth_err)?
                {
                    return Ok(Some(bal.split().0.into_inner()))
                }

                build_bal_for_block(eth_api.provider(), eth_api.evm_config(), &block)
                    .map(Some)
                    .map_err(Self::Error::from_eth_err)
            })
            .await
        }
    }
}
