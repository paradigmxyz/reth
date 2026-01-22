use alloc::{
    collections::{BTreeMap, BTreeSet},
    vec::Vec,
};
use alloy_primitives::{Address, BlockNumber, B256};
use core::ops::RangeInclusive;
use reth_db_models::StorageBeforeTx;
use reth_primitives_traits::StorageEntry;
use reth_storage_errors::provider::ProviderResult;

/// Storage reader
#[auto_impl::auto_impl(&, Box)]
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
#[auto_impl::auto_impl(&, Box)]
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
    ///
    /// NOTE: Get inclusive range of blocks.
    fn storage_changesets_range(
        &self,
        range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<Vec<(reth_db_api::models::BlockNumberAddress, StorageEntry)>>;

    /// Get the total count of all storage changes.
    fn storage_changeset_count(&self) -> ProviderResult<usize>;

    /// Get storage changesets for a block as static-file rows.
    ///
    /// Default implementation uses `storage_changeset` and maps to `StorageBeforeTx`.
    fn storage_block_changeset(
        &self,
        block_number: BlockNumber,
    ) -> ProviderResult<Vec<StorageBeforeTx>> {
        self.storage_changeset(block_number).map(|changesets| {
            changesets
                .into_iter()
                .map(|(block_address, entry)| StorageBeforeTx {
                    address: block_address.address(),
                    key: entry.key,
                    value: entry.value,
                })
                .collect()
        })
    }
}
