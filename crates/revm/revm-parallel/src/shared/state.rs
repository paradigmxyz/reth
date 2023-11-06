use super::{SharedCacheAccount, SharedCacheState};
use dashmap::DashMap;
use derive_more::Deref;
use parking_lot::RwLock;
use reth_primitives::{BlockNumber, TransitionId, U256};
use revm::{
    db::{
        states::{bundle_state::BundleRetention, plain_account::PlainStorage},
        BundleState,
    },
    primitives::{
        hash_map, Account, AccountInfo, Address, Bytecode, HashMap, B256, BLOCK_HASH_HISTORY,
    },
    DatabaseRef,
};
use std::{
    sync::atomic::{AtomicU64, Ordering},
    vec::Vec,
};

/// State of the blockchain. Port of [revm::db::State] that can be safely shared across threads.
#[derive(Debug)]
pub struct SharedState<DB: DatabaseRef> {
    /// The underlying database provider.
    pub database: DB,
    /// Cached state contains both changed from evm execution and cached/loaded account/storages
    /// from database. This allows us to have only one layer of cache where we can fetch data.
    /// Additionally we can introduce some preloading of data from database.
    pub cache: SharedCacheState,
    /// After block is finishes we merge those changes inside bundle.
    /// Bundle is used to update database and create changesets.
    /// Bundle state can be set on initialization if we want to use preloaded bundle.
    pub bundle_state: BundleState,
    /// If EVM asks for block hash we will first check if they are found here.
    /// and then ask the database.
    ///
    /// This map can be used to give different values for block hashes if in case
    /// The fork block is different or some blocks are not saved inside database.
    pub block_hashes: DashMap<u64, B256>,
    /// The earliest block hash stored in the state.
    pub earliest_block_hash: AtomicU64,
}

impl<DB: DatabaseRef> SharedState<DB> {
    /// Create new shared state.
    pub fn new(database: DB) -> Self {
        Self {
            database,
            cache: SharedCacheState::new(false),
            bundle_state: BundleState::default(),
            block_hashes: DashMap::default(),
            earliest_block_hash: AtomicU64::new(0),
        }
    }

    /// Returns the size hint for the inner bundle state.
    /// See [BundleState::size_hint] for more info.
    pub fn bundle_size_hint(&self) -> usize {
        self.bundle_state.size_hint()
    }

    /// Iterate over received balances and increment all account balances.
    /// If account is not found inside cache state it will be loaded from database.
    ///
    /// Update will create transitions for all accounts that are updated.
    pub fn increment_balances(
        &self,
        block_number: BlockNumber,
        balances: impl IntoIterator<Item = (Address, u128)>,
    ) -> Result<(), DB::Error> {
        let mut accounts = self.cache.accounts.write();
        for (address, balance) in balances.into_iter().filter(|(_, incr)| *incr != 0) {
            match accounts.entry(address) {
                hash_map::Entry::Occupied(mut entry) => {
                    entry.get_mut().increment_balance(block_number, balance)
                }
                hash_map::Entry::Vacant(entry) => {
                    let account = self.load_account_from_database(address)?;
                    entry.insert(account).increment_balance(block_number, balance)
                }
            }
        }
        Ok(())
    }

    /// Drain balances from given account and return those values.
    ///
    /// It is used for DAO hardfork state change to move values from given accounts.
    pub fn drain_balances(
        &mut self,
        block_number: BlockNumber,
        addresses: impl IntoIterator<Item = Address>,
    ) -> Result<Vec<u128>, DB::Error> {
        let mut balances = Vec::new();
        let mut accounts = self.cache.accounts.write();
        for address in addresses {
            let balance = match accounts.entry(address) {
                hash_map::Entry::Occupied(mut entry) => entry.get_mut().drain_balance(block_number),
                hash_map::Entry::Vacant(entry) => {
                    let account = self.load_account_from_database(address)?;
                    entry.insert(account).drain_balance(block_number)
                }
            };
            balances.push(balance);
        }

        Ok(balances)
    }

    /// State clear EIP-161 is enabled in Spurious Dragon hardfork.
    pub fn set_state_clear_flag(&mut self, has_state_clear: bool) {
        self.cache.set_state_clear_flag(has_state_clear);
    }

    /// Insert non existing account into cache.
    pub fn insert_not_existing(&self, address: Address) {
        self.cache.insert_not_existing(address)
    }

    /// Insert account into cache.
    pub fn insert_account(&self, address: Address, info: AccountInfo) {
        self.cache.insert_account(address, info)
    }

    /// Insert account with storage into cache.
    pub fn insert_account_with_storage(
        &self,
        address: Address,
        info: AccountInfo,
        storage: PlainStorage,
    ) {
        self.cache.insert_account_with_storage(address, info, storage)
    }

    /// Take all transitions and merge them inside bundle state.
    /// This action will create final post state and all reverts so that
    /// we at any time revert state of bundle to the state before transition
    /// is applied.
    pub fn merge_transitions(&mut self, block_number: BlockNumber, retention: BundleRetention) {
        let transitions = self.cache.take_transitions(block_number);
        self.bundle_state.apply_transitions_and_create_reverts(transitions, retention);
    }

