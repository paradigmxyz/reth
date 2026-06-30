//! Representation of state accessed during a single block's execution.
//!
//! This is the input to `NetworkStateCache::on_block_executed()`.
//! It can be constructed from revm's `State` database after block execution.

use crate::policy::AccountData;
use alloy_primitives::{Address, Bytes, B256, U256};
use revm_database::State;
use revm_primitives::KECCAK_EMPTY;
use std::collections::{HashMap, HashSet};

/// All state keys accessed during a single block's execution.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct BlockAccessedState {
    /// Accounts accessed (address → account data).
    pub accounts: HashMap<Address, AccountData>,
    /// Storage slots accessed (address, slot) → value.
    pub storage: HashMap<(Address, B256), U256>,
    /// Bytecodes accessed: code_hash → bytecode bytes.
    pub codes: HashMap<B256, Bytes>,
}

impl BlockAccessedState {
    /// Construct from revm's `State` database after block execution.
    ///
    /// This captures the complete read-set (including read-only accounts and contracts)
    /// from the database cache and bundle state.
    pub fn from_simulated_state<DB>(statedb: &State<DB>) -> Self {
        let mut accounts = HashMap::new();
        let mut storage = HashMap::new();
        let mut codes = HashMap::new();

        // 1. Traverse cached accounts (captures both read-only and modified state)
        for (address, cache_account) in &statedb.cache.accounts {
            if let Some(account) = &cache_account.account {
                // If account is present, record its info
                let code_hash = if account.info.code_hash == KECCAK_EMPTY {
                    None
                } else {
                    Some(account.info.code_hash)
                };
                accounts.insert(
                    *address,
                    AccountData {
                        nonce: account.info.nonce,
                        balance: account.info.balance,
                        code_hash,
                    },
                );

                // Record accessed storage slots
                for (slot, value) in &account.storage {
                    let slot_b256 = B256::from(*slot);
                    storage.insert((*address, slot_b256), *value);
                }
            } else {
                // Account is empty/destroyed/not found but was accessed
                accounts.insert(
                    *address,
                    AccountData {
                        nonce: 0,
                        balance: U256::ZERO,
                        code_hash: None,
                    },
                );
            }
        }

        // 2. Traverse bytecodes (contracts)
        // Check cache.contracts
        for (code_hash, code) in &statedb.cache.contracts {
            let bytes = code.original_bytes();
            if !bytes.is_empty() {
                codes.insert(*code_hash, bytes);
            }
        }

        // Check bundle_state.contracts (for newly created contracts in the block)
        for (code_hash, code) in &statedb.bundle_state.contracts {
            let bytes = code.original_bytes();
            if !bytes.is_empty() {
                codes.insert(*code_hash, bytes);
            }
        }

        Self { accounts, storage, codes }
    }



    /// Total number of unique state keys accessed.
    pub fn total_keys(&self) -> usize {
        self.accounts.len() + self.storage.len() + self.codes.len()
    }

    /// Set of all account addresses accessed.
    pub fn account_addresses(&self) -> HashSet<Address> {
        self.accounts.keys().cloned().collect()
    }

    /// Set of all storage keys accessed.
    pub fn storage_keys(&self) -> HashSet<(Address, B256)> {
        self.storage.keys().cloned().collect()
    }
}
