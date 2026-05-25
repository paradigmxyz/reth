//! Helpers for `eth_blockAccessList` RPC method.
use alloy_consensus::BlockHeader;
use alloy_eips::eip7928::{
    AccountChanges, BalanceChange, BlockAccessIndex, BlockAccessList, CodeChange, NonceChange,
    SlotChanges, StorageChange,
};
use alloy_rpc_types_eth::BlockId;
use reth_errors::RethError;
use reth_evm::{block::BlockExecutor, ConfigureEvm, Evm};
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

                // replay all transactions prior to the targeted transaction
                for block_tx in block_txs {
                    executor.execute_transaction(block_tx).map_err(Self::Error::from_eth_err)?;
                    executor.evm_mut().db_mut().bump_bal_index();
                }

                executor
                    .apply_post_execution_changes()
                    .map_err(|err| EthApiError::Internal(err.into()))?;

                let bal = db.take_built_alloy_bal().map(|bal| {
                    bal.into_iter()
                        .map(|account| AccountChanges {
                            address: account.address,
                            storage_changes: account
                                .storage_changes
                                .into_iter()
                                .map(|slot| SlotChanges {
                                    slot: slot.slot,
                                    changes: slot
                                        .changes
                                        .into_iter()
                                        .map(|change| StorageChange {
                                            block_access_index: BlockAccessIndex::new(
                                                change.block_access_index,
                                            ),
                                            new_value: change.new_value,
                                        })
                                        .collect(),
                                })
                                .collect(),
                            storage_reads: account.storage_reads,
                            balance_changes: account
                                .balance_changes
                                .into_iter()
                                .map(|change| BalanceChange {
                                    block_access_index: BlockAccessIndex::new(
                                        change.block_access_index,
                                    ),
                                    post_balance: change.post_balance,
                                })
                                .collect(),
                            nonce_changes: account
                                .nonce_changes
                                .into_iter()
                                .map(|change| NonceChange {
                                    block_access_index: BlockAccessIndex::new(
                                        change.block_access_index,
                                    ),
                                    new_nonce: change.new_nonce,
                                })
                                .collect(),
                            code_changes: account
                                .code_changes
                                .into_iter()
                                .map(|change| CodeChange {
                                    block_access_index: BlockAccessIndex::new(
                                        change.block_access_index,
                                    ),
                                    new_code: change.new_code,
                                })
                                .collect(),
                        })
                        .collect()
                });
                Ok(bal)
            })
            .await
        }
    }
}
