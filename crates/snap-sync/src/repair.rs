//! Entries the downloaded state holds values for that no list will overwrite.
//!
//! Lists only overwrite the fields their blocks change, so a value the state holds for another
//! reason, such as a block a reorg orphaned, survives catch-up. Such entries are scheduled here and
//! fetched again on their own once catch-up reaches the pivot, where the pivot's values leave the
//! whole state at one block.

use crate::{common::SnapRecord, SnapSyncError};
use alloy_eip7928::AccountChanges;
use alloy_primitives::{keccak256, B256, U256};
use reth_storage_api::{MetadataWriter, SnapAttemptId};
use serde::{Deserialize, Serialize};
use std::{
    collections::{btree_map::Entry, BTreeMap, BTreeSet},
    mem,
};

/// Accounts and storage slots to fetch again at the pivot, in key order.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StateRepairs {
    // Hashed addresses, each with the fields and hashed slots its state holds stale values for.
    accounts: BTreeMap<B256, StaleAccount>,
}

impl StateRepairs {
    /// Schedules every field of the account at `hashed_address`.
    pub fn insert_account(&mut self, hashed_address: B256) {
        let account = self.accounts.entry(hashed_address).or_default();
        (account.balance, account.nonce, account.code) = (true, true, true);
    }

    /// Schedules `hashed_slot` of the storage at `hashed_address`, along with its account.
    pub fn insert_slot(&mut self, hashed_address: B256, hashed_slot: B256) {
        self.accounts.entry(hashed_address).or_default().slots.insert(hashed_slot);
    }

    /// Schedules the fields and slots `changes` writes for the account at `hashed_address`.
    pub fn insert_changes(&mut self, hashed_address: B256, changes: &AccountChanges) {
        self.accounts.entry(hashed_address).or_default().insert_changes(changes);
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
        self.accounts
            .get(&hashed_address)
            .into_iter()
            .flat_map(|account| account.slots.iter())
            .copied()
    }

    // Adds what `other` schedules.
    pub(crate) fn extend(&mut self, other: Self) {
        for (hashed_address, stale) in other.accounts {
            self.accounts.entry(hashed_address).or_default().extend(stale);
        }
    }

    // Drops the fields and slots a canonical list overwrites, as `changes` records them. Returns
    // whether anything was dropped.
    pub(crate) fn resolve_changes(
        &mut self,
        hashed_address: B256,
        changes: &AccountChanges,
    ) -> bool {
        let Entry::Occupied(mut entry) = self.accounts.entry(hashed_address) else { return false };
        let resolved = entry.get_mut().resolve_changes(changes);
        if entry.get().is_resolved() {
            entry.remove();
        }
        resolved
    }

    // Drops the account at `hashed_address`, fetched whole, with the `slots` fetched along with it,
    // or every slot when it has no storage at the pivot. Slots scheduled after the fetch stay, and
    // keep their account scheduled.
    pub(crate) fn resolve(&mut self, hashed_address: B256, slots: Option<&[(B256, U256)]>) {
        let Entry::Occupied(mut entry) = self.accounts.entry(hashed_address) else { return };
        let account = entry.get_mut();
        (account.balance, account.nonce, account.code) = (false, false, false);
        match slots {
            Some(slots) => {
                for (slot, _) in slots {
                    account.slots.remove(slot);
                }
            }
            None => account.slots.clear(),
        }
        if account.is_resolved() {
            entry.remove();
        }
    }
}

// What the state holds stale values for in one account.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
struct StaleAccount {
    // Whether the balance is stale.
    balance: bool,
    // Whether the nonce is stale.
    nonce: bool,
    // Whether the code hash is stale.
    code: bool,
    // Hashed slots of its storage.
    slots: BTreeSet<B256>,
}

impl StaleAccount {
    // Whether nothing about the account is stale any more.
    fn is_resolved(&self) -> bool {
        !self.balance && !self.nonce && !self.code && self.slots.is_empty()
    }

