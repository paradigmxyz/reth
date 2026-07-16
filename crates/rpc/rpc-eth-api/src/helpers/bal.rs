//! Helpers for `eth_blockAccessList` RPC method.
use alloy_consensus::BlockHeader;
use alloy_eip7928::{bal::DecodedBal, BlockAccessList};
use alloy_primitives::Bytes;
use alloy_rpc_types_eth::BlockId;
use reth_errors::RethError;
use reth_evm::{
    database::StateProviderDatabase, BlockExecutor, BlockExecutorTransactionFor, ConfigureEvm,
    ExecutableTxParts,
};
use reth_primitives_traits::TxTy;
use reth_rpc_eth_types::{error::FromEthApiError, EthApiError};
use reth_storage_api::StateProviderFactory;

use crate::{
    helpers::{Call, LoadBlock, Trace},
    RpcNodeCore, RpcNodeCoreExt,
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
            if let Some(cached_bal) =
                self.cache().get_bal(block.hash()).await.map_err(Self::Error::from_eth_err)?
            {
                let bal = decode_cached_block_access_list(cached_bal.as_ref())
                    .map_err(RethError::other)
                    .map_err(Self::Error::from_eth_err)?;
                return Ok(Some(bal))
            }

            self.spawn_blocking_io(move |eth_api| {
                let state = eth_api
                    .provider()
                    .state_by_block_id(block.parent_hash().into())
                    .map_err(Self::Error::from_eth_err)?;
                let db = StateProviderDatabase::new(state.as_ref());
                let mut executor = RpcNodeCore::evm_config(&eth_api)
                    .executor_for_block(db, block.sealed_block())
                    .map_err(RethError::other)
                    .map_err(Self::Error::from_eth_err)?;
                executor.enable_block_access_list_builder();
                executor.apply_pre_execution_changes().map_err(Self::Error::from_eth_err)?;
                for block_tx in block.transactions_recovered() {
                    let (tx_env, _) = <_ as ExecutableTxParts<
                        BlockExecutorTransactionFor<Self::Evm>,
                        TxTy<Self::Primitives>,
                    >>::into_parts(block_tx);
                    executor.execute_transaction(tx_env).map_err(Self::Error::from_eth_err)?;
                }
                let (_, bal) =
                    executor.finish_with_block_access_list().map_err(Self::Error::from_eth_err)?;
                Ok(bal)
            })
            .await
        }
    }

    /// Retrieves the raw RLP-encoded block access list for a block.
    fn get_raw_block_access_list(
        &self,
        block_id: BlockId,
    ) -> impl Future<Output = Result<Option<Bytes>, Self::Error>> + Send {
        async move {
            let block = self
                .recovered_block(block_id)
                .await?
                .ok_or_else(|| EthApiError::HeaderNotFound(block_id))?;
            if let Some(cached_bal) =
                self.cache().get_bal(block.hash()).await.map_err(Self::Error::from_eth_err)?
            {
                return Ok(Some(cached_bal.as_raw().clone()))
            }

            Ok(self.get_block_access_list(block_id).await?.map(|bal| alloy_rlp::encode(bal).into()))
        }
    }
}

fn decode_cached_block_access_list<T>(
    cached_bal: &DecodedBal<T>,
) -> Result<BlockAccessList, alloy_rlp::Error> {
    let (bal, _) = DecodedBal::from_rlp_bytes(cached_bal.as_raw().clone())?.split();
    Ok(bal.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_eip7928::{bal::Bal, AccountChanges};
    use alloy_primitives::Address;
    use evm2::evm::Bal as EvmBal;
    use std::sync::Arc;

    #[test]
    fn cached_response_decodes_preserved_raw_bal() {
        let expected = vec![AccountChanges::new(Address::repeat_byte(0x11))];
        let raw = alloy_rlp::encode(Bal::from(expected.clone())).into();
        let cached = DecodedBal::new(Arc::new(EvmBal::default()), raw);

        assert_eq!(decode_cached_block_access_list(&cached).unwrap(), expected);
    }

    #[test]
    fn cached_response_rejects_malformed_raw_bal() {
        let cached = DecodedBal::new(
            Arc::new(EvmBal::default()),
            Bytes::from_static(&[alloy_rlp::EMPTY_STRING_CODE]),
        );

        assert!(decode_cached_block_access_list(&cached).is_err());
    }
}
