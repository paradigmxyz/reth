//! Shared post-block state extraction from EIP-7928 block access list entries.
//!
//! Missing fields are unchanged and must be merged with the pre-block account state.

use alloy_eip7928::AccountChanges;
use alloy_primitives::{keccak256, B256, KECCAK256_EMPTY, U256};
use reth_primitives_traits::Account;

/// The post-block account-level values one block access list entry commits to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BalAccountState {
    // Post-block balance, when the block changed it.
    balance: Option<U256>,
    // Post-block nonce, when the block changed it.
    nonce: Option<u64>,
    // Post-block code hash, when the block changed the code.
    // The inner `None` means the code was removed or set empty.
    code_hash: Option<Option<B256>>,
}

impl BalAccountState {
    /// Extracts changed account-level values using Alloy's post-state accessors.
    pub fn from_changes(changes: &AccountChanges) -> Self {
        Self {
            balance: changes.balance_post_state(),
            nonce: changes.nonce_post_state(),
            code_hash: changes
                .code_post_state()
                .map(|code| (!code.is_empty()).then(|| keccak256(code))),
        }
    }

    /// Returns the post-block balance, or `None` if unchanged.
    pub const fn balance(self) -> Option<U256> {
        self.balance
    }

    /// Returns the post-block nonce, or `None` if unchanged.
    pub const fn nonce(self) -> Option<u64> {
        self.nonce
    }

    /// Returns the post-block code hash, or `None` if unchanged.
    /// `Some(None)` means the code was removed or set empty.
    pub const fn code_hash(self) -> Option<Option<B256>> {
        self.code_hash
    }

    /// Returns `true` when the entry changed no account-level field.
    /// Read-only entries are empty and must not overwrite existing state.
    pub const fn is_empty(self) -> bool {
        self.balance.is_none() && self.nonce.is_none() && self.code_hash.is_none()
    }

    /// Returns `true` when merging needs the pre-block account.
    /// Fields the block did not touch retain their pre-block values.
    pub const fn needs_parent_account(self) -> bool {
        self.balance.is_none() || self.nonce.is_none() || self.code_hash.is_none()
    }

    /// Returns `true` when the entry contributes to the block's state root.
    pub const fn changes_state_root(self, changes: &AccountChanges) -> bool {
        !self.is_empty() || !changes.storage_changes.is_empty()
    }

