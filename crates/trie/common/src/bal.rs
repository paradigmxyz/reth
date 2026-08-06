//! Shared post-block state extraction from EIP-7928 block access list entries.
//!
//! Missing fields are unchanged and must be merged with the pre-block account state.

use alloy_eip7928::AccountChanges;
use alloy_primitives::{keccak256, B256, KECCAK256_EMPTY, U256};
use reth_primitives_traits::Account;

/// The post-block account-level values one block access list entry commits to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BalAccountState {
    /// Post-block balance, when the block changed it.
    pub balance: Option<U256>,
    /// Post-block nonce, when the block changed it.
    pub nonce: Option<u64>,
    /// Post-block code hash, when the block changed the code.
    ///
    /// The inner `None` means the code was removed or set empty.
    pub code_hash: Option<Option<B256>>,
}

impl BalAccountState {
    /// Extracts the post-block value of every changed account-level field: the change with the
    /// highest block access index.
    ///
    /// Canonical lists are sorted, but RLP decoding does not enforce it, so entries off the wire
    /// cannot be assumed ordered.
    pub fn from_changes(changes: &AccountChanges) -> Self {
        Self {
            balance: changes
                .balance_changes
                .iter()
                .max_by_key(|change| change.block_access_index)
                .map(|change| change.post_balance),
            nonce: changes
                .nonce_changes
                .iter()
                .max_by_key(|change| change.block_access_index)
                .map(|change| change.new_nonce),
            code_hash: changes
                .code_changes
                .iter()
                .max_by_key(|change| change.block_access_index)
                .map(|change| (!change.new_code.is_empty()).then(|| keccak256(&change.new_code))),
        }
    }

    /// Returns `true` when the entry changed no account-level field.
    /// Read-only entries are empty and must not overwrite existing state.
    pub const fn is_empty(&self) -> bool {
        self.balance.is_none() && self.nonce.is_none() && self.code_hash.is_none()
    }

    /// Returns `true` when merging needs the pre-block account.
    /// Fields the block did not touch retain their pre-block values.
    pub const fn needs_parent_account(&self) -> bool {
        self.balance.is_none() || self.nonce.is_none() || self.code_hash.is_none()
    }

    /// Returns `true` when the entry contributes to the block's state root.
    pub fn changes_state_root(&self, changes: &AccountChanges) -> bool {
        !self.is_empty() || !changes.storage_changes.is_empty()
    }

    /// Applies the changed fields on top of `existing`, the account before the block.
    /// Missing fields keep their previous values; empty code normalises to the database's `None`.
    pub fn merge_onto(&self, existing: Option<&Account>) -> Account {
        Account {
            balance: self
                .balance
                .or_else(|| existing.map(|account| account.balance))
                .unwrap_or_default(),
            nonce: self.nonce.or_else(|| existing.map(|account| account.nonce)).unwrap_or_default(),
            bytecode_hash: match self.code_hash {
                Some(Some(hash)) if hash != KECCAK256_EMPTY => Some(hash),
                Some(_) => None,
                None => existing.and_then(|account| account.bytecode_hash),
            },
        }
    }
}

/// Yields `(hashed slot, post-block value)` for every slot the entry changed.
/// Ordering is not assumed, as in [`BalAccountState::from_changes`].
pub fn hashed_storage_updates(changes: &AccountChanges) -> impl Iterator<Item = (B256, U256)> {
    changes.storage_changes.iter().filter_map(|slot| {
        slot.changes
            .iter()
            .max_by_key(|change| change.block_access_index)
            .map(|change| (keccak256(B256::from(slot.slot)), change.new_value))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_eip7928::{
        BalanceChange, BlockAccessIndex, CodeChange, NonceChange, SlotChanges, StorageChange,
    };
    use alloy_primitives::{bytes, Address};

    fn index(value: u64) -> BlockAccessIndex {
        BlockAccessIndex::new(value)
    }

    #[test]
    fn highest_block_access_index_wins() {
        let mut changes = AccountChanges::new(Address::repeat_byte(0xaa));
        let code = bytes!("6002");
        changes.balance_changes.push(BalanceChange::new(index(1), U256::from(10)));
        changes.balance_changes.push(BalanceChange::new(index(3), U256::from(30)));
        changes.nonce_changes.push(NonceChange::new(index(1), 5));
        changes.nonce_changes.push(NonceChange::new(index(2), 7));
        changes.code_changes.push(CodeChange::new(index(1), bytes!("6001")));
        changes.code_changes.push(CodeChange::new(index(2), code.clone()));

        let state = BalAccountState::from_changes(&changes);

        assert_eq!(state.balance, Some(U256::from(30)));
        assert_eq!(state.nonce, Some(7));
        assert_eq!(state.code_hash, Some(Some(keccak256(code))));
    }

    #[test]
    fn unsorted_changes_still_yield_the_post_block_value() {
        let slot = U256::from(1);
        let code = bytes!("6002");
        let mut changes = AccountChanges::new(Address::repeat_byte(0xaa));
        changes.balance_changes.push(BalanceChange::new(index(3), U256::from(30)));
        changes.balance_changes.push(BalanceChange::new(index(1), U256::from(10)));
        changes.nonce_changes.push(NonceChange::new(index(2), 7));
        changes.nonce_changes.push(NonceChange::new(index(1), 5));
        changes.code_changes.push(CodeChange::new(index(2), code.clone()));
        changes.code_changes.push(CodeChange::new(index(1), bytes!("6001")));
        changes.storage_changes.push(SlotChanges::new(
            slot,
            vec![
                StorageChange::new(index(4), U256::from(44)),
                StorageChange::new(index(1), U256::from(11)),
            ],
        ));

        let state = BalAccountState::from_changes(&changes);

        assert_eq!(state.balance, Some(U256::from(30)));
        assert_eq!(state.nonce, Some(7));
        assert_eq!(state.code_hash, Some(Some(keccak256(code))));
        assert_eq!(
            hashed_storage_updates(&changes).collect::<Vec<_>>(),
            vec![(keccak256(B256::from(slot)), U256::from(44))]
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

        let hashed = hashed_storage_updates(&changes).collect::<Vec<_>>();

        assert_eq!(hashed, vec![(keccak256(B256::from(slot)), U256::from(44))]);
    }

    #[test]
    fn read_only_entries_are_empty() {
        let mut changes = AccountChanges::new(Address::repeat_byte(0xdd));
        changes.storage_reads.push(U256::from(1));

        assert!(BalAccountState::from_changes(&changes).is_empty());
        assert_eq!(hashed_storage_updates(&changes).next(), None);
    }

    #[test]
    fn untouched_fields_keep_their_stored_values() {
        let existing =
            Account { nonce: 4, balance: U256::from(9), bytecode_hash: Some(B256::repeat_byte(1)) };
        let state = BalAccountState { balance: Some(U256::from(99)), nonce: None, code_hash: None };

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
        assert_eq!(merged.bytecode_hash, None);
    }

    #[test]
    fn cleared_and_empty_code_normalise_to_no_code() {
        let existing =
            Account { nonce: 1, balance: U256::ZERO, bytecode_hash: Some(B256::repeat_byte(2)) };
        let cleared = BalAccountState { balance: None, nonce: None, code_hash: Some(None) };

        assert_eq!(cleared.merge_onto(Some(&existing)).bytecode_hash, None);

        let empty_hash =
            BalAccountState { balance: None, nonce: None, code_hash: Some(Some(KECCAK256_EMPTY)) };
        assert_eq!(empty_hash.merge_onto(None).bytecode_hash, None);
    }
}
