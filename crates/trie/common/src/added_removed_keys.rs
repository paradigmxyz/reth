//! Tracking of keys having been added and removed from the tries.

use crate::HashedPostState;
use alloy_primitives::{
    map::{B256Map, B256Set},
    B256, U256,
};
use alloy_trie::{proof::AddedRemovedKeys, Nibbles, TrieMask};

/// Tracks added and removed keys for accounts.
///
/// We don't yet track added keys for the accounts trie, and keys are never deleted, so this only
/// serves to implement [`AddedRemovedKeys`].
#[derive(Debug, Default, Clone)]
pub struct AccountsAddedRemovedKeys;

impl AddedRemovedKeys for AccountsAddedRemovedKeys {
    fn is_prefix_added(&self, _prefix: &Nibbles) -> bool {
        true
    }

    fn get_removed_mask(&self, _branch_path: &Nibbles) -> TrieMask {
        TrieMask::default()
    }
}

/// Tracks added and removed keys for a single storage trie.
///
/// Implements [`AddedRemovedKeys`] in order to be used with a hash builder when generating
/// multiproofs. It always returns true from `is_prefix_added`, because we are not able (yet) to
/// properly track added keys.
///
/// Note: Currently only removed keys are tracked. Added keys tracking is not yet implemented.
///
/// TODO don't derive Clone
#[derive(Debug, Default, Clone)]
pub struct StorageAddedRemovedKeys(B256Set);

impl StorageAddedRemovedKeys {
    /// TODO document
    pub fn is_removed(&self, path: &B256) -> bool {
        self.0.contains(path)
    }
}

impl AddedRemovedKeys for StorageAddedRemovedKeys {
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

/// Tracks added and removed keys across storage tries.
///
/// We don't need to track account keys, because account keys are never deleted, only tombstoned.
///
/// TODO don't derive Clone
#[derive(Debug, Default, Clone)]
pub struct MultiAddedRemovedKeys(B256Map<StorageAddedRemovedKeys>);

impl MultiAddedRemovedKeys {
    /// Updates the set of removed keys based on a [`HashedPostState`].
    ///
    /// Storage keys set to [`U256::ZERO`] are added to the set for their respective account. Keys
    /// set to any other value are removed from their respective account.
    pub fn update_with_state(&mut self, update: &HashedPostState) {
        for (hashed_address, storage) in &update.storages {
            if storage.wiped {
                self.0.remove(hashed_address);
                continue
            }

            let storage_removed_keys = self.0.entry(*hashed_address).or_default();

            for (key, val) in &storage.storage {
                if *val == U256::ZERO {
                    storage_removed_keys.0.insert(*key);
                } else {
                    storage_removed_keys.0.remove(key);
                }
            }
        }
    }

    /// Returns a [`StorageAddedRemovedKeys`] for the storage trie of a particular account.
    pub fn get_storage(&self, hashed_address: &B256) -> StorageAddedRemovedKeys {
        self.0.get(hashed_address).cloned().unwrap_or_default()
    }

    /// Returns an [`AccountsAddedRemovedKeys`] for tracking account-level changes.
    ///
    /// Since accounts are never deleted (only tombstoned), this always returns
    /// a default instance.
    pub const fn get_accounts(&self) -> AccountsAddedRemovedKeys {
        AccountsAddedRemovedKeys
    }
}