    // Marks the fields and slots `changes` writes as stale.
    fn insert_changes(&mut self, changes: &AccountChanges) {
        let info = changes.account_info();
        self.balance |= info.balance.is_some();
        self.nonce |= info.nonce.is_some();
        self.code |= info.code_hash.is_some();
        self.slots.extend(Self::slots_of(changes));
    }

    // Marks what `other` holds stale as stale.
    fn extend(&mut self, other: Self) {
        self.balance |= other.balance;
        self.nonce |= other.nonce;
        self.code |= other.code;
        self.slots.extend(other.slots);
    }

    // Clears the fields and slots `changes` overwrites, returning whether any was stale.
    fn resolve_changes(&mut self, changes: &AccountChanges) -> bool {
        let info = changes.account_info();
        let mut resolved = false;
        resolved |= info.balance.is_some() && mem::take(&mut self.balance);
        resolved |= info.nonce.is_some() && mem::take(&mut self.nonce);
        resolved |= info.code_hash.is_some() && mem::take(&mut self.code);
        for slot in Self::slots_of(changes) {
            resolved |= self.slots.remove(&slot);
        }
        resolved
    }

    // Hashed keys of the slots `changes` writes.
    fn slots_of(changes: &AccountChanges) -> impl Iterator<Item = B256> + '_ {
        changes.storage_post_states().map(|(slot, _)| keccak256(B256::from(slot)))
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

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_eip7928::{BalanceChange, BlockAccessIndex, NonceChange, SlotChanges, StorageChange};
    use alloy_primitives::Address;

    const ACCOUNT: Address = Address::repeat_byte(0xaa);

    fn hashed() -> B256 {
        keccak256(ACCOUNT)
    }

    fn balance(value: u64) -> AccountChanges {
        AccountChanges::new(ACCOUNT)
            .with_balance_change(BalanceChange::new(BlockAccessIndex::new(1), U256::from(value)))
    }

    fn nonce(value: u64) -> AccountChanges {
        AccountChanges::new(ACCOUNT)
            .with_nonce_change(NonceChange::new(BlockAccessIndex::new(1), value))
    }

    fn slots(slots: &[u64]) -> AccountChanges {
        slots.iter().fold(AccountChanges::new(ACCOUNT), |changes, slot| {
            changes.with_storage_change(SlotChanges::new(
                U256::from(*slot),
                vec![StorageChange::new(BlockAccessIndex::new(1), U256::from(1))],
            ))
        })
    }

    fn scheduled(changes: &AccountChanges) -> StateRepairs {
        let mut repairs = StateRepairs::default();
        repairs.insert_changes(hashed(), changes);
        repairs
    }

    #[test]
    fn a_field_both_branches_change_needs_no_repair() {
        let mut repairs = scheduled(&balance(1));

        repairs.resolve_changes(hashed(), &balance(2));

        assert!(repairs.is_empty());
    }

    #[test]
    fn a_field_only_the_old_branch_changes_stays_scheduled() {
        let mut repairs = scheduled(&balance(1));

        repairs.resolve_changes(hashed(), &nonce(2));

        assert_eq!(repairs, scheduled(&balance(1)));
    }

    #[test]
    fn slots_are_resolved_one_by_one() {
        let mut repairs = scheduled(&slots(&[1, 2]));

        repairs.resolve_changes(hashed(), &slots(&[1]));

        let remaining: Vec<_> = repairs.slots(hashed()).collect();
        assert_eq!(remaining, [keccak256(B256::from(U256::from(2)))]);
    }

    #[test]
    fn a_fetched_account_resolves_every_field() {
        let mut repairs =
            scheduled(&balance(1).with_nonce_change(NonceChange::new(BlockAccessIndex::new(1), 1)));

        repairs.resolve(hashed(), None);

        assert!(repairs.is_empty());
    }
}
