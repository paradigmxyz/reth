//! Shared state for multiproof and sparse trie coordination.
//!
//! This module provides lock-free shared state that enables parallel proof computation
//! and direct communication between workers and the sparse trie task, eliminating
//! the multiproof task as a bottleneck.

use alloy_primitives::{map::B256Set, B256};
use alloy_trie::proof::AddedRemovedKeys;
use dashmap::DashMap;
use reth_trie::{HashedPostState, MultiProofTargets};
use std::sync::Arc;

/// Returns [`AddedRemovedKeys`] with default parameters.
fn default_added_removed_keys() -> AddedRemovedKeys {
    AddedRemovedKeys::default().with_assume_added(true)
}

/// Shared proof state that coordinates between multiproof dispatching and workers.
///
/// This structure is designed to be shared across multiple threads:
/// - `MultiProofTask` reads to determine what proofs to dispatch
/// - Workers update after computing proofs
/// - Eliminates cloning of `MultiAddedRemovedKeys` on every dispatch
#[derive(Debug, Default)]
pub struct SharedProofState {
    /// Already fetched proof targets - prevents re-fetching.
    /// Key: hashed address, Value: set of storage slots fetched for that address.
    fetched_targets: DashMap<B256, B256Set>,

    /// Tracks added/removed keys for account trie.
    account_added_removed: AddedRemovedKeysShared,

    /// Tracks added/removed keys for storage tries per account.
    storage_added_removed: DashMap<B256, AddedRemovedKeysShared>,
}

/// Thread-safe wrapper around AddedRemovedKeys using interior mutability.
#[derive(Debug, Default)]
pub struct AddedRemovedKeysShared {
    inner: parking_lot::RwLock<AddedRemovedKeys>,
}

impl AddedRemovedKeysShared {
    /// Creates a new instance with default settings.
    pub fn new() -> Self {
        Self { inner: parking_lot::RwLock::new(default_added_removed_keys()) }
    }

    /// Inserts a key as removed.
    pub fn insert_removed(&self, key: B256) {
        self.inner.write().insert_removed(key);
    }

    /// Removes a key from the removed set.
    pub fn remove_removed(&self, key: &B256) {
        self.inner.write().remove_removed(key);
    }

    /// Checks if a key is marked as removed.
    pub fn is_removed(&self, key: &B256) -> bool {
        self.inner.read().is_removed(key)
    }

    /// Returns a clone of the inner AddedRemovedKeys.
    pub fn clone_inner(&self) -> AddedRemovedKeys {
        self.inner.read().clone()
    }
}

