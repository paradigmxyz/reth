//! Helpers for block access lists.
use alloy_consensus::BlockHeader;
use alloy_eips::eip7928::BlockAccessList;
use alloy_primitives::B256;
use alloy_rpc_types_eth::BlockId;
use reth_errors::RethError;
use reth_evm::{block::BlockExecutor, ConfigureEvm, Evm};
use reth_revm::{database::StateProviderDatabase, State};
use reth_rpc_eth_types::{
    cache::db::StateProviderTraitObjWrapper, error::FromEthApiError, EthApiError, StateCacheDb,
};
use reth_storage_api::{BalProvider, StateProviderFactory};
use revm::state::bal::Bal;
use std::sync::Arc;
use tracing::debug;

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

/// Loads the block BAL into `db` when it is available.
pub fn attach_block_bal<Provider>(provider: &Provider, block_hash: B256, db: &mut StateCacheDb)
where
    Provider: BalProvider,
{
    if let Some(bal) = load_revm_block_bal(provider, block_hash) {
        db.set_bal(Some(bal));
    }
}

/// Fetches and decodes the block BAL into the revm representation.
pub fn load_revm_block_bal<Provider>(provider: &Provider, block_hash: B256) -> Option<Arc<Bal>>
where
    Provider: BalProvider,
{
    let decoded_bal = match provider.bal_store().get_decoded_by_hash(block_hash) {
        Ok(Some(bal)) => bal,
        Ok(None) => return None,
        Err(err) => {
            debug!(
                target: "reth::rpc",
                ?block_hash,
                %err,
                "Failed to load block access list"
            );
            return None
        }
    };

    match Bal::try_from(decoded_bal.split().0.into_inner()) {
        Ok(bal) => Some(Arc::new(bal)),
        Err(err) => {
            debug!(
                target: "reth::rpc",
                ?block_hash,
                %err,
                "Failed to convert block access list"
            );
            None
        }
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