    /// Applies the changed fields on top of `existing`, the account before the block.
    /// Missing fields keep their previous values; accounts without code use the empty-code hash.
    pub fn merge_onto(self, existing: Option<&Account>) -> Account {
        Account {
            balance: self
                .balance
                .or_else(|| existing.map(|account| account.balance))
                .unwrap_or_default(),
            nonce: self.nonce.or_else(|| existing.map(|account| account.nonce)).unwrap_or_default(),
            bytecode_hash: self
                .code_hash
                .map(|hash| hash.unwrap_or(KECCAK256_EMPTY))
                .or_else(|| existing.and_then(|account| account.bytecode_hash))
                .or(Some(KECCAK256_EMPTY)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::HashedStorage;
    use alloy_eip7928::{
        BalanceChange, BlockAccessIndex, CodeChange, NonceChange, SlotChanges, StorageChange,
    };
    use alloy_primitives::{bytes, Address};

    fn index(value: u64) -> BlockAccessIndex {
        BlockAccessIndex::new(value)
    }

    #[test]
    fn last_canonical_change_wins() {
        let mut changes = AccountChanges::new(Address::repeat_byte(0xaa));
        let code = bytes!("6002");
        changes.balance_changes.push(BalanceChange::new(index(1), U256::from(10)));
        changes.balance_changes.push(BalanceChange::new(index(3), U256::from(30)));
        changes.nonce_changes.push(NonceChange::new(index(1), 5));
        changes.nonce_changes.push(NonceChange::new(index(2), 7));
        changes.code_changes.push(CodeChange::new(index(1), bytes!("6001")));
        changes.code_changes.push(CodeChange::new(index(2), code.clone()));

        let state = BalAccountState::from_changes(&changes);

        assert!(state.changes_state_root(&changes));
        assert!(!state.needs_parent_account());
        assert_eq!(state.balance(), Some(U256::from(30)));
        assert_eq!(state.nonce(), Some(7));
        assert_eq!(state.code_hash(), Some(Some(keccak256(code))));
    }

    #[test]
    fn unsorted_changes_keep_last_recorded_values() {
        let slot = U256::from(1);
        let code = bytes!("6002");
        let mut changes = AccountChanges::new(Address::repeat_byte(0xaa));
        changes.balance_changes.push(BalanceChange::new(index(3), U256::from(30)));
        changes.balance_changes.push(BalanceChange::new(index(1), U256::from(10)));
        changes.nonce_changes.push(NonceChange::new(index(2), 7));
        changes.nonce_changes.push(NonceChange::new(index(1), 5));
        changes.code_changes.push(CodeChange::new(index(2), code));
        changes.code_changes.push(CodeChange::new(index(1), bytes!("6001")));
        changes.storage_changes.push(SlotChanges::new(
            slot,
            vec![
                StorageChange::new(index(4), U256::from(44)),
                StorageChange::new(index(1), U256::from(11)),
            ],
        ));

        let state = BalAccountState::from_changes(&changes);

        assert_eq!(state.balance(), Some(U256::from(10)));
        assert_eq!(state.nonce(), Some(5));
        assert_eq!(state.code_hash(), Some(Some(keccak256(bytes!("6001")))));
        assert_eq!(
            HashedStorage::from_account_changes(&changes),
            HashedStorage::from_iter([(keccak256(B256::from(slot)), U256::from(11))])
        );
    }

    #[test]
    fn storage_slots_are_hashed_and_take_the_final_value() {
        let slot = U256::from(1);
        let mut changes = AccountChanges::new(Address::repeat_byte(0xbb));
        changes.storage_changes.push(SlotChanges::new(
            slot,
            vec![
                StorageChange::new(index(1), U256::from(11)),
                StorageChange::new(index(4), U256::from(44)),
            ],
        ));

        let hashed = HashedStorage::from_account_changes(&changes);
        let state = BalAccountState::from_changes(&changes);

        assert!(state.changes_state_root(&changes));
        assert!(state.needs_parent_account());
        assert_eq!(
            hashed,
            HashedStorage::from_iter([(keccak256(B256::from(slot)), U256::from(44))])
        );
    }

    #[test]
    fn an_account_funded_then_emptied_merges_to_a_deletable_account() {
        let mut changes = AccountChanges::new(Address::repeat_byte(0xee));
        changes.balance_changes.push(BalanceChange::new(index(1), U256::from(100)));
        changes.balance_changes.push(BalanceChange::new(index(2), U256::ZERO));

        let state = BalAccountState::from_changes(&changes);

        assert!(state.changes_state_root(&changes));
        assert!(state.merge_onto(None).is_empty());
    }

    #[test]
    fn read_only_entries_are_empty() {
        let mut changes = AccountChanges::new(Address::repeat_byte(0xdd));
        changes.storage_reads.push(U256::from(1));

        let state = BalAccountState::from_changes(&changes);
        assert!(state.is_empty());
        assert!(!state.changes_state_root(&changes));
        assert!(HashedStorage::from_account_changes(&changes).is_empty());
    }

    #[test]
    fn untouched_fields_keep_their_stored_values() {
        let existing =
            Account { nonce: 4, balance: U256::from(9), bytecode_hash: Some(B256::repeat_byte(1)) };
        let changes = AccountChanges::new(Address::repeat_byte(0xaa))
            .with_balance_change(BalanceChange::new(index(1), U256::from(99)));
        let state = BalAccountState::from_changes(&changes);

        assert!(state.needs_parent_account());
        let merged = state.merge_onto(Some(&existing));

        assert_eq!(merged.balance, U256::from(99));
        assert_eq!(merged.nonce, 4);
        assert_eq!(merged.bytecode_hash, existing.bytecode_hash);
    }

    #[test]
    fn new_accounts_default_their_untouched_fields() {
        let state = BalAccountState { balance: Some(U256::from(1)), nonce: None, code_hash: None };

        let merged = state.merge_onto(None);

        assert_eq!(merged.nonce, 0);
        assert_eq!(merged.bytecode_hash, Some(KECCAK256_EMPTY));
    }

    #[test]
    fn cleared_and_empty_code_use_the_empty_code_hash() {
        let existing =
            Account { nonce: 1, balance: U256::ZERO, bytecode_hash: Some(B256::repeat_byte(2)) };
        let changes = AccountChanges::new(Address::repeat_byte(0xaa))
            .with_code_change(CodeChange::new(index(1), bytes!("")));
        let cleared = BalAccountState::from_changes(&changes);

        assert_eq!(cleared.merge_onto(Some(&existing)).bytecode_hash, Some(KECCAK256_EMPTY));

        let empty_hash =
            BalAccountState { balance: None, nonce: None, code_hash: Some(Some(KECCAK256_EMPTY)) };
        assert_eq!(empty_hash.merge_onto(None).bytecode_hash, Some(KECCAK256_EMPTY));
    }
}