impl SharedProofState {
    /// Creates a new shared proof state.
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            fetched_targets: DashMap::new(),
            account_added_removed: AddedRemovedKeysShared::new(),
            storage_added_removed: DashMap::new(),
        })
    }

    /// Returns true if the given account has already been fetched.
    pub fn is_account_fetched(&self, hashed_address: &B256) -> bool {
        self.fetched_targets.contains_key(hashed_address)
    }

    /// Returns true if the given storage slot for an account has already been fetched.
    pub fn is_storage_slot_fetched(&self, hashed_address: &B256, slot: &B256) -> bool {
        self.fetched_targets
            .get(hashed_address)
            .is_some_and(|slots| slots.contains(slot))
    }

    /// Marks an account as fetched (without any specific storage slots).
    pub fn mark_account_fetched(&self, hashed_address: B256) {
        self.fetched_targets.entry(hashed_address).or_default();
    }

    /// Marks storage slots as fetched for an account.
    pub fn mark_storage_fetched(&self, hashed_address: B256, slots: impl Iterator<Item = B256>) {
        self.fetched_targets.entry(hashed_address).or_default().extend(slots);
    }

    /// Marks all targets as fetched.
    pub fn mark_targets_fetched(&self, targets: &MultiProofTargets) {
        for (hashed_address, slots) in targets.iter() {
            self.fetched_targets.entry(*hashed_address).or_default().extend(slots.iter().copied());
        }
    }

    /// Marks an account as existing (touched), ensuring it has an entry for storage tracking.
    pub fn touch_account(&self, hashed_address: B256) {
        self.storage_added_removed
            .entry(hashed_address)
            .or_insert_with(AddedRemovedKeysShared::new);
    }

    /// Marks multiple accounts as existing.
    pub fn touch_accounts(&self, addresses: impl Iterator<Item = B256>) {
        for address in addresses {
            self.touch_account(address);
        }
    }

    /// Updates added/removed keys based on a state update.
    ///
    /// Storage keys set to zero are marked as removed.
    /// Keys set to non-zero values are removed from the removed set.
    pub fn update_with_state(&self, update: &HashedPostState) {
        for (hashed_address, storage) in &update.storages {
            let account = update
                .accounts
                .get(hashed_address)
                .map(|entry| entry.unwrap_or_default())
                .unwrap_or_default();

            if storage.wiped {
                self.storage_added_removed.remove(hashed_address);
                if account.is_empty() {
                    self.account_added_removed.insert_removed(*hashed_address);
                }
                continue;
            }

            let storage_entry = self
                .storage_added_removed
                .entry(*hashed_address)
                .or_insert_with(AddedRemovedKeysShared::new);

            for (key, val) in &storage.storage {
                if val.is_zero() {
                    storage_entry.insert_removed(*key);
                } else {
                    storage_entry.remove_removed(key);
                }
            }

            if !account.is_empty() {
                self.account_added_removed.remove_removed(hashed_address);
            }
        }
    }

    /// Returns proof targets that haven't been fetched yet, filtering out already-fetched ones.
    pub fn get_unfetched_targets(&self, state_update: &HashedPostState) -> MultiProofTargets {
        let mut targets = MultiProofTargets::default();

        for hashed_address in state_update.accounts.keys() {
            if !self.is_account_fetched(hashed_address) {
                targets.entry(*hashed_address).or_default();
            }
        }

        for (hashed_address, storage) in &state_update.storages {
            let fetched = self.fetched_targets.get(hashed_address);

            let changed_slots = storage.storage.keys().filter(|slot| {
                fetched.as_ref().is_none_or(|f| !f.contains(*slot))
            });

            // If storage is wiped, we need to fetch proofs for any previously removed keys
            if storage.wiped && fetched.is_none() {
                targets.entry(*hashed_address).or_default();
            }

            // Add slots that aren't fetched yet
            let unfetched: Vec<_> = changed_slots.copied().collect();
            if !unfetched.is_empty() {
                targets.entry(*hashed_address).or_default().extend(unfetched);
            }
        }

        targets
    }

    /// Returns a reference to storage added/removed keys for an account.
    pub fn get_storage_added_removed(
        &self,
        hashed_address: &B256,
    ) -> Option<dashmap::mapref::one::Ref<'_, B256, AddedRemovedKeysShared>> {
        self.storage_added_removed.get(hashed_address)
    }

    /// Returns a clone of account-level AddedRemovedKeys.
    pub fn clone_account_added_removed(&self) -> AddedRemovedKeys {
        self.account_added_removed.clone_inner()
    }

    /// Clears all state (for reuse between blocks).
    pub fn clear(&self) {
        self.fetched_targets.clear();
        self.storage_added_removed.clear();
        // Reset account added/removed to default
        *self.account_added_removed.inner.write() = default_added_removed_keys();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::U256;
    use reth_trie::HashedStorage;

    #[test]
    fn test_mark_and_check_fetched() {
        let state = SharedProofState::new();
        let addr = B256::random();
        let slot1 = B256::random();
        let slot2 = B256::random();

        assert!(!state.is_account_fetched(&addr));
        assert!(!state.is_storage_slot_fetched(&addr, &slot1));

        state.mark_account_fetched(addr);
        assert!(state.is_account_fetched(&addr));
        assert!(!state.is_storage_slot_fetched(&addr, &slot1));

        state.mark_storage_fetched(addr, [slot1].into_iter());
        assert!(state.is_storage_slot_fetched(&addr, &slot1));
        assert!(!state.is_storage_slot_fetched(&addr, &slot2));
    }

    #[test]
    fn test_update_with_state_storage_keys() {
        let state = SharedProofState::new();
        let addr = B256::random();
        let slot1 = B256::random();
        let slot2 = B256::random();

        let mut update = HashedPostState::default();
        let mut storage = HashedStorage::default();
        storage.storage.insert(slot1, U256::ZERO);
        storage.storage.insert(slot2, U256::from(100));
        update.storages.insert(addr, storage);

        state.update_with_state(&update);

        let storage_keys = state.get_storage_added_removed(&addr).unwrap();
        assert!(storage_keys.is_removed(&slot1));
        assert!(!storage_keys.is_removed(&slot2));
    }

    #[test]
    fn test_concurrent_access() {
        use std::thread;

        let state = SharedProofState::new();
        let state_clone = Arc::clone(&state);

        let handles: Vec<_> = (0..10)
            .map(|i| {
                let s = Arc::clone(&state_clone);
                thread::spawn(move || {
                    let addr = B256::from_slice(&[i as u8; 32]);
                    s.mark_account_fetched(addr);
                    s.touch_account(addr);

                    for j in 0..100 {
                        let slot = B256::from_slice(&[j as u8; 32]);
                        s.mark_storage_fetched(addr, [slot].into_iter());
                    }
                })
            })
            .collect();

        for h in handles {
            h.join().unwrap();
        }

        // Verify all addresses were added
        for i in 0..10 {
            let addr = B256::from_slice(&[i as u8; 32]);
            assert!(state.is_account_fetched(&addr));
        }
    }
}
