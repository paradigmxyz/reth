//! Helpers for `eth_blockAccessList` RPC method.
use alloy_consensus::BlockHeader;
use alloy_eips::eip7928::BlockAccessList;
use alloy_rpc_types_eth::BlockId;
use reth_chainspec::{ChainSpecProvider, EthereumHardforks};
use reth_rpc_eth_types::{bal::build_bal_for_block, error::FromEthApiError, EthApiError};
use reth_storage_api::BalProvider;

use crate::{
    helpers::{Call, LoadBlock, Trace},
    RpcNodeCoreExt,
};

/// Helper trait for `eth_blockAccessList` RPC method.
pub trait GetBlockAccessList: Trace + Call + LoadBlock + RpcNodeCoreExt {
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

            ensure_block_access_list_available(self.provider().chain_spec(), block.timestamp())
                .map_err(Self::Error::from_eth_err)?;

            if let Some(cached_bal) =
                self.cache().get_bal(block.hash()).await.map_err(Self::Error::from_eth_err)?
            {
                return Ok(Some(cached_bal.as_bal().clone().into_alloy_bal()))
            }

            self.spawn_blocking_io(move |eth_api| {
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

fn ensure_block_access_list_available(
    chain_spec: impl EthereumHardforks,
    timestamp: u64,
) -> Result<(), EthApiError> {
    if chain_spec.is_amsterdam_active_at_timestamp(timestamp) {
        return Ok(())
    }

    Err(EthApiError::BlockAccessListNotAvailablePreAmsterdam)
}

#[cfg(test)]
mod tests {
    use super::*;
    use reth_chainspec::ChainSpecBuilder;

    #[test]
    fn block_access_list_is_unavailable_before_amsterdam() {
        let chain_spec = ChainSpecBuilder::mainnet().with_amsterdam_at(10).build();

        assert!(matches!(
            ensure_block_access_list_available(&chain_spec, 9),
            Err(EthApiError::BlockAccessListNotAvailablePreAmsterdam)
        ));
        assert!(ensure_block_access_list_available(&chain_spec, 10).is_ok());
    }
}
