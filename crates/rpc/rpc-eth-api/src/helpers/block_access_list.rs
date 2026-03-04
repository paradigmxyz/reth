//! Helpers for `eth_blockAccessList` RPC method.
use alloy_consensus::BlockHeader;
use alloy_eips::eip7928::BlockAccessList;
use alloy_primitives::B256;
use reth_errors::RethError;
use reth_evm::{block::BlockExecutor, ConfigureEvm};
use reth_revm::{database::StateProviderDatabase, State};
use reth_rpc_eth_types::{
    cache::db::StateProviderTraitObjWrapper, error::FromEthApiError, EthApiError,
};
use reth_storage_api::StateProviderFactory;

use crate::{
    helpers::{Call, LoadBlock, Trace},
    RpcNodeCore,
};

/// Helper trait for `eth_blockAccessList` RPC method.
pub trait GetBlockAccessList: Trace + Call + LoadBlock {
    /// Retrieves the block access list for a block identified by its hash.
    fn get_block_access_list(
        &self,
        block_hash: B256,
    ) -> impl Future<Output = Result<Option<BlockAccessList>, Self::Error>> + Send {
        async move {
            let block = self
                .recovered_block(block_hash.into())
                .await?
                .ok_or_else(|| EthApiError::HeaderNotFound(block_hash.into()))?;

            self.spawn_blocking_io(move |eth_api| {
                let state = eth_api
                    .provider()
                    .state_by_block_id(block.parent_hash().into())
                    .map_err(Self::Error::from_eth_err)?;

                let mut db = State::builder()
                    .with_database(StateProviderDatabase::new(StateProviderTraitObjWrapper(state)))
                    .with_bal_builder()
                    .build();

                let block_txs = block.transactions_recovered();
                let mut executor = RpcNodeCore::evm_config(&eth_api)
                    .executor_for_block(&mut db, block.sealed_block())
                    .map_err(RethError::other)
                    .map_err(Self::Error::from_eth_err)?;

                executor.apply_pre_execution_changes().map_err(Self::Error::from_eth_err)?;

                // replay all transactions prior to the targeted transaction
                for block_tx in block_txs {
                    executor.execute_transaction(block_tx).map_err(Self::Error::from_eth_err)?;
                }
                let _ = executor.apply_post_execution_changes();

                let bal = db.take_built_alloy_bal().ok_or_else(|| {
                    EthApiError::Internal(reth_errors::RethError::msg("BAL not built"))
                })?;

                Ok(Some(bal))
            })
            .await
        }
    }
}
