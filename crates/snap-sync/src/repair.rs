//! Entries the downloaded state holds values for that no list will overwrite.
//!
//! Lists only overwrite the fields their blocks change, so a value the state holds for another
//! reason, such as a block a reorg orphaned, survives catch-up. Such entries are scheduled here and
//! fetched again on their own once catch-up reaches the pivot, where the pivot's values leave the
//! whole state at one block.

use crate::{common::SnapRecord, SnapSyncError};
use alloy_primitives::{B256, U256};
use reth_storage_api::{MetadataWriter, SnapAttemptId};
use serde::{Deserialize, Serialize};
use std::collections::{btree_map::Entry, BTreeMap, BTreeSet};

/// Accounts and storage slots to fetch again at the pivot, in key order.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StateRepairs {
    // Hashed addresses, each with the hashed slots of its storage to fetch again.
    accounts: BTreeMap<B256, BTreeSet<B256>>,
}

impl StateRepairs {
    /// Schedules the account at `hashed_address`.
    pub fn insert_account(&mut self, hashed_address: B256) {
        self.accounts.entry(hashed_address).or_default();
    }

    /// Schedules `hashed_slot` of the storage at `hashed_address`, along with its account.
    pub fn insert_slot(&mut self, hashed_address: B256, hashed_slot: B256) {
        self.accounts.entry(hashed_address).or_default().insert(hashed_slot);
    }

    /// Returns whether nothing is scheduled.
    pub fn is_empty(&self) -> bool {
        self.accounts.is_empty()
    }

    /// Number of accounts scheduled.
    pub fn len(&self) -> usize {
        self.accounts.len()
    }

    /// First account scheduled, in key order.
    pub fn first(&self) -> Option<B256> {
        self.accounts.keys().next().copied()
    }

    /// Slots scheduled for the storage at `hashed_address`, in key order.
    pub fn slots(&self, hashed_address: B256) -> impl Iterator<Item = B256> + '_ {
        self.accounts.get(&hashed_address).into_iter().flatten().copied()
    }

    // Adds what `other` schedules.
    pub(crate) fn extend(&mut self, other: Self) {
        for (hashed_address, slots) in other.accounts {
            self.accounts.entry(hashed_address).or_default().extend(slots);
        }
    }

    // Drops the account at `hashed_address` once `slots` resolve every slot scheduled for it, or
    // outright when it has no storage at the pivot. Slots scheduled after the fetch stay, and
    // keep their account scheduled.
    pub(crate) fn resolve(&mut self, hashed_address: B256, slots: Option<&[(B256, U256)]>) {
        let Entry::Occupied(mut entry) = self.accounts.entry(hashed_address) else { return };
        match slots {
            Some(slots) => {
                for (slot, _) in slots {
                    entry.get_mut().remove(slot);
                }
            }
            None => entry.get_mut().clear(),
        }
        if entry.get().is_empty() {
            entry.remove();
        }
    }
}

// The schedule as persisted, tied to the attempt that recorded it.
#[derive(Serialize, Deserialize)]
pub(crate) struct StoredRepairs {
    // Encoding version, checked before the rest is decoded.
    version: u32,
    // Attempt the repairs belong to.
    pub(crate) attempt: SnapAttemptId,
    // What that attempt still has to fetch again.
    pub(crate) repairs: StateRepairs,
}

impl SnapRecord for StoredRepairs {
    const KEY: &'static str = "snap_state_repairs";
    const VERSION: u32 = 1;
}

impl StoredRepairs {
    // Persists `repairs` for `attempt`, removing the record once nothing is left.
    pub(crate) fn store(
        provider: &impl MetadataWriter,
        attempt: SnapAttemptId,
        repairs: StateRepairs,
    ) -> Result<(), SnapSyncError> {
        if repairs.is_empty() {
            return Self::clear(provider)
        }
        Self { version: Self::VERSION, attempt, repairs }.write(provider)
    }
}
