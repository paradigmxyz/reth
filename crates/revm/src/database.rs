use reth_executor::cache::ExecutionCache;
use reth_interfaces::Error;
use reth_primitives::{Account, Address, Bytecode, H160, H256, KECCAK_EMPTY, U256};
use reth_provider::{post_state::Storage, StateProvider};
use reth_revm_primitives::to_reth_acc;
use revm::{
    db::{CacheDB, DatabaseRef},
    primitives::AccountInfo,
    Database, DatabaseCommit,
};
use std::{
    collections::{hash_map::Entry, HashMap},
    sync::{Arc, Mutex},
};

#[derive(Default)]
struct SubStateInner {
    /// Account info where None means it is not existing. Not existing state is needed for Pre
    /// TANGERINE forks. `code` is always `None`, and bytecode can be found in `contracts`.
    pub accounts: HashMap<Address, Option<Account>>,
    pub storages: HashMap<Address, Storage>,
    pub bytecode: HashMap<H256, reth_primitives::Bytecode>,
    pub block_hashes: HashMap<U256, U256>,
}

// TODO
pub struct SubState<SP: StateProvider> {
    cache: Arc<Mutex<ExecutionCache>>,
    substate: SubStateInner,
    state_provider: SP,
}

impl<SP: StateProvider> SubState<SP> {
    pub fn new(state_provider: SP) -> Self {
        Self::new_with_cache(state_provider, Arc::new(Mutex::new(ExecutionCache::new(0, 0, 0))))
    }

    pub fn new_with_cache(state_provider: SP, cache: Arc<Mutex<ExecutionCache>>) -> Self {
        Self { cache, substate: SubStateInner::default(), state_provider }
    }

    #[deprecated]
    pub fn load_account(&mut self, address: Address) -> Result<&mut Option<Account>, Error> {
        // TODO: Make this function go away. We should not mutate this state directly.
        // Currently, as a fast PoC, I just remove the loaded account from the LRU and manage the
        // state using the dumb cache.
        self.cache.lock().expect("Could not lock execution cache").drop_account(address);
        match self.substate.accounts.entry(address) {
            Entry::Occupied(entry) => Ok(entry.into_mut()),
            Entry::Vacant(entry) => Ok(entry.insert(self.state_provider.basic_account(address)?)),
        }
    }

    #[deprecated]
    pub fn substate_account(&mut self, address: Address) -> Option<Account> {
        if let Some(v) =
            self.cache.lock().expect("Could not lock execution cache").get_account(address)
        {
            Some(v.clone())
        } else if let Some(account) = self.substate.accounts.get(&address).map(|a| a.as_ref()) {
            account.cloned()
        } else {
            None
        }
    }

    #[deprecated]
    pub fn has_code(&mut self, code_hash: H256) -> bool {
        if let Some(_) =
            self.cache.lock().expect("Could not lock execution cache").get_bytecode(code_hash)
        {
            true
        } else {
            self.substate.bytecode.contains_key(&code_hash)
        }
    }

    #[deprecated]
    pub fn state(&self) -> &SP {
        &self.state_provider
    }
}

impl<SP: StateProvider> DatabaseCommit for SubState<SP> {
    fn commit(&mut self, changes: revm::primitives::HashMap<Address, revm::primitives::Account>) {
        let mut cache = self.cache.lock().expect("Could not lock execution cache");

        for (address, mut account) in changes {
            let fresh = cache.get_account(address).is_none() &&
                !self.substate.accounts.contains_key(&address);

            // TODO: If we do not have the account in LRU *or* substate, then this is an entirely
            // new account and we can mark it as fresh. If the account is fresh (in the
            // substate or LRU) we do not need to ask the underlying state provider for storage
            // slots.

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
            // TODO: Maybe doesnt exist account state will fuck things up here ?
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

// TODO: Remove unwraps
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
        // TODO: Wiped storage check
        Ok(self
            .cache
            .lock()
            .expect("Could not lock execution cache")
            .get_storage_with(address, index.into(), || {
                if let Some(v) = self
                    .substate
                    .storages
                    .get(&address)
                    .and_then(|a| a.storage.get(&index).cloned())
                {
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

/// Wrapper around StateProvider that implements revm database trait
#[deprecated]
pub struct State<DB: StateProvider>(pub DB);

impl<DB: StateProvider> State<DB> {
    /// Create new State with generic StateProvider.
    pub fn new(db: DB) -> Self {
        Self(db)
    }

    /// Return inner state reference
    pub fn state(&self) -> &DB {
        &self.0
    }

    /// Return inner state mutable reference
    pub fn state_mut(&mut self) -> &mut DB {
        &mut self.0
    }

    /// Consume State and return inner StateProvider.
    pub fn into_inner(self) -> DB {
        self.0
    }
}

impl<DB: StateProvider> DatabaseRef for State<DB> {
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
