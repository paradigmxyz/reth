//! BAL (Block Access List, EIP-7928) related functionality.

use crate::tree::cached_state::CachedStateProvider;
use alloy_consensus::constants::KECCAK_EMPTY;
use alloy_eip7928::BlockAccessList;
use alloy_primitives::{keccak256, Address, StorageKey, U256};
use reth_primitives_traits::Account;
use reth_provider::{AccountReader, ProviderError};
use reth_trie::{HashedPostState, HashedStorage};
use std::ops::Range;

/// Returns the total number of changed storage slots across all accounts in the BAL.
pub fn total_changed_slots(bal: &BlockAccessList) -> usize {
    bal.iter().map(|account| account.storage_changes.len()).sum()
}

/// Iterator over changed slots in a [`BlockAccessList`], with range-based filtering.
///
/// Iterates over all `(Address, StorageKey)` pairs representing changed storage slots
/// across all accounts in the BAL. The iterator intelligently skips accounts and slots
/// outside the specified range for efficient traversal.
#[derive(Debug)]
pub(crate) struct ChangedSlotIter<'a> {
    bal: &'a BlockAccessList,
    range: Range<usize>,
    current_index: usize,
    account_idx: usize,
    slot_idx: usize,
}

impl<'a> ChangedSlotIter<'a> {
    /// Creates a new iterator over changed slots within the specified range.
    pub(crate) fn new(bal: &'a BlockAccessList, range: Range<usize>) -> Self {
        let mut iter = Self { bal, range, current_index: 0, account_idx: 0, slot_idx: 0 };
        iter.skip_to_range_start();
        iter
    }

    /// Skips to the first item within the range.
    fn skip_to_range_start(&mut self) {
        while self.account_idx < self.bal.len() {
            let account = &self.bal[self.account_idx];
            let slots_in_account = account.storage_changes.len();

            // Check if this account contains items in our range
            let account_end = self.current_index + slots_in_account;

            if account_end <= self.range.start {
                // Entire account is before range, skip it
                self.current_index = account_end;
                self.account_idx += 1;
                self.slot_idx = 0;
            } else if self.current_index < self.range.start {
                // Range starts somewhere in this account
                let skip_slots = self.range.start - self.current_index;
                self.slot_idx = skip_slots;
                self.current_index = self.range.start;
                break;
            } else {
                // We're at or past range start
                break;
            }
        }
    }
}

impl<'a> Iterator for ChangedSlotIter<'a> {
    type Item = (Address, StorageKey);

    fn next(&mut self) -> Option<Self::Item> {
        // Check if we've exceeded the range
        if self.current_index >= self.range.end {
            return None;
        }

        // Find the next valid slot
        while self.account_idx < self.bal.len() {
            let account = &self.bal[self.account_idx];

            if self.slot_idx < account.storage_changes.len() {
                // Return current slot and advance
                let slot = account.storage_changes[self.slot_idx].slot;
                let address = account.address;

                self.slot_idx += 1;
                self.current_index += 1;

                // Check if we've reached the end of range
                if self.current_index > self.range.end {
                    return None;
                }

                return Some((address, slot));
            }

            // Move to next account
            self.account_idx += 1;
            self.slot_idx = 0;
        }

        None
    }
}

