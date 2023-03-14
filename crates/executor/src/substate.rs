//! Substate for blockchain trees

use reth_interfaces::{provider::ProviderError, Result};
use reth_primitives::{Account, Address, BlockHash, BlockNumber, Bytecode, Bytes, H256, U256};
use reth_provider::{AccountProvider, BlockHashProvider, StateProvider};
use std::collections::{hash_map::Entry, BTreeMap, HashMap};

use crate::execution_result::{AccountInfoChangeSet, ExecutionResult};

/// Memory backend, storing all state values in a `Map` in memory.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SubStateData {
    /// Account info where None means it is not existing. Not existing state is needed for Pre
    /// TANGERINE forks. `code` is always `None`, and bytecode can be found in `contracts`.
    pub accounts: HashMap<Address, AccountSubState>,
    /// New bytecodes
    pub bytecodes: HashMap<H256, (u32, Bytecode)>,
}

impl SubStateData {
    /// Apply changesets to substate.
    pub fn apply(&mut self, changesets: &[ExecutionResult]) {
        for changeset in changesets {
            self.apply_one(changeset)
        }
    }

    /// Apply one changeset to substate.
    pub fn apply_one(&mut self, changeset: &ExecutionResult) {
        for tx_changeset in changeset.tx_changesets.iter() {
            // apply accounts
            for (address, account_change) in tx_changeset.changeset.iter() {
                // revert account
                self.apply_account(address, &account_change.account);
                // revert its storage
                self.apply_storage(address, &account_change.storage);
            }
            // apply bytecodes
            for (hash, bytecode) in tx_changeset.new_bytecodes.iter() {
                self.bytecodes.entry(*hash).or_insert((0, Bytecode(bytecode.clone()))).0 += 1;
            }
        }
        // apply block reward
        for (address, change) in changeset.block_changesets.iter() {
            self.apply_account(address, change)
        }
    }

    /// Apply account changeset to substate
    fn apply_account(&mut self, address: &Address, change: &AccountInfoChangeSet) {
        match change {
            AccountInfoChangeSet::Created { new } => match self.accounts.entry(*address) {
                Entry::Vacant(entry) => {
                    entry.insert(AccountSubState::created_account(*new));
                }
                Entry::Occupied(mut entry) => {
                    let account = entry.get_mut();
                    // increment counter
                    account.inc_storage_counter();
                    account.info = *new;
                }
            },
            AccountInfoChangeSet::Destroyed { .. } => {
                // set selfdestructed account
                let account = self.accounts.entry(*address).or_default();
                account.inc_storage_counter();
                account.info = Default::default();
                account.storage.clear();
            }
            AccountInfoChangeSet::Changed { old, .. } => {
                self.accounts.entry(*address).or_default().info = *old;
            }
            AccountInfoChangeSet::NoChange { is_empty } => {
                if *is_empty {
                    self.accounts.entry(*address).or_default();
                }
            }
        }
    }

    /// Apply storage changeset to substate
    fn apply_storage(&mut self, address: &Address, storage: &BTreeMap<U256, (U256, U256)>) {
        if let Entry::Occupied(mut entry) = self.accounts.entry(*address) {
            let account_storage = &mut entry.get_mut().storage;
            for (key, (_, new_value)) in storage {
                let key = H256(key.to_be_bytes());
                account_storage.insert(key, *new_value);
            }
        }
    }

    /// Revert to old state in substate. Changesets will be reverted in reverse order,
    pub fn revert(&mut self, changesets: &[ExecutionResult]) {
        for changeset in changesets.iter().rev() {
            // revert block changeset
            for (address, change) in changeset.block_changesets.iter() {
                self.revert_account(address, change)
            }

            for tx_changeset in changeset.tx_changesets.iter() {
                // revert bytecodes
                for (hash, _) in tx_changeset.new_bytecodes.iter() {
                    match self.bytecodes.entry(*hash) {
                        Entry::Vacant(_) => panic!("Bytecode should be present"),
                        Entry::Occupied(mut entry) => {
                            let (cnt, _) = entry.get_mut();
                            *cnt -= 1;
                            if *cnt == 0 {
                                entry.remove_entry();
                            }
                        }
                    }
                }
                // revert accounts
                for (address, account_change) in tx_changeset.changeset.iter() {
                    // revert account
                    self.revert_account(address, &account_change.account);
                    // revert its storage
                    self.revert_storage(address, &account_change.storage);
                }
            }
        }
    }

    /// Revert storage
    fn revert_storage(&mut self, address: &Address, storage: &BTreeMap<U256, (U256, U256)>) {
        if let Entry::Occupied(mut entry) = self.accounts.entry(*address) {
            let account_storage = &mut entry.get_mut().storage;
            for (key, (old_value, _)) in storage {
                let key = H256(key.to_be_bytes());
                account_storage.insert(key, *old_value);
            }
        }
    }

