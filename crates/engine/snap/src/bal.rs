//! BAL (Block Access List) diff application for snap sync.
//!
//! Converts raw `Vec<AccountChanges>` from a single block's BAL into
//! partial account diffs, storage writes, and bytecode entries that can
//! be merged with existing hashed state.

use alloy_consensus::constants::KECCAK_EMPTY;
use alloy_eip7928::AccountChanges;
use alloy_primitives::{keccak256, Bytes, B256, U256};

/// A partial diff for a single account extracted from one block's BAL.
///
/// Fields are `Some` only when the BAL contains at least one change for that
/// field. The caller must merge these with existing DB state before writing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BalAccountDiff {
    /// `keccak256(address)`.
    pub hashed_address: B256,
    /// Final balance if `balance_changes` was non-empty.
    pub balance: Option<U256>,
    /// Final nonce if `nonce_changes` was non-empty.
    pub nonce: Option<u64>,
    /// Final bytecode hash if `code_changes` was non-empty.
    /// Inner `None` means the code was cleared (empty code).
    pub bytecode_hash: Option<Option<B256>>,
}

/// Storage entries extracted from one block's BAL.
///
/// Each entry is `(hashed_address, hashed_slot, final_value)`.
pub type BalStorageEntry = (B256, B256, U256);

/// Bytecode entries extracted from one block's BAL.
///
/// Each entry is `(code_hash, code_bytes)`.
pub type BalBytecodeEntry = (B256, Bytes);

/// Parsed state diffs from a single block's BAL.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BalStateDiff {
    /// Per-account partial diffs (fields that changed in this block).
    pub accounts: Vec<BalAccountDiff>,
    /// `(hashed_address, hashed_slot, value)` triples for storage writes.
    pub storage: Vec<BalStorageEntry>,
    /// `(code_hash, code_bytes)` pairs for bytecode writes.
    pub bytecodes: Vec<BalBytecodeEntry>,
}

/// Convert a list of [`AccountChanges`] (from one block's BAL) into partial
/// state diffs.
///
/// For each account, the final post-block value is the entry with the highest
/// transaction index (i.e. the last element, since entries are ordered).
/// Fields with empty change lists are left as `None` in [`BalAccountDiff`],
/// meaning they were not modified in this block.
pub fn bal_to_state_diff(changes: &[AccountChanges]) -> BalStateDiff {
    let mut diff = BalStateDiff::default();

    for ac in changes {
        let hashed_address = keccak256(ac.address);

        // Last balance change → final balance.
        let balance = ac.balance_changes.last().map(|c| c.post_balance);

        // Last nonce change → final nonce.
        let nonce = ac.nonce_changes.last().map(|c| c.new_nonce);

        // Last code change → final code hash + bytecodes entry.
        let bytecode_hash = ac.code_changes.last().map(|c| {
            if c.new_code.is_empty() {
                None
            } else {
                let code_hash = keccak256(&c.new_code);
                diff.bytecodes.push((code_hash, c.new_code.clone()));
                Some(code_hash)
            }
        });

        // Storage: for each slot, take the last change's value.
        for slot_changes in &ac.storage_changes {
            if let Some(last_change) = slot_changes.changes.last() {
                let hashed_slot = keccak256(slot_changes.slot.to_be_bytes::<32>());
                diff.storage.push((hashed_address, hashed_slot, last_change.new_value));
            }
        }

        // Only emit an account diff if at least one field changed.
        if balance.is_some() || nonce.is_some() || bytecode_hash.is_some() {
            diff.accounts.push(BalAccountDiff { hashed_address, balance, nonce, bytecode_hash });
        }
    }

    diff
}