/// Converts a Block Access List into a [`HashedPostState`] by extracting the final state
/// of modified accounts and storage slots.
pub(crate) fn bal_to_hashed_post_state<P>(
    bal: &BlockAccessList,
    provider: &CachedStateProvider<P>,
) -> Result<HashedPostState, ProviderError>
where
    P: AccountReader,
{
    let mut hashed_state = HashedPostState::with_capacity(bal.len());

    for account_changes in bal {
        let address = account_changes.address;

        // Always fetch the account; even if we don't need the db account to construct the final
        // `Account`, doing this fills the cache.
        let existing_account = provider.basic_account(&address)?;

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

        // If the account was only read then don't add it to the HashedPostState
        if balance.is_none() &&
            nonce.is_none() &&
            code_hash.is_none() &&
            account_changes.storage_changes.is_empty()
        {
            continue
        }

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

        let hashed_address = keccak256(address);
        hashed_state.accounts.insert(hashed_address, Some(account));

        // Process storage changes
        if !account_changes.storage_changes.is_empty() {
            let mut storage_map = HashedStorage::new(false);

            for slot_changes in &account_changes.storage_changes {
                let hashed_slot = keccak256(slot_changes.slot);

                // Get the last change for this slot
                if let Some(last_change) = slot_changes.changes.last() {
                    storage_map
                        .storage
                        .insert(hashed_slot, U256::from_be_bytes(last_change.new_value.0));
                }
            }

            hashed_state.storages.insert(hashed_address, storage_map);
        }
    }

    Ok(hashed_state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tree::cached_state::{ExecutionCache, ExecutionCacheBuilder};
    use alloy_eip7928::{
        AccountChanges, BalanceChange, CodeChange, NonceChange, SlotChanges, StorageChange,
    };
    use alloy_primitives::{Address, Bytes, StorageKey, B256};
    use reth_revm::test_utils::StateProviderTest;

    fn new_cache() -> ExecutionCache {
        ExecutionCacheBuilder::default().build_caches(1000)
    }

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
        let provider = CachedStateProvider::new(provider, new_cache(), Default::default());
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
        let slot = StorageKey::random();
        let value = B256::random();

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
        let provider = CachedStateProvider::new(provider, new_cache(), Default::default());
        let result = bal_to_hashed_post_state(&bal, &provider).unwrap();

        let hashed_address = keccak256(address);
        assert!(result.storages.contains_key(&hashed_address));

        let storage = result.storages.get(&hashed_address).unwrap();
        let hashed_slot = keccak256(slot);

        let stored_value = storage.storage.get(&hashed_slot).unwrap();
        assert_eq!(*stored_value, U256::from_be_bytes(value.0));
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
        let provider = CachedStateProvider::new(provider, new_cache(), Default::default());
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
        let provider = CachedStateProvider::new(provider, new_cache(), Default::default());
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
        let provider = CachedStateProvider::new(provider, new_cache(), Default::default());
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
        let provider = CachedStateProvider::new(provider, new_cache(), Default::default());
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
        let slot = StorageKey::random();

        // Multiple changes to the same slot - should take the last one
        let slot_changes = SlotChanges {
            slot,
            changes: vec![
                StorageChange::new(0, B256::from(U256::from(100).to_be_bytes::<32>())),
                StorageChange::new(1, B256::from(U256::from(200).to_be_bytes::<32>())),
                StorageChange::new(2, B256::from(U256::from(300).to_be_bytes::<32>())),
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
        let provider = CachedStateProvider::new(provider, new_cache(), Default::default());
        let result = bal_to_hashed_post_state(&bal, &provider).unwrap();

        let hashed_address = keccak256(address);
        let storage = result.storages.get(&hashed_address).unwrap();
        let hashed_slot = keccak256(slot);

        let stored_value = storage.storage.get(&hashed_slot).unwrap();

        // Should have the last value
        assert_eq!(*stored_value, U256::from(300));
    }

    #[test]
    fn test_changed_slot_iter() {
        // Create test data with multiple accounts and slots
        let addr1 = Address::repeat_byte(0x01);
        let addr2 = Address::repeat_byte(0x02);
        let addr3 = Address::repeat_byte(0x03);

        // Account 1: 3 slots (indices 0, 1, 2)
        let account1 = AccountChanges {
            address: addr1,
            storage_changes: vec![
                SlotChanges {
                    slot: StorageKey::from(U256::from(100)),
                    changes: vec![StorageChange::new(0, B256::ZERO)],
                },
                SlotChanges {
                    slot: StorageKey::from(U256::from(101)),
                    changes: vec![StorageChange::new(0, B256::ZERO)],
                },
                SlotChanges {
                    slot: StorageKey::from(U256::from(102)),
                    changes: vec![StorageChange::new(0, B256::ZERO)],
                },
            ],
            storage_reads: vec![],
            balance_changes: vec![],
            nonce_changes: vec![],
            code_changes: vec![],
        };

        // Account 2: 2 slots (indices 3, 4)
        let account2 = AccountChanges {
            address: addr2,
            storage_changes: vec![
                SlotChanges {
                    slot: StorageKey::from(U256::from(200)),
                    changes: vec![StorageChange::new(0, B256::ZERO)],
                },
                SlotChanges {
                    slot: StorageKey::from(U256::from(201)),
                    changes: vec![StorageChange::new(0, B256::ZERO)],
                },
            ],
            storage_reads: vec![],
            balance_changes: vec![],
            nonce_changes: vec![],
            code_changes: vec![],
        };

        // Account 3: 3 slots (indices 5, 6, 7)
        let account3 = AccountChanges {
            address: addr3,
            storage_changes: vec![
                SlotChanges {
                    slot: StorageKey::from(U256::from(300)),
                    changes: vec![StorageChange::new(0, B256::ZERO)],
                },
                SlotChanges {
                    slot: StorageKey::from(U256::from(301)),
                    changes: vec![StorageChange::new(0, B256::ZERO)],
                },
                SlotChanges {
                    slot: StorageKey::from(U256::from(302)),
                    changes: vec![StorageChange::new(0, B256::ZERO)],
                },
            ],
            storage_reads: vec![],
            balance_changes: vec![],
            nonce_changes: vec![],
            code_changes: vec![],
        };

        let bal = vec![account1, account2, account3];

        // Test 1: Iterate over all slots (range 0..8)
        let items: Vec<_> = ChangedSlotIter::new(&bal, 0..8).collect();
        assert_eq!(items.len(), 8);
        assert_eq!(items[0], (addr1, StorageKey::from(U256::from(100))));
        assert_eq!(items[2], (addr1, StorageKey::from(U256::from(102))));
        assert_eq!(items[3], (addr2, StorageKey::from(U256::from(200))));
        assert_eq!(items[7], (addr3, StorageKey::from(U256::from(302))));

        // Test 2: Range that skips first account (range 3..6)
        let items: Vec<_> = ChangedSlotIter::new(&bal, 3..6).collect();
        assert_eq!(items.len(), 3);
        assert_eq!(items[0], (addr2, StorageKey::from(U256::from(200))));
        assert_eq!(items[1], (addr2, StorageKey::from(U256::from(201))));
        assert_eq!(items[2], (addr3, StorageKey::from(U256::from(300))));

        // Test 3: Range within first account (range 1..2)
        let items: Vec<_> = ChangedSlotIter::new(&bal, 1..2).collect();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0], (addr1, StorageKey::from(U256::from(101))));

        // Test 4: Range spanning multiple accounts (range 2..5)
        let items: Vec<_> = ChangedSlotIter::new(&bal, 2..5).collect();
        assert_eq!(items.len(), 3);
        assert_eq!(items[0], (addr1, StorageKey::from(U256::from(102))));
        assert_eq!(items[1], (addr2, StorageKey::from(U256::from(200))));
        assert_eq!(items[2], (addr2, StorageKey::from(U256::from(201))));

        // Test 5: Empty range
        let items: Vec<_> = ChangedSlotIter::new(&bal, 5..5).collect();
        assert_eq!(items.len(), 0);

        // Test 6: Range beyond end
        let items: Vec<_> = ChangedSlotIter::new(&bal, 6..100).collect();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0], (addr3, StorageKey::from(U256::from(301))));
        assert_eq!(items[1], (addr3, StorageKey::from(U256::from(302))));
    }
}
