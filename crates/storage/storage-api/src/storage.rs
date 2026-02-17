use alloc::{
    collections::{BTreeMap, BTreeSet},
    vec::Vec,
};
use alloy_primitives::{Address, BlockNumber, B256, U256};
use core::ops::RangeInclusive;
use reth_primitives_traits::{StorageEntry, StorageSlotKey};
use reth_storage_errors::provider::ProviderResult;

/// A storage changeset entry whose key is tagged as [`StorageSlotKey::Plain`] or
/// [`StorageSlotKey::Hashed`] by the reader that produced it.
///
/// Unlike [`StorageEntry`] (the raw DB row type with an untagged `B256` key),
/// this type carries provenance so downstream code can call
/// [`StorageSlotKey::to_hashed`] without consulting `StorageSettings`.
#[derive(Debug, Default, Copy, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ChangesetEntry {
    /// Storage slot key, tagged with its hashing status.
    pub key: StorageSlotKey,
    /// Value at this storage slot before the change.
    pub value: U256,
}

impl ChangesetEntry {
    /// Convert to a raw [`StorageEntry`] (drops the tag).
    pub const fn into_storage_entry(self) -> StorageEntry {
        StorageEntry { key: self.key.as_b256(), value: self.value }
    }
}

impl From<ChangesetEntry> for StorageEntry {
    fn from(e: ChangesetEntry) -> Self {
        e.into_storage_entry()
    }
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
    ///
    /// Returned entries have their keys tagged as [`StorageSlotKey::Plain`] or
    /// [`StorageSlotKey::Hashed`] based on the current storage mode.
    fn storage_changeset(
        &self,
        block_number: BlockNumber,
    ) -> ProviderResult<Vec<(reth_db_api::models::BlockNumberAddress, ChangesetEntry)>>;

    /// Search the block's changesets for the given address and storage key, and return the result.
    ///
    /// The `storage_key` must match the key format used by the storage mode
    /// (plain in v1, keccak256-hashed in v2).
    ///
    /// Returns `None` if the storage slot was not changed in this block.
    fn get_storage_before_block(
        &self,
        block_number: BlockNumber,
        address: Address,
        storage_key: B256,
    ) -> ProviderResult<Option<ChangesetEntry>>;

    /// Get all storage changesets in a range of blocks.
    ///
    /// Returned entries have their keys tagged as [`StorageSlotKey::Plain`] or
    /// [`StorageSlotKey::Hashed`] based on the current storage mode.
    fn storage_changesets_range(
        &self,
        range: impl core::ops::RangeBounds<BlockNumber>,
    ) -> ProviderResult<Vec<(reth_db_api::models::BlockNumberAddress, ChangesetEntry)>>;

    /// Get the total count of all storage changes.
    fn storage_changeset_count(&self) -> ProviderResult<usize>;

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
                    key: entry.key.as_b256(),
                    value: entry.value,
                })
                .collect()
        })
    }
}
