use derive_more::Deref;
use reth_primitives::{Address, BlockNumber, U256};
use std::collections::{btree_map::Entry, BTreeMap};

/// Storage for an account with the old and new values for each slot: (slot -> (old, new)).
pub type StorageChangeset = BTreeMap<U256, (U256, U256)>;

/// Changed storage state for the account.
///
/// # Wiped Storage
///
/// The field `wiped` denotes whether the pre-existing storage in the database should be cleared or
/// not.
#[derive(Debug, Default, Clone, Eq, PartialEq)]
pub struct ChangedStorage {
    /// Whether the storage was wiped or not.
    pub wiped: bool,
    /// The storage slots.
    pub storage: BTreeMap<U256, U256>,
}

/// Latest storage state for the account.
///
/// # Wiped Storage
///
/// The `times_wiped` field indicates the number of times the storage was wiped in this poststate.
///
/// If `times_wiped` is greater than 0, then the account was selfdestructed at some point, and the
/// values contained in `storage` should be the only values written to the database.
#[derive(Debug, Default, Clone, Eq, PartialEq)]
pub struct Storage {
    /// The number of times the storage was wiped.
    pub times_wiped: u64,
    /// The storage slots.
    pub storage: BTreeMap<U256, U256>,
}

impl Storage {
    /// Returns `true` if the storage was wiped at any point.
    pub fn wiped(&self) -> bool {
        self.times_wiped > 0
    }
}

/// A mapping of `block -> account -> slot -> old value` that represents what slots were changed,
/// and what their values were prior to that change.
#[derive(Default, Clone, Eq, PartialEq, Debug, Deref)]
pub struct StorageChanges {
    /// The inner mapping of block changes.
    #[deref]
    pub inner: BTreeMap<BlockNumber, BTreeMap<Address, ChangedStorage>>,
    /// Hand tracked change size.
    pub size: usize,
}

impl StorageChanges {
    /// Set storage `wiped` flag for specified block number and address.
    pub fn set_wiped(&mut self, block: BlockNumber, address: Address) {
        self.inner.entry(block).or_default().entry(address).or_default().wiped = true;
    }

    /// Insert storage entries for specified block number and address.
    pub fn insert_for_block_and_address<I>(
        &mut self,
        block: BlockNumber,
        address: Address,
        storage: I,
    ) where
        I: Iterator<Item = (U256, U256)>,
    {
        let block_entry = self.inner.entry(block).or_default();
        let storage_entry = block_entry.entry(address).or_default();
        for (slot, value) in storage {
            if let Entry::Vacant(entry) = storage_entry.storage.entry(slot) {
                entry.insert(value);
                self.size += 1;
            }
        }
    }

    /// Drain and return any entries above the target block number.
    pub fn drain_above(
        &mut self,
        target_block: BlockNumber,
    ) -> BTreeMap<BlockNumber, BTreeMap<Address, ChangedStorage>> {
        let mut evicted = BTreeMap::new();
        self.inner.retain(|block_number, storages| {
            if *block_number > target_block {
                // This is fine, because it's called only on post state splits
                self.size -=
                    storages.iter().fold(0, |acc, (_, storage)| acc + storage.storage.len());
                evicted.insert(*block_number, storages.clone());
                false
            } else {
                true
            }
        });
        evicted
    }

    /// Retain entries only above specified block number.
    pub fn retain_above(&mut self, target_block: BlockNumber) {
        self.inner.retain(|block_number, storages| {
            if *block_number > target_block {
                true
            } else {
                // This is fine, because it's called only on post state splits
                self.size -=
                    storages.iter().fold(0, |acc, (_, storage)| acc + storage.storage.len());
                false
            }
        });
    }
}
