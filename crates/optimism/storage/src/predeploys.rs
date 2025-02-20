//! Utilities for accessing Optimism predeploy state

use alloy_primitives::B256;
use reth_optimism_primitives::predeploys::ADDRESS_L2_TO_L1_MESSAGE_PASSER;
use reth_storage_api::{errors::ProviderResult, StorageRootProvider};
use reth_trie_common::HashedStorage;
use revm::database::BundleState;

/// Computes the storage root of predeploy `L2ToL1MessagePasser.sol` with state updates from block
/// execution.
pub fn withdrawals_root<DB: StorageRootProvider>(
    state: DB,
    state_updates: &BundleState,
) -> ProviderResult<B256> {
    // if l2 withdrawals transactions were executed, use predeploy storage updates in storage root
    // computation
    let hashed_storage_updates =
        state_updates.state().get(&ADDRESS_L2_TO_L1_MESSAGE_PASSER).map(|acc| {
            HashedStorage::from_plain_storage(
                acc.status,
                acc.storage.iter().map(|(slot, value)| (slot, &value.present_value)),
            )
        });

    state.storage_root(ADDRESS_L2_TO_L1_MESSAGE_PASSER, hashed_storage_updates.unwrap_or_default())
}
