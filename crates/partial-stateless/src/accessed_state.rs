//! Representation of state accessed during a single block's execution.
//!
//! This is the input to `NetworkStateCache::on_block_executed()`.
//! It can be constructed from reth's `BundleState` after block execution.

use crate::policy::AccountData;
use alloy_primitives::{Address, Bytes, B256, U256};
use revm_database::BundleState;
use revm_primitives::KECCAK_EMPTY;
use std::collections::{HashMap, HashSet};

/// All state keys accessed during a single block's execution.
#[derive(Debug, Clone, Default)]
pub struct BlockAccessedState {
    /// Accounts accessed (address → account data).
    pub accounts: HashMap<Address, AccountData>,
    /// Storage slots accessed (address, slot) → value.
    pub storage: HashMap<(Address, B256), U256>,
    /// Bytecodes accessed: code_hash → bytecode bytes.
    pub codes: HashMap<B256, Bytes>,
}

impl BlockAccessedState {
    /// Construct from revm's `BundleState` output after block execution.
    ///
    /// The BundleState contains all accounts/storage that were touched during execution.
    /// We extract the accessed keys and their current values.
    pub fn from_bundle(bundle: &BundleState) -> Self {
        let mut accounts = HashMap::new();
        let mut storage = HashMap::new();
        let mut codes = HashMap::new();

        for (address, account) in &bundle.state {
            // Record account data
            if let Some(info) = &account.info {
                let code_hash =
                    if info.code_hash == KECCAK_EMPTY { None } else { Some(info.code_hash) };
                accounts.insert(
                    *address,
                    AccountData { nonce: info.nonce, balance: info.balance, code_hash },
                );
            } else {
                // Account was accessed but is empty/destroyed
                accounts.insert(
                    *address,
                    AccountData { nonce: 0, balance: U256::ZERO, code_hash: None },
                );
            }

            // Record storage slots
            for (slot, slot_value) in &account.storage {
                let slot_b256 = B256::from(*slot);
                // Use present_value
                storage.insert((*address, slot_b256), slot_value.present_value);
            }
        }

        // Record deployed/accessed contracts
        for (code_hash, bytecode) in &bundle.contracts {
            codes.insert(*code_hash, bytecode.original_bytes());
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
