//! BAL (Block Access List, EIP-7928) related functionality.

use alloy_consensus::constants::KECCAK_EMPTY;
use alloy_eip7928::BlockAccessList;
use alloy_primitives::{keccak256, U256};
use reth_primitives_traits::Account;
use reth_provider::{AccountReader, ProviderError};
use reth_trie::{HashedPostState, HashedStorage};
use revm_primitives::B256;

/// Converts a Block Access List into a [`HashedPostState`] by extracting the final state
/// of modified accounts and storage slots.
pub fn bal_to_hashed_post_state<P>(
    bal: &BlockAccessList,
    provider: &P,
) -> Result<HashedPostState, ProviderError>
where
    P: AccountReader,
{
    let mut hashed_state = HashedPostState::with_capacity(bal.len());

    for account_changes in bal {
        let address = account_changes.address;
        let hashed_address = keccak256(address);

        // Get the latest balance (last balance change if any)
        let balance = account_changes.balance_changes.last().map(|change| change.post_balance);

        // Get the latest nonce (last nonce change if any)
        let nonce = account_changes.nonce_changes.last().map(|change| change.new_nonce);

        // Get the latest code (last code change if any)
        let code_hash = if let Some(code_change) = account_changes.code_changes.last() {
            if code_change.new_code.is_empty() {
                Some(Some(KECCAK_EMPTY))
            } else {
                Some(Some(keccak256(&code_change.new_code)))
            }
        } else {
            None
        };

        // Only fetch account from provider if we're missing any field
        let existing_account = if balance.is_none() || nonce.is_none() || code_hash.is_none() {
            provider.basic_account(&address)?
        } else {
            None
        };

        // Build the final account state
        let account = Account {
            balance: balance.unwrap_or_else(|| {
                existing_account.as_ref().map(|acc| acc.balance).unwrap_or(U256::ZERO)
            }),
            nonce: nonce
                .unwrap_or_else(|| existing_account.as_ref().map(|acc| acc.nonce).unwrap_or(0)),
            bytecode_hash: code_hash.unwrap_or_else(|| {
                existing_account.as_ref().and_then(|acc| acc.bytecode_hash).or(Some(KECCAK_EMPTY))
            }),
        };

        hashed_state.accounts.insert(hashed_address, Some(account));

        // Process storage changes
        if !account_changes.storage_changes.is_empty() {
            let mut storage_map = HashedStorage::new(false);

            for slot_changes in &account_changes.storage_changes {
                let hashed_slot = keccak256(B256::from(slot_changes.slot));

                // Get the last change for this slot
                if let Some(last_change) = slot_changes.changes.last() {
                    storage_map.storage.insert(hashed_slot, last_change.new_value);
                }
            }

            if !storage_map.storage.is_empty() {
                hashed_state.storages.insert(hashed_address, storage_map);
            }
        }
    }

    Ok(hashed_state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_eip7928::{
        AccountChanges, BalanceChange, CodeChange, NonceChange, SlotChanges, StorageChange,
    };
    use alloy_primitives::{Address, Bytes, B256};
    use reth_revm::test_utils::StateProviderTest;

    #[test]
    fn test_bal_to_hashed_post_state_basic() {
        let provider = StateProviderTest::default();

        let address = Address::random();
        let account_changes = AccountChanges {
            address,
            storage_changes: vec![],
            storage_reads: vec![],
            balance_changes: vec![BalanceChange::new(0, U256::from(100))],
            nonce_changes: vec![NonceChange::new(0, 1)],
            code_changes: vec![],
        };

        let bal = vec![account_changes];
        let result = bal_to_hashed_post_state(&bal, &provider).unwrap();

        assert_eq!(result.accounts.len(), 1);

        let hashed_address = keccak256(address);
        let account_opt = result.accounts.get(&hashed_address).unwrap();
        assert!(account_opt.is_some());

        let account = account_opt.as_ref().unwrap();
        assert_eq!(account.balance, U256::from(100));
        assert_eq!(account.nonce, 1);
        assert_eq!(account.bytecode_hash, Some(KECCAK_EMPTY));
    }

    #[test]
    fn test_bal_with_storage_changes() {
        let provider = StateProviderTest::default();

        let address = Address::random();
        let slot = U256::random();
        let value = U256::random();

        let slot_changes = SlotChanges { slot, changes: vec![StorageChange::new(0, value)] };

        let account_changes = AccountChanges {
            address,
            storage_changes: vec![slot_changes],
            storage_reads: vec![],
            balance_changes: vec![BalanceChange::new(0, U256::from(500))],
            nonce_changes: vec![NonceChange::new(0, 2)],
            code_changes: vec![],
        };

        let bal = vec![account_changes];
        let result = bal_to_hashed_post_state(&bal, &provider).unwrap();

        let hashed_address = keccak256(address);
        assert!(result.storages.contains_key(&hashed_address));

        let storage = result.storages.get(&hashed_address).unwrap();
        let hashed_slot = keccak256(B256::from(slot));

        let stored_value = storage.storage.get(&hashed_slot).unwrap();
        assert_eq!(*stored_value, value);
    }

    #[test]
    fn test_bal_with_code_change() {
        let provider = StateProviderTest::default();

        let address = Address::random();
        let code = Bytes::from(vec![0x60, 0x80, 0x60, 0x40]); // Some bytecode

        let account_changes = AccountChanges {
            address,
            storage_changes: vec![],
            storage_reads: vec![],
            balance_changes: vec![BalanceChange::new(0, U256::from(1000))],
            nonce_changes: vec![NonceChange::new(0, 1)],
            code_changes: vec![CodeChange::new(0, code.clone())],
        };

        let bal = vec![account_changes];
        let result = bal_to_hashed_post_state(&bal, &provider).unwrap();

        let hashed_address = keccak256(address);
        let account_opt = result.accounts.get(&hashed_address).unwrap();
        let account = account_opt.as_ref().unwrap();

        let expected_code_hash = keccak256(&code);
        assert_eq!(account.bytecode_hash, Some(expected_code_hash));
    }

    #[test]
    fn test_bal_with_empty_code() {
        let provider = StateProviderTest::default();

        let address = Address::random();
        let empty_code = Bytes::default();

        let account_changes = AccountChanges {
            address,
            storage_changes: vec![],
            storage_reads: vec![],
            balance_changes: vec![BalanceChange::new(0, U256::from(1000))],
            nonce_changes: vec![NonceChange::new(0, 1)],
            code_changes: vec![CodeChange::new(0, empty_code)],
        };

        let bal = vec![account_changes];
        let result = bal_to_hashed_post_state(&bal, &provider).unwrap();

        let hashed_address = keccak256(address);
        let account_opt = result.accounts.get(&hashed_address).unwrap();
        let account = account_opt.as_ref().unwrap();

        assert_eq!(account.bytecode_hash, Some(KECCAK_EMPTY));
    }

    #[test]
    fn test_bal_multiple_changes_takes_last() {
        let provider = StateProviderTest::default();

        let address = Address::random();

        // Multiple balance changes - should take the last one
        let account_changes = AccountChanges {
            address,
            storage_changes: vec![],
            storage_reads: vec![],
            balance_changes: vec![
                BalanceChange::new(0, U256::from(100)),
                BalanceChange::new(1, U256::from(200)),
                BalanceChange::new(2, U256::from(300)),
            ],
            nonce_changes: vec![
                NonceChange::new(0, 1),
                NonceChange::new(1, 2),
                NonceChange::new(2, 3),
            ],
            code_changes: vec![],
        };

        let bal = vec![account_changes];
        let result = bal_to_hashed_post_state(&bal, &provider).unwrap();

        let hashed_address = keccak256(address);
        let account_opt = result.accounts.get(&hashed_address).unwrap();
        let account = account_opt.as_ref().unwrap();

        // Should have the last values
        assert_eq!(account.balance, U256::from(300));
        assert_eq!(account.nonce, 3);
    }

    #[test]
    fn test_bal_uses_provider_for_missing_fields() {
        let mut provider = StateProviderTest::default();

        let address = Address::random();
        let code_hash = B256::random();
        let existing_account =
            Account { balance: U256::from(999), nonce: 42, bytecode_hash: Some(code_hash) };
        provider.insert_account(address, existing_account, None, Default::default());

        // Only change balance, nonce and code should come from provider
        let account_changes = AccountChanges {
            address,
            storage_changes: vec![],
            storage_reads: vec![],
            balance_changes: vec![BalanceChange::new(0, U256::from(1500))],
            nonce_changes: vec![],
            code_changes: vec![],
        };

        let bal = vec![account_changes];
        let result = bal_to_hashed_post_state(&bal, &provider).unwrap();

        let hashed_address = keccak256(address);
        let account_opt = result.accounts.get(&hashed_address).unwrap();
        let account = account_opt.as_ref().unwrap();

        // Balance should be updated
        assert_eq!(account.balance, U256::from(1500));
        // Nonce and bytecode_hash should come from provider
        assert_eq!(account.nonce, 42);
        assert_eq!(account.bytecode_hash, Some(code_hash));
    }

    #[test]
    fn test_bal_multiple_storage_changes_per_slot() {
        let provider = StateProviderTest::default();

        let address = Address::random();
        let slot = U256::random();

        // Multiple changes to the same slot - should take the last one
        let slot_changes = SlotChanges {
            slot,
            changes: vec![
                StorageChange::new(0, U256::from(100)),
                StorageChange::new(1, U256::from(200)),
                StorageChange::new(2, U256::from(300)),
            ],
        };

        let account_changes = AccountChanges {
            address,
            storage_changes: vec![slot_changes],
            storage_reads: vec![],
            balance_changes: vec![BalanceChange::new(0, U256::from(100))],
            nonce_changes: vec![NonceChange::new(0, 1)],
            code_changes: vec![],
        };

        let bal = vec![account_changes];
        let result = bal_to_hashed_post_state(&bal, &provider).unwrap();

        let hashed_address = keccak256(address);
        let storage = result.storages.get(&hashed_address).unwrap();
        let hashed_slot = keccak256(B256::from(slot));

        let stored_value = storage.storage.get(&hashed_slot).unwrap();

        // Should have the last value
        assert_eq!(*stored_value, U256::from(300));
    }
}
