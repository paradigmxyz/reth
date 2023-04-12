use reth_executor::cache::ExecutionCache;
use reth_interfaces::Error;
use reth_primitives::{Account, Address, Bytecode, H160, H256, KECCAK_EMPTY, U256};
use reth_provider::{post_state::Storage, StateProvider};
use reth_revm_primitives::to_reth_acc;
use revm::{db::DatabaseRef, primitives::AccountInfo, Database, DatabaseCommit};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

/// A substate contains the current world state after execution. It uses a [`StateProvider`] to
/// fetch pre-execution state from disk.
///
/// The database can be used with a shared [`ExecutionCache`] to reduce reads in-between execution
/// of blocks or batches of blocks.
pub struct SubState<SP: StateProvider> {
    cache: Arc<Mutex<ExecutionCache>>,
    substate: FlatCache,
    state_provider: SP,
}

impl<SP: StateProvider> SubState<SP> {
    /// Create a new `SubState` database with the given state provider and a 0-capacity execution
    /// cache.
    pub fn new(state_provider: SP) -> Self {
        Self::new_with_cache(state_provider, Arc::new(Mutex::new(ExecutionCache::new(0, 0, 0))))
    }

    /// Create a new `SubState` database with the given state provider and execution cache.
    pub fn new_with_cache(state_provider: SP, cache: Arc<Mutex<ExecutionCache>>) -> Self {
        Self { cache, substate: FlatCache::default(), state_provider }
    }

    /// Check if the cache contains a specific bytecode.
    ///
    /// # Note
    ///
    /// This will not attempt to load from the database if the bytecode is not in the cache.
    pub fn has_code(&mut self, code_hash: H256) -> bool {
        if code_hash == KECCAK_EMPTY {
            true
        } else if let Some(_) =
            self.cache.lock().expect("Could not lock execution cache").get_bytecode(code_hash)
        {
            true
        } else {
            self.substate.bytecode.contains_key(&code_hash)
        }
    }
}

impl<SP: StateProvider> DatabaseCommit for SubState<SP> {
    fn commit(&mut self, changes: revm::primitives::HashMap<Address, revm::primitives::Account>) {
        let mut cache = self.cache.lock().expect("Could not lock execution cache");

        for (address, mut account) in changes {
            // Account was destroyed
            if account.is_destroyed {
                cache.update_account(address, None);
                cache.clear_storage(address);
                let db_account = self.substate.accounts.entry(address).or_default();
                if let Some(storage) = self.substate.storages.get_mut(&address) {
                    storage.storage.clear();
                    storage.wiped = true;
                }
                *db_account = None;
                continue
            }

            // Insert contract
            if let Some(code) = &account.info.code {
                if !code.is_empty() {
                    account.info.code_hash = code.hash();
                    self.substate
                        .bytecode
                        .entry(account.info.code_hash)
                        .or_insert_with(|| Bytecode(code.clone()));
                    cache.update_bytecode(account.info.code_hash, Bytecode(code.clone()));
                }
            }
            if account.info.code_hash == H256::zero() {
                account.info.code_hash = KECCAK_EMPTY;
            }

            // Update account
            let db_account = self.substate.accounts.entry(address).or_default();
            *db_account = Some(to_reth_acc(&account.info));
            cache.update_account(address, db_account.clone());

            // Update storage
            if account.storage_cleared {
                self.substate.storages.entry(address).and_modify(|storage| {
                    storage.storage.clear();
                    storage.wiped = true;
                });
                cache.clear_storage(address);
            };

            if !account.storage.is_empty() {
                cache.update_storage(
                    address,
                    account
                        .storage
                        .clone()
                        .into_iter()
                        .map(|(key, value)| (key, value.present_value())),
                );
                self.substate.storages.entry(address).or_default().storage.extend(
                    account.storage.into_iter().map(|(key, value)| (key, value.present_value())),
                );
            }
        }
    }
}

impl<SP: StateProvider> Database for SubState<SP> {
    type Error = Error;

    fn basic(&mut self, address: H160) -> Result<Option<AccountInfo>, Self::Error> {
        Ok(self
            .cache
            .lock()
            .expect("Could not lock execution cache")
            .get_account_with(address, || {
                if let Some(v) = self.substate.accounts.get(&address) {
                    Ok(v.clone())
                } else {
                    self.state_provider.basic_account(address)
                }
            })?
            .map(|account| AccountInfo {
                balance: account.balance,
                nonce: account.nonce,
                code_hash: account.bytecode_hash.unwrap_or(KECCAK_EMPTY),
                code: None,
            }))
    }

