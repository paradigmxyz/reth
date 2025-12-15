//! Tracking of keys having been added and removed from the tries.

use crate::HashedPostState;
use alloy_primitives::{map::B256Map, B256};
use alloy_trie::proof::AddedRemovedKeys;

/// Tracks added and removed keys across account and storage tries.
#[derive(Debug, Clone)]
pub struct MultiAddedRemovedKeys {
    account: AddedRemovedKeys,
    storages: B256Map<AddedRemovedKeys>,
}

/// Returns [`AddedRemovedKeys`] with default parameters. This is necessary while we are not yet
/// tracking added keys.
fn default_added_removed_keys() -> AddedRemovedKeys {
    AddedRemovedKeys::default().with_assume_added(true)
}

impl Default for MultiAddedRemovedKeys {
    fn default() -> Self {
        Self::new()
    }
}

impl MultiAddedRemovedKeys {
    /// Returns a new instance.
    pub fn new() -> Self {
        Self { account: default_added_removed_keys(), storages: Default::default() }
    }

    /// Updates the set of removed keys based on a [`HashedPostState`].
    ///
    /// Storage keys set to [`alloy_primitives::U256::ZERO`] are added to the set for their
    /// respective account. Keys set to any other value are removed from their respective
    /// account.
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
                    self.account.insert_removed(*hashed_address);
                }
                continue
            }

            let storage_removed_keys =
                self.storages.entry(*hashed_address).or_insert_with(default_added_removed_keys);

            for (key, val) in &storage.storage {
                if val.is_zero() {
                    storage_removed_keys.insert_removed(*key);
                } else {
                    storage_removed_keys.remove_removed(key);
                }
            }

            if !account.is_empty() {
                self.account.remove_removed(hashed_address);
            }
        }
    }

    /// Returns a [`AddedRemovedKeys`] for the storage trie of a particular account, if any.
    pub fn get_storage(&self, hashed_address: &B256) -> Option<&AddedRemovedKeys> {
        self.storages.get(hashed_address)
    }

    /// Returns an [`AddedRemovedKeys`] for tracking account-level changes.
    pub const fn get_accounts(&self) -> &AddedRemovedKeys {
        &self.account
    }

    /// Marks an account as existing, and therefore having storage.
    pub fn touch_accounts(&mut self, addresses: impl Iterator<Item = B256>) {
        for address in addresses {
            self.storages.entry(address).or_insert_with(default_added_removed_keys);
        }
    }

    /// Marks an account as removed.
    pub fn mark_account_removed(&mut self, account: B256) {
        self.account.insert_removed(account);
    }

    /// Marks a storage slot as removed for the given account.
    pub fn mark_storage_removed(&mut self, hashed_address: B256, slot: B256) {
        self.storages
            .entry(hashed_address)
            .or_insert_with(default_added_removed_keys)
            .insert_removed(slot);
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
        let mut multi_keys = MultiAddedRemovedKeys::new();
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
        let mut multi_keys = MultiAddedRemovedKeys::new();
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
        let mut multi_keys = MultiAddedRemovedKeys::new();
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
    fn test_record_removals_is_monotonic() {
        use alloy_primitives::Address;
        use revm_state::{Account, AccountInfo, AccountStatus, EvmStorageSlot};

        let mut multi_keys = MultiAddedRemovedKeys::new();
        let address = Address::random();
        let slot = U256::from(42);
        let hashed_addr = keccak256(address);
        let hashed_slot = keccak256(B256::from(slot));

        // Update 1: Create slot with value 100
        let mut update1 = EvmState::default();
        update1.insert(
            address,
            Account {
                info: AccountInfo::default(),
                transaction_id: 0,
                storage: std::iter::once((
                    slot,
                    EvmStorageSlot::new_changed(U256::ZERO, U256::from(100), 0),
                ))
                .collect(),
                status: AccountStatus::Touched,
            },
        );
        multi_keys.record_removals(&update1);

        // Slot should NOT be marked as removed (value is 100, not 0)
        assert!(
            multi_keys.get_storage(&hashed_addr).is_none() ||
                !multi_keys.get_storage(&hashed_addr).unwrap().is_removed(&hashed_slot)
        );

        // Update 2: Delete slot (set to 0)
        let mut update2 = EvmState::default();
        update2.insert(
            address,
            Account {
                info: AccountInfo::default(),
                transaction_id: 1,
                storage: std::iter::once((
                    slot,
                    EvmStorageSlot::new_changed(U256::from(100), U256::ZERO, 1),
                ))
                .collect(),
                status: AccountStatus::Touched,
            },
        );
        multi_keys.record_removals(&update2);

        // Slot should be marked as removed
        assert!(multi_keys.get_storage(&hashed_addr).unwrap().is_removed(&hashed_slot));

        // Update 3: Recreate slot with value 200
        let mut update3 = EvmState::default();
        update3.insert(
            address,
            Account {
                info: AccountInfo::default(),
                transaction_id: 2,
                storage: std::iter::once((
                    slot,
                    EvmStorageSlot::new_changed(U256::ZERO, U256::from(200), 2),
                ))
                .collect(),
                status: AccountStatus::Touched,
            },
        );
        multi_keys.record_removals(&update3);

        // KEY ASSERTION: Still removed after recreation!
        // This is the critical difference from update_with_state.
        // Removals are monotonic - once removed, stays removed for proof invalidation.
        assert!(
            multi_keys.get_storage(&hashed_addr).unwrap().is_removed(&hashed_slot),
            "slot should remain marked as removed even after recreation"
        );
    }

    #[test]
    fn test_record_removals_selfdestruct() {
        use alloy_primitives::Address;
        use revm_state::{Account, AccountInfo, AccountStatus};

        let mut multi_keys = MultiAddedRemovedKeys::new();
        let address = Address::random();
        let hashed_addr = keccak256(address);

        // Selfdestruct the account (must also be Touched to be processed)
        let mut update = EvmState::default();
        update.insert(
            address,
            Account {
                info: AccountInfo::default(),
                transaction_id: 0,
                storage: Default::default(),
                status: AccountStatus::SelfDestructed | AccountStatus::Touched,
            },
        );
        multi_keys.record_removals(&update);

        // Account should be marked as removed
        assert!(multi_keys.get_accounts().is_removed(&hashed_addr));
    }

    #[test]
    fn test_record_removals_ignores_untouched() {
        use alloy_primitives::Address;
        use revm_state::{Account, AccountInfo, AccountStatus, EvmStorageSlot};

        let mut multi_keys = MultiAddedRemovedKeys::new();
        let address = Address::random();
        let slot = U256::from(1);
        let hashed_addr = keccak256(address);
        let hashed_slot = keccak256(B256::from(slot));

        // Create an untouched account with zero storage
        let mut update = EvmState::default();
        update.insert(
            address,
            Account {
                info: AccountInfo::default(),
                transaction_id: 0,
                storage: std::iter::once((
                    slot,
                    EvmStorageSlot::new_changed(U256::from(100), U256::ZERO, 0),
                ))
                .collect(),
                status: AccountStatus::default(), // NOT touched
            },
        );
        multi_keys.record_removals(&update);

        // Should NOT be marked as removed because account wasn't touched
        assert!(
            multi_keys.get_storage(&hashed_addr).is_none() ||
                !multi_keys.get_storage(&hashed_addr).unwrap().is_removed(&hashed_slot)
        );
    }

    #[test]
    fn test_update_with_state_account_with_balance() {
        let mut multi_keys = MultiAddedRemovedKeys::new();
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
