use alloy_primitives::{Address, BlockNumber, B256};
use reth_db_api::models::BlockNumberAddress;
use reth_primitives::StorageEntry;
use reth_storage_errors::provider::ProviderResult;
use std::{
    collections::{BTreeMap, BTreeSet},
    ops::RangeInclusive,
};

/// Storage reader
#[auto_impl::auto_impl(&, Arc, Box)]
pub trait StorageReader: Send + Sync {
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

/// Storage ChangeSet reader
#[auto_impl::auto_impl(&, Arc, Box)]
pub trait StorageChangeSetReader: Send + Sync {
    /// Iterate over storage changesets and return the storage state from before this block.
    fn storage_changeset(
        &self,
        block_number: BlockNumber,
    ) -> ProviderResult<Vec<(BlockNumberAddress, StorageEntry)>>;
}
