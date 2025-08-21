//! Tracking of keys having been added and removed from the tries.

use alloy_primitives::{map::B256Map, B256};
use alloy_trie::proof::AddedRemovedKeys;

/// Tracks added and removed keys across account and storage tries.
#[derive(Debug, Clone, Default)]
pub struct MultiAddedRemovedKeys {
    account: AddedRemovedKeys,
    storages: B256Map<AddedRemovedKeys>,
}

impl MultiAddedRemovedKeys {
    /// Returns a new instance.
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns a [`AddedRemovedKeys`] for the storage trie of a particular account, if any.
    pub fn get_storage(&self, hashed_address: &B256) -> Option<&AddedRemovedKeys> {
        self.storages.get(hashed_address)
    }

    /// Returns an [`AddedRemovedKeys`] for tracking account-level changes.
    pub const fn get_accounts(&self) -> &AddedRemovedKeys {
        &self.account
    }

    /// Returns a mutable reference to the [`AddedRemovedKeys`] for the storage trie of a particular
    /// account.
    pub fn get_storage_or_default_mut(&mut self, hashed_address: B256) -> &mut AddedRemovedKeys {
        self.storages.entry(hashed_address).or_default()
    }

    /// Removes the [`AddedRemovedKeys`] for the given storage trie, if any.
    pub fn remove_storage(&mut self, hashed_address: &B256) {
        self.storages.remove(hashed_address);
    }

    /// Returns a mutable reference to the [`AddedRemovedKeys`] for tracking account-level changes.
    pub const fn get_accounts_mut(&mut self) -> &mut AddedRemovedKeys {
        &mut self.account
    }

    /// Clears all keys which have been added to either account or storages key tracking.
    pub fn clear_added(&mut self) {
        self.account.clear_added();
        for storage in self.storages.values_mut() {
            storage.clear_added();
        }
    }
}
