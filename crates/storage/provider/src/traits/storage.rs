use std::{
    collections::{BTreeMap, BTreeSet},
    ops::RangeInclusive,
};

use auto_impl::auto_impl;
use reth_interfaces::Result;
use reth_primitives::{Address, BlockNumber, StorageEntry, H256};

/// Storage reader
#[auto_impl(&, Arc, Box)]
pub trait StorageReader: Send + Sync {
    /// Get plainstate storages from an Address.
    fn basic_storages(
        &self,
        iter: impl IntoIterator<Item = (Address, impl IntoIterator<Item = H256>)>,
    ) -> Result<Vec<(Address, Vec<StorageEntry>)>>;

    /// Iterate over storage changesets and return all storage slots that were changed.
    fn changed_storages(
        &self,
        range: RangeInclusive<BlockNumber>,
    ) -> Result<BTreeMap<Address, BTreeSet<H256>>>;

    /// Iterate over storage changesets and return all storage slots that were changed alongside
    /// each specific set of blocks.
    ///
    /// NOTE: Get inclusive range of blocks.
    fn changed_storage_and_blocks_with_range(
        &self,
        range: RangeInclusive<BlockNumber>,
    ) -> Result<BTreeMap<(Address, H256), Vec<u64>>>;
}
