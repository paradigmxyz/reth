//! Helpers for `eth_blockAccessList` RPC method.
use alloy_consensus::BlockHeader;
use alloy_eips::eip7928::BlockAccessList;
use alloy_primitives::B256;
use reth_evm::ConfigureEvm;
use reth_revm::{database::StateProviderDatabase, State};
use reth_rpc_eth_types::{
    cache::db::StateProviderTraitObjWrapper, error::FromEthApiError, EthApiError,
};
use reth_storage_api::StateProviderFactory;
use revm::DatabaseCommit;

use crate::helpers::{Call, LoadBlock, Trace};

/// Helper trait for `eth_blockAccessList` RPC method.
pub trait GetBlockAccessList: Trace + Call + LoadBlock {
    /// Retrieves the block access list for a block identified by its hash.
    fn get_block_access_list(
        &self,
        block_hash: B256,
    ) -> impl Future<Output = Result<Option<BlockAccessList>, Self::Error>> + Send {
        async move {
            let ((evm_env, _), block) = futures::try_join!(
                self.evm_env_at(block_hash.into()),
                self.recovered_block(block_hash.into()),
            )?;

            let block = block.ok_or_else(|| EthApiError::HeaderNotFound(block_hash.into()))?;

            let bal = self
                .spawn_blocking_io(move |eth_api| {
                    let state = eth_api
                        .provider()
                        .state_by_block_id(block.parent_hash().into())
                        .map_err(Self::Error::from_eth_err)?;

                    let mut db = State::builder()
                        .with_database(StateProviderDatabase::new(StateProviderTraitObjWrapper(
                            state,
                        )))
                        .with_bal_builder()
                        .build();

                    eth_api.apply_pre_execution_changes(&block, &mut db)?;

                    // Move to tx index 1 before first tx (index 0 is for pre-execution)
                    for tx in block.transactions_recovered() {
                        let tx_env = eth_api.evm_config().tx_env(tx);
                        let res = eth_api.transact(&mut db, evm_env.clone(), tx_env)?;
                        db.commit(res.state);
                    }

                    eth_api.apply_post_execution_changes(&block, &mut db)?;
                    // Current index is now n+1 for post-execution

                    let bal = db.take_built_alloy_bal().ok_or_else(|| {
                        EthApiError::Internal(reth_errors::RethError::msg("BAL not built"))
                    })?;

                    return Ok(Some(bal))
                })
                .await;
            bal
        }
    }
}
