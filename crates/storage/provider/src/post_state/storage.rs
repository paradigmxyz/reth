use derive_more::Deref;
use reth_primitives::{Address, BlockNumber, U256};
use std::collections::{btree_map::Entry, BTreeMap, HashSet};

/// Storage for an account with the old and new values for each slot: (slot -> (old, new)).
pub type StorageChangeset = BTreeMap<U256, (U256, U256)>;

/// The storage state of the account before the state transition.
#[derive(Debug, Default, Clone, Eq, PartialEq)]
pub struct StorageTransition {
    /// The indicator of the storage wipe.
    pub wipe: StorageWipe,
    /// The storage slots.
    pub storage: BTreeMap<U256, U256>,
}

/// The indicator of the storage wipe.
#[derive(Debug, Default, Clone, Eq, PartialEq)]
pub enum StorageWipe {
    /// The storage was not wiped at this change.
    #[default]
    None,
    /// The storage was wiped for the first time in the current in-memory state.
    ///
    /// When writing history to the database, on the primary storage wipe the pre-existing storage
    /// will be inserted as the storage state before this transition.
    Primary,
    /// The storage had been already wiped before.
    Secondary,
}

impl StorageWipe {
    /// Returns `true` if the wipe occurred at this transition.
    pub fn is_wiped(&self) -> bool {
        matches!(self, Self::Primary | Self::Secondary)
    }

    /// Returns `true` if the primary wiped occurred at this transition.
    /// See [StorageWipe::Primary] for more details.
    pub fn is_primary(&self) -> bool {
        matches!(self, Self::Primary)
    }
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
    pub inner: BTreeMap<BlockNumber, BTreeMap<Address, StorageTransition>>,
    /// Hand tracked change size.
    pub size: usize,
}

impl StorageChanges {
    /// Insert storage entries for specified block number and address.
    pub fn insert_for_block_and_address<I>(
        &mut self,
        block: BlockNumber,
        address: Address,
        wipe: StorageWipe,
        storage: I,
    ) where
        I: Iterator<Item = (U256, U256)>,
    {
        let block_entry = self.inner.entry(block).or_default();
        let storage_entry = block_entry.entry(address).or_default();
        if wipe.is_wiped() {
            storage_entry.wipe = wipe;
        }
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
    ) -> BTreeMap<BlockNumber, BTreeMap<Address, StorageTransition>> {
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
        let mut observed_storage_wipes: HashSet<Address> = HashSet::default();
        self.inner.retain(|block_number, storages| {
            if *block_number > target_block {
                for (address, storage) in storages.iter_mut() {
                    storage.wipe = match storage.wipe {
                        StorageWipe::Primary => {
                            observed_storage_wipes.insert(*address);
                            StorageWipe::Primary
                        }
                        StorageWipe::Secondary => {
                            if observed_storage_wipes.contains(address) {
                                // We already observed the storage wipe for this address
                                StorageWipe::Secondary
                            } else {
                                // No wipe was observed, promote the secondary wipe to primary
                                observed_storage_wipes.insert(*address);
                                StorageWipe::Primary
                            }
                        }
                        StorageWipe::None => StorageWipe::None, // nothing to do
                    };
                }
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