    /// Revert account
    fn revert_account(&mut self, address: &Address, change: &AccountInfoChangeSet) {
        match change {
            AccountInfoChangeSet::Created { .. } => {
                match self.accounts.entry(*address) {
                    Entry::Vacant(_) => {
                        // We inserted this account in apply fn.
                        panic!("It should be present, something is broken");
                    }
                    Entry::Occupied(mut entry) => {
                        let val = entry.get_mut();
                        if val.decr_storage_counter() {
                            // remove account that we didn't change from substate

                            entry.remove_entry();
                            return
                        }
                        val.info = Account::default();
                        val.storage.clear();
                    }
                };
            }
            AccountInfoChangeSet::Destroyed { old } => match self.accounts.entry(*address) {
                Entry::Vacant(_) => {
                    // We inserted this account in apply fn.
                    panic!("It should be present, something is broken");
                }
                Entry::Occupied(mut entry) => {
                    let val = entry.get_mut();

                    // Contrary to Created we are not removing this account as we dont know if
                    // this account was changer or not by `Changed` changeset.
                    val.decr_storage_counter();
                    val.info = *old;
                }
            },
            AccountInfoChangeSet::Changed { old, .. } => {
                self.accounts.entry(*address).or_default().info = *old;
            }
            AccountInfoChangeSet::NoChange { is_empty: _ } => {
                // do nothing
            }
        }
    }
}
/// Account changes in substate
#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub struct AccountSubState {
    /// New account state
    pub info: Account,
    /// If account is selfdestructed or newly created, storage will be cleared.
    /// and we dont need to ask the provider for data.
    /// As we need to have as
    pub storage_is_clear: Option<u32>,
    /// storage slots
    pub storage: HashMap<H256, U256>,
}

impl AccountSubState {
    /// Increment storage counter to mark this storage was cleared
    pub fn inc_storage_counter(&mut self) {
        self.storage_is_clear = Some(self.storage_is_clear.unwrap_or_default() + 1);
    }

    /// Decrement storage counter to represent that changeset that cleared storage was reverted.
    pub fn decr_storage_counter(&mut self) -> bool {
        let Some(cnt) = self.storage_is_clear else { return false};

        if cnt == 1 {
            self.storage_is_clear = None;
            return true
        }
        false
    }
    /// Account is created
    pub fn created_account(info: Account) -> Self {
        Self { info, storage_is_clear: Some(1), storage: HashMap::new() }
    }
    /// Should we ask the provider for storage data
    pub fn ask_provider(&self) -> bool {
        self.storage_is_clear.is_none()
    }
}

/// Wrapper around substate and provider, it decouples the database that can be Latest or historical
/// with substate changes that happened previously.
pub struct SubStateWithProvider<'a, SP: StateProvider> {
    /// Substate
    substate: &'a SubStateData,
    /// Provider
    provider: SP,
    /// side chain block hashes
    sidechain_block_hashes: &'a BTreeMap<BlockNumber, BlockHash>,
    /// Last N canonical hashes,
    canonical_block_hashes: &'a BTreeMap<BlockNumber, BlockHash>,
}

impl<'a, SP: StateProvider> SubStateWithProvider<'a, SP> {
    /// Create new substate with provider
    pub fn new(
        substate: &'a SubStateData,
        provider: SP,
        sidechain_block_hashes: &'a BTreeMap<BlockNumber, BlockHash>,
        canonical_block_hashes: &'a BTreeMap<BlockNumber, BlockHash>,
    ) -> Self {
        Self { substate, provider, sidechain_block_hashes, canonical_block_hashes }
    }
}

/* Implement StateProvider traits */

impl<'a, SP: StateProvider> BlockHashProvider for SubStateWithProvider<'a, SP> {
    fn block_hash(&self, number: U256) -> Result<Option<H256>> {
        // All block numbers fit inside u64 and revm checks if it is last 256 block numbers.
        let block_number = number.as_limbs()[0];
        if let Some(sidechain_block_hash) = self.sidechain_block_hashes.get(&block_number).cloned()
        {
            return Ok(Some(sidechain_block_hash))
        }

        Ok(Some(
            self.canonical_block_hashes
                .get(&block_number)
                .cloned()
                .ok_or(ProviderError::BlockchainTreeBlockHash { block_number })?,
        ))
    }
}

impl<'a, SP: StateProvider> AccountProvider for SubStateWithProvider<'a, SP> {
    fn basic_account(&self, address: Address) -> Result<Option<Account>> {
        if let Some(account) = self.substate.accounts.get(&address).map(|acc| acc.info) {
            return Ok(Some(account))
        }
        self.provider.basic_account(address)
    }
}

impl<'a, SP: StateProvider> StateProvider for SubStateWithProvider<'a, SP> {
    fn storage(
        &self,
        account: Address,
        storage_key: reth_primitives::StorageKey,
    ) -> Result<Option<reth_primitives::StorageValue>> {
        if let Some(substate_account) = self.substate.accounts.get(&account) {
            if let Some(storage) = substate_account.storage.get(&storage_key) {
                return Ok(Some(*storage))
            }
            if !substate_account.ask_provider() {
                return Ok(Some(U256::ZERO))
            }
        }
        self.provider.storage(account, storage_key)
    }

    /// Get account and storage proofs.
    fn proof(
        &self,
        _address: Address,
        _keys: &[H256],
    ) -> Result<(Vec<Bytes>, H256, Vec<Vec<Bytes>>)> {
        Err(ProviderError::HistoryStateRoot.into())
    }

    fn bytecode_by_hash(&self, code_hash: H256) -> Result<Option<Bytecode>> {
        if let Some((_, bytecode)) = self.substate.bytecodes.get(&code_hash).cloned() {
            return Ok(Some(bytecode))
        }
        self.provider.bytecode_by_hash(code_hash)
    }
}
