//! Helpers for `eth_blockAccessList` RPC method.
use alloy_consensus::BlockHeader;
use alloy_eip7928::BlockAccessList;
use alloy_primitives::Bytes;
use alloy_rpc_types_eth::BlockId;
use reth_errors::RethError;
use reth_evm::{
    database::StateProviderDatabase, BlockExecutor, ConfigureEvm, ExecutableTxParts, TxEnvFor,
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
                return Ok(Some((**cached_bal.as_bal()).clone().into()))
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
                        TxEnvFor<Self::Evm>,
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
