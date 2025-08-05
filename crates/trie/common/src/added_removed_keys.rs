//! Tracking of keys having been added and removed from the tries.

use crate::HashedPostState;
use alloy_primitives::{
    map::{B256Map, B256Set},
    B256, U256,
};
use alloy_trie::{proof::AddedRemovedKeys, Nibbles, TrieMask};

/// Tracks added and removed keys for a single account or storage trie.
///
/// Implements [`AddedRemovedKeys`] in order to be used with a hash builder when generating
/// multiproofs. It always returns true from `is_prefix_added`, because we are not able (yet) to
/// properly track added keys.
///
/// Note: Currently only removed keys are tracked. Added keys tracking is not yet implemented.
///
/// TODO don't derive Clone
#[derive(Debug, Default, Clone)]
pub struct AddedRemovedKeysSet(B256Set);

impl AddedRemovedKeysSet {
    /// TODO document
    pub fn is_removed(&self, path: &B256) -> bool {
        self.0.contains(path)
    }
}

impl AddedRemovedKeys for AddedRemovedKeysSet {
    fn is_prefix_added(&self, _prefix: &Nibbles) -> bool {
        true
    }

    fn get_removed_mask(&self, branch_path: &Nibbles) -> TrieMask {
        let mut mask = TrieMask::default();
        for key in &self.0 {
            let key_nibbles = Nibbles::unpack(key);
            if key_nibbles.starts_with(branch_path) {
                let child_bit = key_nibbles.get_unchecked(branch_path.len());
                mask.set_bit(child_bit);
            }
        }
        mask
    }
}

/// Tracks added and removed keys across account and storage tries.
#[derive(Debug, Default, Clone)]
pub struct MultiAddedRemovedKeys {
    account: AddedRemovedKeysSet,
    storages: B256Map<AddedRemovedKeysSet>,
}

impl MultiAddedRemovedKeys {
    /// Updates the set of removed keys based on a [`HashedPostState`].
    ///
    /// Storage keys set to [`U256::ZERO`] are added to the set for their respective account. Keys
    /// set to any other value are removed from their respective account.
    pub fn update_with_state(&mut self, update: &HashedPostState) {
        for (hashed_address, storage) in &update.storages {
            let account = update
                .accounts
                .get(hashed_address)
                .map(|entry| entry.unwrap_or_default())
                .unwrap_or_default();

            if storage.wiped {
                self.storages.remove(hashed_address);
                if account.is_empty() {
                    self.account.0.insert(*hashed_address);
                }
                continue
            }

            let storage_removed_keys = self.storages.entry(*hashed_address).or_default();

            for (key, val) in &storage.storage {
                if *val == U256::ZERO {
                    storage_removed_keys.0.insert(*key);
                } else {
                    storage_removed_keys.0.remove(key);
                }
            }

            if storage_removed_keys.0.is_empty() && account.is_empty() {
                self.account.0.insert(*hashed_address);
            } else {
                self.account.0.remove(hashed_address);
            }
        }
    }

    /// Returns a [`AddedRemovedKeysSet`] for the storage trie of a particular account.
    pub fn get_storage(&self, hashed_address: &B256) -> AddedRemovedKeysSet {
        self.storages.get(hashed_address).cloned().unwrap_or_default()
    }

    /// Returns an [`AddedRemovedKeysSet`] for tracking account-level changes.
    pub fn get_accounts(&self) -> AddedRemovedKeysSet {
        self.account.clone()
    }
}