/// Merge a [`BalAccountDiff`] with an existing [`Account`], returning the
/// updated account.
///
/// Fields that are `None` in the diff retain their existing values. If
/// `existing` is `None` (new account), absent fields default to zero / no code.
pub fn merge_account_diff(
    diff: &BalAccountDiff,
    existing: Option<&reth_primitives_traits::Account>,
) -> reth_primitives_traits::Account {
    reth_primitives_traits::Account {
        balance: diff.balance.unwrap_or_else(|| existing.map(|a| a.balance).unwrap_or(U256::ZERO)),
        nonce: diff.nonce.unwrap_or_else(|| existing.map(|a| a.nonce).unwrap_or(0)),
        bytecode_hash: match diff.bytecode_hash {
            Some(hash) => {
                // Explicit code change: Some(hash) for non-empty, None for cleared.
                // Normalize: treat KECCAK_EMPTY as None (no code).
                match hash {
                    Some(h) if h == KECCAK_EMPTY => None,
                    other => other,
                }
            }
            None => {
                // No code change in this block — keep existing.
                existing.and_then(|a| a.bytecode_hash)
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_eip7928::{BalanceChange, CodeChange, NonceChange, SlotChanges, StorageChange};
    use alloy_primitives::Address;

    #[test]
    fn all_fields_present() {
        let addr = Address::from([0xaa; 20]);
        let code = Bytes::from(vec![0x60, 0x00, 0x56]);
        let code_hash = keccak256(&code);

        let changes = vec![AccountChanges {
            address: addr,
            balance_changes: vec![
                BalanceChange::new(0, U256::from(100)),
                BalanceChange::new(1, U256::from(200)),
            ],
            nonce_changes: vec![NonceChange::new(0, 1), NonceChange::new(1, 2)],
            code_changes: vec![CodeChange::new(0, code.clone())],
            storage_changes: vec![SlotChanges::new(
                U256::from(1),
                vec![StorageChange::new(0, U256::from(10)), StorageChange::new(1, U256::from(20))],
            )],
            storage_reads: vec![],
        }];

        let diff = bal_to_state_diff(&changes);

        assert_eq!(diff.accounts.len(), 1);
        let acct = &diff.accounts[0];
        assert_eq!(acct.hashed_address, keccak256(addr));
        assert_eq!(acct.balance, Some(U256::from(200)));
        assert_eq!(acct.nonce, Some(2));
        assert_eq!(acct.bytecode_hash, Some(Some(code_hash)));

        assert_eq!(diff.storage.len(), 1);
        let (ha, hs, val) = &diff.storage[0];
        assert_eq!(*ha, keccak256(addr));
        assert_eq!(*hs, keccak256(U256::from(1).to_be_bytes::<32>()));
        assert_eq!(*val, U256::from(20));

        assert_eq!(diff.bytecodes.len(), 1);
        assert_eq!(diff.bytecodes[0], (code_hash, code));
    }

    #[test]
    fn partial_fields_only_balance() {
        let addr = Address::from([0xbb; 20]);
        let changes = vec![AccountChanges {
            address: addr,
            balance_changes: vec![BalanceChange::new(0, U256::from(500))],
            nonce_changes: vec![],
            code_changes: vec![],
            storage_changes: vec![],
            storage_reads: vec![],
        }];

        let diff = bal_to_state_diff(&changes);

        assert_eq!(diff.accounts.len(), 1);
        let acct = &diff.accounts[0];
        assert_eq!(acct.balance, Some(U256::from(500)));
        assert_eq!(acct.nonce, None);
        assert_eq!(acct.bytecode_hash, None);
        assert!(diff.storage.is_empty());
        assert!(diff.bytecodes.is_empty());
    }

    #[test]
    fn last_entry_wins() {
        let addr = Address::from([0xcc; 20]);
        let changes = vec![AccountChanges {
            address: addr,
            balance_changes: vec![
                BalanceChange::new(0, U256::from(10)),
                BalanceChange::new(1, U256::from(20)),
                BalanceChange::new(5, U256::from(99)),
            ],
            nonce_changes: vec![NonceChange::new(0, 1), NonceChange::new(3, 7)],
            code_changes: vec![],
            storage_changes: vec![SlotChanges::new(
                U256::from(42),
                vec![
                    StorageChange::new(0, U256::from(100)),
                    StorageChange::new(2, U256::from(300)),
                    StorageChange::new(4, U256::from(999)),
                ],
            )],
            storage_reads: vec![],
        }];

        let diff = bal_to_state_diff(&changes);

        assert_eq!(diff.accounts[0].balance, Some(U256::from(99)));
        assert_eq!(diff.accounts[0].nonce, Some(7));
        assert_eq!(diff.storage[0].2, U256::from(999));
    }

    #[test]
    fn empty_changes_no_account_diff() {
        let addr = Address::from([0xdd; 20]);
        let changes = vec![AccountChanges {
            address: addr,
            balance_changes: vec![],
            nonce_changes: vec![],
            code_changes: vec![],
            storage_changes: vec![],
            storage_reads: vec![U256::from(1)],
        }];

        let diff = bal_to_state_diff(&changes);

        // No account, storage, or bytecode diffs — only reads.
        assert!(diff.accounts.is_empty());
        assert!(diff.storage.is_empty());
        assert!(diff.bytecodes.is_empty());
    }

    #[test]
    fn empty_code_clears_bytecode_hash() {
        let addr = Address::from([0xee; 20]);
        let changes = vec![AccountChanges {
            address: addr,
            balance_changes: vec![],
            nonce_changes: vec![],
            code_changes: vec![CodeChange::new(0, Bytes::new())],
            storage_changes: vec![],
            storage_reads: vec![],
        }];

        let diff = bal_to_state_diff(&changes);

        assert_eq!(diff.accounts.len(), 1);
        // Empty code → bytecode_hash = Some(None) (cleared).
        assert_eq!(diff.accounts[0].bytecode_hash, Some(None));
        assert!(diff.bytecodes.is_empty());
    }

    #[test]
    fn merge_with_existing_account() {
        let existing = reth_primitives_traits::Account {
            nonce: 5,
            balance: U256::from(1000),
            bytecode_hash: Some(B256::from([0xab; 32])),
        };

        let diff = BalAccountDiff {
            hashed_address: B256::ZERO,
            balance: Some(U256::from(2000)),
            nonce: None,
            bytecode_hash: None,
        };

        let merged = merge_account_diff(&diff, Some(&existing));
        assert_eq!(merged.balance, U256::from(2000));
        assert_eq!(merged.nonce, 5); // kept from existing
        assert_eq!(merged.bytecode_hash, Some(B256::from([0xab; 32]))); // kept from existing
    }

    #[test]
    fn merge_new_account_defaults() {
        let diff = BalAccountDiff {
            hashed_address: B256::ZERO,
            balance: Some(U256::from(100)),
            nonce: None,
            bytecode_hash: None,
        };

        let merged = merge_account_diff(&diff, None);
        assert_eq!(merged.balance, U256::from(100));
        assert_eq!(merged.nonce, 0);
        assert_eq!(merged.bytecode_hash, None);
    }

    #[test]
    fn multiple_accounts() {
        let changes = vec![
            AccountChanges {
                address: Address::from([0x01; 20]),
                balance_changes: vec![BalanceChange::new(0, U256::from(100))],
                nonce_changes: vec![NonceChange::new(0, 1)],
                code_changes: vec![],
                storage_changes: vec![],
                storage_reads: vec![],
            },
            AccountChanges {
                address: Address::from([0x02; 20]),
                balance_changes: vec![BalanceChange::new(0, U256::from(200))],
                nonce_changes: vec![],
                code_changes: vec![],
                storage_changes: vec![SlotChanges::new(
                    U256::from(5),
                    vec![StorageChange::new(0, U256::from(50))],
                )],
                storage_reads: vec![],
            },
        ];

        let diff = bal_to_state_diff(&changes);

        assert_eq!(diff.accounts.len(), 2);
        assert_eq!(diff.accounts[0].hashed_address, keccak256(Address::from([0x01; 20])));
        assert_eq!(diff.accounts[1].hashed_address, keccak256(Address::from([0x02; 20])));
        assert_eq!(diff.storage.len(), 1);
        assert_eq!(diff.storage[0].0, keccak256(Address::from([0x02; 20])));
    }
}