    /// Load account from database.
    pub fn load_account_from_database(
        &self,
        address: Address,
    ) -> Result<SharedCacheAccount, DB::Error> {
        // if not found in bundle, load it from database
        let info = self.database.basic_ref(address)?;
        let account = match info {
            None => SharedCacheAccount::new_loaded_not_existing(),
            Some(acc) if acc.is_empty() => {
                SharedCacheAccount::new_loaded_empty_eip161(HashMap::new())
            }
            Some(acc) => SharedCacheAccount::new_loaded(acc, HashMap::new()),
        };
        Ok(account)
    }

    /// Commit concurrent evm state results to database.
    pub fn commit(&mut self, evm_states: HashMap<Address, Vec<(TransitionId, Account)>>) {
        self.cache.apply_evm_states(evm_states);
    }

    /// Takes changeset and reverts from state and replaces it with empty one.
    /// This will trop pending Transition and any transitions would be lost.
    pub fn take_bundle(&mut self) -> BundleState {
        core::mem::take(&mut self.bundle_state)
    }
}

impl<DB: DatabaseRef> DatabaseRef for SharedState<DB> {
    type Error = DB::Error;

    fn basic_ref(&self, address: Address) -> Result<Option<AccountInfo>, Self::Error> {
        let accounts = self.cache.accounts.read();
        let account = if let Some(account) = accounts.get(&address) {
            account.latest_account_info()
        } else {
            drop(accounts);
            let account = self.load_account_from_database(address)?;
            self.cache.accounts.write().insert(address, account.clone());
            account.latest_account_info()
        };
        Ok(account)
    }

    fn code_by_hash_ref(&self, code_hash: B256) -> Result<Bytecode, Self::Error> {
        let code = match self.cache.contracts.get(&code_hash) {
            Some(code) => code.clone(),
            None => {
                // if not found in bundle ask database
                let code = self.database.code_by_hash_ref(code_hash)?;
                self.cache.contracts.insert(code_hash, code.clone());
                code
            }
        };
        Ok(code)
    }

    fn storage_ref(&self, address: Address, index: U256) -> Result<U256, Self::Error> {
        // Account is guaranteed to be loaded.
        // Note that storage from bundle is already loaded with account.
        let accounts = self.cache.accounts.read();
        let account = accounts.get(&address).expect("account is loaded");
        let value = if let Some(value) = account.storage_slot(index) {
            value
        } else {
            let is_storage_known = account.previous_status().is_storage_known();
            drop(accounts);

            // If account was destroyed or created, we return zero without loading.
            let value = if is_storage_known {
                U256::ZERO
            } else {
                self.database.storage_ref(address, index)?
            };

            self.cache
                .accounts
                .write()
                .entry(address)
                .and_modify(|account| account.insert_storage_slot(index, value));

            value
        };
        Ok(value)
    }

    fn block_hash_ref(&self, number: U256) -> Result<B256, Self::Error> {
        // block number is never bigger then u64::MAX.
        let number_u64: u64 = number.to();
        let block_hash = match self.block_hashes.get(&number_u64) {
            Some(hash) => *hash,
            None => {
                let block_hash = self.database.block_hash_ref(number)?;
                self.block_hashes.insert(number_u64, block_hash);

                let mut earliest_block_hash = self.earliest_block_hash.load(Ordering::Relaxed);
                if self.earliest_block_hash.load(Ordering::Relaxed) == 0 {
                    earliest_block_hash = number_u64;
                } else {
                    // TODO: fix this
                    // prune all hashes that are older then BLOCK_HASH_HISTORY
                    let minimum_required_block_hash =
                        number_u64.saturating_sub(BLOCK_HASH_HISTORY as u64);
                    while earliest_block_hash < minimum_required_block_hash {
                        self.block_hashes.remove(&earliest_block_hash);
                        earliest_block_hash += 1;
                    }
                }
                self.earliest_block_hash.store(earliest_block_hash, Ordering::Relaxed);

                block_hash
            }
        };
        Ok(block_hash)
    }
}

/// Shared state behind RW lock.
#[derive(Deref, Debug)]
pub struct SharedStateLock<DB: DatabaseRef>(RwLock<SharedState<DB>>);

impl<DB: DatabaseRef> SharedStateLock<DB> {
    /// Create new locked shared state.
    pub fn new(state: SharedState<DB>) -> Self {
        Self(RwLock::new(state))
    }
}

impl<DB: DatabaseRef> DatabaseRef for SharedStateLock<DB> {
    type Error = DB::Error;

    fn basic_ref(&self, address: Address) -> Result<Option<AccountInfo>, Self::Error> {
        self.read().basic_ref(address)
    }

    fn code_by_hash_ref(&self, code_hash: B256) -> Result<Bytecode, Self::Error> {
        self.read().code_by_hash_ref(code_hash)
    }

    fn storage_ref(&self, address: Address, index: U256) -> Result<U256, Self::Error> {
        self.read().storage_ref(address, index)
    }

    fn block_hash_ref(&self, number: U256) -> Result<B256, Self::Error> {
        self.read().block_hash_ref(number)
    }
}
