use std::{
    collections::{BTreeMap, BTreeSet},
    ops::RangeInclusive,
};

use auto_impl::auto_impl;
use reth_interfaces::RethResult;
use reth_primitives::{Address, BlockNumber, StorageEntry, B256};

/// Storage reader
#[auto_impl(&, Arc, Box)]
pub trait StorageReader: Send + Sync {
    /// Get plainstate storages for addresses and storage keys.
    fn plainstate_storages(
        &self,
        addresses_with_keys: impl IntoIterator<Item = (Address, impl IntoIterator<Item = B256>)>,
    ) -> RethResult<Vec<(Address, Vec<StorageEntry>)>>;

    /// Iterate over storage changesets and return all storage slots that were changed.
    fn changed_storages_with_range(
        &self,
        range: RangeInclusive<BlockNumber>,
    ) -> RethResult<BTreeMap<Address, BTreeSet<B256>>>;

    /// Iterate over storage changesets and return all storage slots that were changed alongside
    /// each specific set of blocks.
    ///
    /// NOTE: Get inclusive range of blocks.
    fn changed_storages_and_blocks_with_range(
        &self,
        range: RangeInclusive<BlockNumber>,
    ) -> RethResult<BTreeMap<(Address, B256), Vec<u64>>>;
}
