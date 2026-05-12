//! Helpers for block access lists.
use alloy_consensus::BlockHeader;
use alloy_eips::eip7928::{bal::DecodedBal, BlockAccessList};
use alloy_rpc_types_eth::BlockId;
use reth_errors::RethError;
use reth_evm::{block::BlockExecutor, ConfigureEvm, Evm};
use reth_revm::{database::StateProviderDatabase, state::bal::Bal as RevmBal, State};
use reth_rpc_eth_types::{
    cache::db::StateProviderTraitObjWrapper, error::FromEthApiError, EthApiError, StateCacheDb,
};
use reth_storage_api::StateProviderFactory;
use std::sync::Arc;

use crate::{
    helpers::{Call, LoadBlock, Trace},
    RpcNodeCore,
};

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
                executor.evm_mut().db_mut().bump_bal_index();

                // Advance the BAL index after each transaction so writes are recorded at the
                // matching block access index.
                for block_tx in block_txs {
                    executor.execute_transaction(block_tx).map_err(Self::Error::from_eth_err)?;
                    executor.evm_mut().db_mut().bump_bal_index();
                }

                executor
                    .apply_post_execution_changes()
                    .map_err(|err| EthApiError::Internal(err.into()))?;

                let bal = db.take_built_alloy_bal();
                Ok(bal)
            })
            .await
        }
    }
}

/// Loads the cached block BAL into `db` when it is available.
pub fn attach_cached_block_bal(db: &mut StateCacheDb, bal: Option<Arc<DecodedBal<RevmBal>>>) {
    if let Some(bal) = bal {
        db.set_bal(Some(Arc::new(bal.as_bal().clone())));
    }
}

/// Positions `db` at the state before the transaction at `target_tx_index` if a BAL is attached.
///
/// Returns `true` if the state was positioned with BAL data.
pub fn position_before_transaction(db: &mut StateCacheDb, target_tx_index: u64) -> bool {
    if db.bal_state.bal.is_none() {
        return false
    }

    db.set_bal_index(target_tx_index + 1);
    true
}
