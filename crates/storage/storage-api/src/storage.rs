use alloc::{
    collections::{BTreeMap, BTreeSet},
    vec::Vec,
};
use alloy_primitives::{Address, BlockNumber, B256};
use core::ops::RangeInclusive;
use reth_primitives_traits::StorageEntry;
use reth_storage_errors::provider::ProviderResult;

/// Result of pruning one account's storage body while retaining its storage root commitment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PrunedAccountStorage {
    /// Plain account address whose storage was pruned.
    pub address: Address,
    /// Hashed account address used by trie and hashed storage tables.
    pub hashed_address: B256,
    /// Retained storage root for the pruned storage trie.
    pub storage_root: B256,
    /// Number of storage table entries deleted.
    pub deleted_storage_entries: usize,
    /// Number of storage trie nodes deleted, excluding the retained root marker.
    pub deleted_trie_nodes: usize,
}

/// Prunes account storage bodies while preserving trie commitments.
#[auto_impl::auto_impl(&, Arc, Box)]
pub trait AccountStoragePruner: Send {
    /// Prune all storage for `address`, retaining the account's storage root commitment.
    fn prune_account_storage(&self, address: Address) -> ProviderResult<PrunedAccountStorage>;
}

/// Storage reader
#[auto_impl::auto_impl(&, Arc, Box)]
pub trait StorageReader: Send {
    /// Get plainstate storages for addresses and storage keys.
    fn plain_state_storages(
        &self,
        addresses_with_keys: impl IntoIterator<Item = (Address, impl IntoIterator<Item = B256>)>,
    ) -> ProviderResult<Vec<(Address, Vec<StorageEntry>)>>;

    /// Iterate over storage changesets and return all storage slots that were changed.
    fn changed_storages_with_range(
        &self,
        range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<BTreeMap<Address, BTreeSet<B256>>>;

    /// Iterate over storage changesets and return all storage slots that were changed alongside
    /// each specific set of blocks.
    ///
    /// NOTE: Get inclusive range of blocks.
    fn changed_storages_and_blocks_with_range(
        &self,
        range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<BTreeMap<(Address, B256), Vec<u64>>>;
}

/// Storage `ChangeSet` reader
#[cfg(feature = "db-api")]
#[auto_impl::auto_impl(&, Arc, Box)]
pub trait StorageChangeSetReader: Send {
    /// Iterate over storage changesets and return the storage state from before this block.
    fn storage_changeset(
        &self,
        block_number: BlockNumber,
    ) -> ProviderResult<Vec<(reth_db_api::models::BlockNumberAddress, StorageEntry)>>;

    /// Search the block's changesets for the given address and storage key, and return the result.
    ///
    /// Returns `None` if the storage slot was not changed in this block.
    fn get_storage_before_block(
        &self,
        block_number: BlockNumber,
        address: Address,
        storage_key: B256,
    ) -> ProviderResult<Option<StorageEntry>>;

    /// Get all storage changesets in a range of blocks.
    fn storage_changesets_range(
        &self,
        range: impl core::ops::RangeBounds<BlockNumber>,
    ) -> ProviderResult<Vec<(reth_db_api::models::BlockNumberAddress, StorageEntry)>>;

    /// Get storage changesets for a block as static-file rows.
    ///
    /// Default implementation uses `storage_changeset` and maps to `StorageBeforeTx`.
    fn storage_block_changeset(
        &self,
        block_number: BlockNumber,
    ) -> ProviderResult<Vec<reth_db_models::StorageBeforeTx>> {
        self.storage_changeset(block_number).map(|changesets| {
            changesets
                .into_iter()
                .map(|(block_address, entry)| reth_db_models::StorageBeforeTx {
                    address: block_address.address(),
                    key: entry.key,
                    value: entry.value,
                })
                .collect()
        })
    }
}
