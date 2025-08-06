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
#[derive(Debug, Default, Clone)]
pub struct AddedRemovedKeysSet(B256Set);

impl AddedRemovedKeysSet {
    /// Returns true if the given key path is marked as removed.
    pub fn is_removed(&self, path: &B256) -> bool {
        self.0.contains(path)
    }
}

impl AddedRemovedKeys for &AddedRemovedKeysSet {
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

            if !account.is_empty() {
                self.account.0.remove(hashed_address);
            }
        }
    }

    /// Returns a [`AddedRemovedKeysSet`] for the storage trie of a particular account, if any.
    pub fn get_storage(&self, hashed_address: &B256) -> Option<&AddedRemovedKeysSet> {
        self.storages.get(hashed_address)
    }

    /// Returns an [`AddedRemovedKeysSet`] for tracking account-level changes.
    pub const fn get_accounts(&self) -> &AddedRemovedKeysSet {
        &self.account
    }

    /// Marks an account as existing, and therefore having storage.
    pub fn touch_accounts(&mut self, addresses: impl Iterator<Item = B256>) {
        for address in addresses {
            self.storages.entry(address).or_default();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::HashedStorage;
    use alloy_primitives::U256;
    use reth_primitives_traits::Account;

    #[test]
    fn test_update_with_state_storage_keys_non_zero() {
        let mut multi_keys = MultiAddedRemovedKeys::default();
        let mut update = HashedPostState::default();

        let addr = B256::random();
        let slot1 = B256::random();
        let slot2 = B256::random();

        // First mark slots as removed
        let mut storage = HashedStorage::default();
        storage.storage.insert(slot1, U256::ZERO);
        storage.storage.insert(slot2, U256::ZERO);
        update.storages.insert(addr, storage);
        multi_keys.update_with_state(&update);

        // Verify they are removed
        assert!(multi_keys.get_storage(&addr).unwrap().is_removed(&slot1));
        assert!(multi_keys.get_storage(&addr).unwrap().is_removed(&slot2));

        // Now update with non-zero values
        let mut update2 = HashedPostState::default();
        let mut storage2 = HashedStorage::default();
        storage2.storage.insert(slot1, U256::from(100));
        storage2.storage.insert(slot2, U256::from(200));
        update2.storages.insert(addr, storage2);
        multi_keys.update_with_state(&update2);

        // Slots should no longer be marked as removed
        let storage_keys = multi_keys.get_storage(&addr).unwrap();
        assert!(!storage_keys.is_removed(&slot1));
        assert!(!storage_keys.is_removed(&slot2));
    }

    #[test]
    fn test_update_with_state_wiped_storage() {
        let mut multi_keys = MultiAddedRemovedKeys::default();
        let mut update = HashedPostState::default();

        let addr = B256::random();
        let slot1 = B256::random();

        // First add some removed keys
        let mut storage = HashedStorage::default();
        storage.storage.insert(slot1, U256::ZERO);
        update.storages.insert(addr, storage);
        multi_keys.update_with_state(&update);
        assert!(multi_keys.get_storage(&addr).is_some());

        // Now wipe the storage
        let mut update2 = HashedPostState::default();
        let wiped_storage = HashedStorage::new(true);
        update2.storages.insert(addr, wiped_storage);
        multi_keys.update_with_state(&update2);

        // Storage and account should be removed
        assert!(multi_keys.get_storage(&addr).is_none());
        assert!(multi_keys.get_accounts().is_removed(&addr));
    }

    #[test]
    fn test_update_with_state_account_tracking() {
        let mut multi_keys = MultiAddedRemovedKeys::default();
        let mut update = HashedPostState::default();

        let addr = B256::random();
        let slot = B256::random();

        // Add storage with zero value and empty account
        let mut storage = HashedStorage::default();
        storage.storage.insert(slot, U256::ZERO);
        update.storages.insert(addr, storage);
        // Account is implicitly empty (not in accounts map)

        multi_keys.update_with_state(&update);

        // Storage should have removed keys but account should not be removed
        assert!(multi_keys.get_storage(&addr).unwrap().is_removed(&slot));
        assert!(!multi_keys.get_accounts().is_removed(&addr));

        // Now clear all removed storage keys and keep account empty
        let mut update2 = HashedPostState::default();
        let mut storage2 = HashedStorage::default();
        storage2.storage.insert(slot, U256::from(100)); // Non-zero removes from removed set
        update2.storages.insert(addr, storage2);

        multi_keys.update_with_state(&update2);

        // Account should not be marked as removed still
        assert!(!multi_keys.get_accounts().is_removed(&addr));
    }

    #[test]
    fn test_update_with_state_account_with_balance() {
        let mut multi_keys = MultiAddedRemovedKeys::default();
        let mut update = HashedPostState::default();

        let addr = B256::random();

        // Add account with non-empty state (has balance)
        let account = Account { balance: U256::from(1000), nonce: 0, bytecode_hash: None };
        update.accounts.insert(addr, Some(account));

        // Add empty storage
        let storage = HashedStorage::default();
        update.storages.insert(addr, storage);

        multi_keys.update_with_state(&update);

        // Account should not be marked as removed because it has balance
        assert!(!multi_keys.get_accounts().is_removed(&addr));

        // Now wipe the storage
        let mut update2 = HashedPostState::default();
        let wiped_storage = HashedStorage::new(true);
        update2.storages.insert(addr, wiped_storage);
        update2.accounts.insert(addr, Some(account));
        multi_keys.update_with_state(&update2);

        // Storage should be None, but account should not be removed.
        assert!(multi_keys.get_storage(&addr).is_none());
        assert!(!multi_keys.get_accounts().is_removed(&addr));
    }
}
