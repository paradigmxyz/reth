//! Interprets EIP-7928 entries as authenticated post-block state updates.
//!
//! Untouched fields remain absent for merging with parent state, while access indices select the
//! final value independently of container order.

use alloy_consensus::constants::KECCAK_EMPTY;
use alloy_eip7928::AccountChanges;
use alloy_primitives::{keccak256, Bytes, B256, U256};
use reth_primitives_traits::Account;

/// Account fields changed by one block access list entry.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct BalAccountState {
    /// Post-block balance when changed.
    pub balance: Option<U256>,
    /// Post-block nonce when changed.
    pub nonce: Option<u64>,
    /// Post-block code hash when changed.
    pub code_hash: Option<B256>,
}

impl BalAccountState {
    /// Selects each field's change with the highest block access index.
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
                .map(|change| {
                    if change.new_code.is_empty() {
                        KECCAK_EMPTY
                    } else {
                        keccak256(&change.new_code)
                    }
                }),
        }
    }

    /// Returns whether the entry changes no account field.
    pub const fn is_empty(&self) -> bool {
        self.balance.is_none() && self.nonce.is_none() && self.code_hash.is_none()
    }

    /// Returns whether an unchanged field must be read from parent state.
    pub const fn needs_parent_account(&self) -> bool {
        self.balance.is_none() || self.nonce.is_none() || self.code_hash.is_none()
    }

    /// Merges changed fields onto the account before the block.
    pub fn merge_onto(&self, existing: Option<&Account>) -> Account {
        Account {
            balance: self
                .balance
                .or_else(|| existing.map(|account| account.balance))
                .unwrap_or_default(),
            nonce: self.nonce.or_else(|| existing.map(|account| account.nonce)).unwrap_or_default(),
            bytecode_hash: self
                .code_hash
                .or_else(|| existing.and_then(|account| account.bytecode_hash))
                .or(Some(KECCAK_EMPTY)),
        }
    }
}

/// Returns each changed slot's hashed key and final value.
pub fn hashed_storage_changes(changes: &AccountChanges) -> impl Iterator<Item = (B256, U256)> + '_ {
    changes.storage_changes.iter().filter_map(|slot| {
        slot.changes
            .iter()
            .max_by_key(|change| change.block_access_index)
            .map(|change| (keccak256(slot.slot.to_be_bytes::<32>()), change.new_value))
    })
}

/// Returns the final non-empty code deployed by the entry and its hash.
pub fn deployed_bytecode(changes: &AccountChanges) -> Option<(B256, &Bytes)> {
    changes
        .code_changes
        .iter()
        .max_by_key(|change| change.block_access_index)
        .filter(|change| !change.new_code.is_empty())
        .map(|change| (keccak256(&change.new_code), &change.new_code))
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_eip7928::{
        BalanceChange, BlockAccessIndex, CodeChange, NonceChange, SlotChanges, StorageChange,
    };
    use alloy_primitives::Address;

    const fn index(value: u64) -> BlockAccessIndex {
        BlockAccessIndex::new(value)
    }

    #[test]
    fn highest_index_selects_post_block_fields() {
        let mut changes = AccountChanges::new(Address::repeat_byte(0xaa));
        changes.balance_changes.push(BalanceChange::new(index(3), U256::from(30)));
        changes.balance_changes.push(BalanceChange::new(index(1), U256::from(10)));
        changes.nonce_changes.push(NonceChange::new(index(2), 7));
        changes.nonce_changes.push(NonceChange::new(index(1), 5));

        let state = BalAccountState::from_changes(&changes);

        assert_eq!(state.balance, Some(U256::from(30)));
        assert_eq!(state.nonce, Some(7));
    }

    #[test]
    fn changed_slots_are_hashed() {
        let slot = U256::from(1);
        let mut changes = AccountChanges::new(Address::repeat_byte(0xbb));
        changes.storage_changes.push(SlotChanges::new(
            slot,
            vec![
                StorageChange::new(index(4), U256::from(44)),
                StorageChange::new(index(1), U256::from(11)),
            ],
        ));

        let hashed = hashed_storage_changes(&changes).collect::<Vec<_>>();

        assert_eq!(hashed, vec![(keccak256(slot.to_be_bytes::<32>()), U256::from(44))]);
    }

    #[test]
    fn deployed_code_matches_account_hash() {
        let code = Bytes::from_static(&[0x60, 0x00, 0x56]);
        let mut changes = AccountChanges::new(Address::repeat_byte(0xcc));
        changes.code_changes.push(CodeChange::new(index(1), code.clone()));

        let state = BalAccountState::from_changes(&changes);
        let deployed = deployed_bytecode(&changes).unwrap();

        assert_eq!(state.code_hash, Some(deployed.0));
        assert_eq!(deployed, (keccak256(&code), &code));
    }

    #[test]
    fn untouched_fields_use_parent_account() {
        let existing =
            Account { nonce: 4, balance: U256::from(9), bytecode_hash: Some(B256::repeat_byte(1)) };
        let state = BalAccountState { balance: Some(U256::from(99)), ..Default::default() };

        let merged = state.merge_onto(Some(&existing));

        assert_eq!(merged.balance, U256::from(99));
        assert_eq!(merged.nonce, 4);
        assert_eq!(merged.bytecode_hash, existing.bytecode_hash);
    }

    #[test]
    fn empty_code_uses_canonical_hash() {
        let mut changes = AccountChanges::new(Address::repeat_byte(0xdd));
        changes.code_changes.push(CodeChange::new(index(1), Bytes::new()));

        let state = BalAccountState::from_changes(&changes);

        assert_eq!(state.code_hash, Some(KECCAK_EMPTY));
        assert!(deployed_bytecode(&changes).is_none());
    }
}