    fn code_by_hash(&mut self, code_hash: H256) -> Result<revm::primitives::Bytecode, Self::Error> {
        if code_hash == KECCAK_EMPTY {
            return Ok(revm::primitives::Bytecode::new())
        }

        Ok(self
            .cache
            .lock()
            .expect("Could not lock execution cache")
            .get_bytecode_with(code_hash, || {
                if let Some(v) = self.substate.bytecode.get(&code_hash).cloned() {
                    Ok(Some(v))
                } else {
                    self.state_provider.bytecode_by_hash(code_hash)
                }
            })?
            .map(|b| b.with_code_hash(code_hash).0)
            .unwrap_or(revm::primitives::Bytecode::new()))
    }

    fn storage(&mut self, address: H160, index: U256) -> Result<U256, Self::Error> {
        Ok(self
            .cache
            .lock()
            .expect("Could not lock execution cache")
            .get_storage_with(address, index.into(), || {
                if let Some(v) = self.substate.storages.get(&address).and_then(|storage| {
                    if let Some(v) = storage.storage.get(&index) {
                        Some(*v)
                    } else if storage.wiped {
                        Some(U256::ZERO)
                    } else {
                        None
                    }
                }) {
                    Ok(v)
                } else {
                    self.state_provider
                        .storage(address, index.into())
                        .map(|v| v.unwrap_or_default())
                }
            })?
            .into())
    }

    fn block_hash(&mut self, number: U256) -> Result<H256, Self::Error> {
        // Note: this unwrap is potentially unsafe
        Ok(self.state_provider.block_hash(number.try_into().unwrap())?.unwrap_or_default())
    }
}

/// This is a dumb unbounded flat cache for EVM objects.
///
/// It is used in tandem with an [`ExecutionCache`] to ensure changes to state that have not been
/// committed to disk are still available, as those objects might get evicted from the LRU in the
/// [`ExecutionCache`].
///
/// Ideally, this is either replaced with a reference to `PostState` (since it already contains all
/// of this information), or it is repurposed to *only* being an overflow cache, as currently the
/// [`SubState`] database writes to both the execution cache and this cache.
#[derive(Default)]
struct FlatCache {
    /// The account cache. `None` represents that the account does not exist.
    accounts: HashMap<Address, Option<Account>>,
    /// The storage cache.
    storages: HashMap<Address, Storage>,
    /// The bytecode cache.
    bytecode: HashMap<H256, Bytecode>,
}

/// Wrapper around a [`StateProvider`] that implements the revm [`DatabaseRef`] trait.
pub struct StateProviderDatabase<DB: StateProvider>(pub DB);

impl<DB: StateProvider> StateProviderDatabase<DB> {
    /// Create a new [`StateProviderDatabase`] with the given state provider.
    pub fn new(db: DB) -> Self {
        Self(db)
    }
}

impl<DB: StateProvider> DatabaseRef for StateProviderDatabase<DB> {
    type Error = Error;

    fn basic(&self, address: H160) -> Result<Option<AccountInfo>, Self::Error> {
        Ok(self.0.basic_account(address)?.map(|account| AccountInfo {
            balance: account.balance,
            nonce: account.nonce,
            code_hash: account.bytecode_hash.unwrap_or(KECCAK_EMPTY),
            code: None,
        }))
    }

    fn code_by_hash(&self, code_hash: H256) -> Result<revm::primitives::Bytecode, Self::Error> {
        let bytecode = self.0.bytecode_by_hash(code_hash)?;

        if let Some(bytecode) = bytecode {
            Ok(bytecode.with_code_hash(code_hash).0)
        } else {
            Ok(revm::primitives::Bytecode::new())
        }
    }

    fn storage(&self, address: H160, index: U256) -> Result<U256, Self::Error> {
        let index = H256(index.to_be_bytes());
        let ret = self.0.storage(address, index)?.unwrap_or_default();
        Ok(ret)
    }

    fn block_hash(&self, number: U256) -> Result<H256, Self::Error> {
        // Note: this unwrap is potentially unsafe
        Ok(self.0.block_hash(number.try_into().unwrap())?.unwrap_or_default())
    }
}
