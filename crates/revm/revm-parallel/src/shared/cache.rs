use super::SharedCacheAccount;
use dashmap::DashMap;
use rayon::prelude::*;
use revm::{
    db::states::plain_account::PlainStorage,
    primitives::{Account, AccountInfo, Address, Bytecode, HashMap, State as EVMState, B256},
    TransitionAccount, TransitionState,
};

/// Cache state contains both modified and original values.
///
/// Cache state is main state that revm uses to access state.
/// It loads all accounts from database and applies revm output to it.
///
/// It generates transitions that is used to build BundleState.
#[derive(Debug)]
pub struct SharedCacheState {
    /// Block state account with account state
    pub accounts: DashMap<Address, SharedCacheAccount>,
    /// Mapping of the code hash of created contracts to the respective bytecode.
    pub contracts: DashMap<B256, Bytecode>,
    /// Has EIP-161 state clear enabled (Spurious Dragon hardfork).
    pub has_state_clear: bool,
}

impl Default for SharedCacheState {
    fn default() -> Self {
        Self::new(true)
    }
}

impl SharedCacheState {
    /// New default state.
    pub fn new(has_state_clear: bool) -> Self {
        Self { accounts: DashMap::default(), contracts: DashMap::default(), has_state_clear }
    }

    /// Set state clear flag. EIP-161.
    pub fn set_state_clear_flag(&mut self, has_state_clear: bool) {
        self.has_state_clear = has_state_clear;
    }

    /// Insert not existing account.
    pub fn insert_not_existing(&self, address: Address) {
        self.accounts.insert(address, SharedCacheAccount::new_loaded_not_existing());
    }

    /// Insert Loaded (Or LoadedEmptyEip161 if account is empty) account.
    pub fn insert_account(&self, address: Address, info: AccountInfo) {
        let account = if !info.is_empty() {
            SharedCacheAccount::new_loaded(info, HashMap::default())
        } else {
            SharedCacheAccount::new_loaded_empty_eip161(HashMap::default())
        };
        self.accounts.insert(address, account);
    }

    /// Similar to `insert_account` but with storage.
    pub fn insert_account_with_storage(
        &self,
        address: Address,
        info: AccountInfo,
        storage: PlainStorage,
    ) {
        let account = if !info.is_empty() {
            SharedCacheAccount::new_loaded(info, storage)
        } else {
            SharedCacheAccount::new_loaded_empty_eip161(storage)
        };
        self.accounts.insert(address, account);
    }

    /// Take account transitions from shared cache state.
    pub fn take_transitions(&mut self) -> TransitionState {
        let transitions: Vec<(Address, TransitionAccount)> = self
            .accounts
            .par_iter_mut()
            .filter_map(|mut account| {
                account
                    .finalize_transition(self.has_state_clear)
                    .map(|transition| (*account.key(), transition))
            })
            .collect();
        TransitionState { transitions: HashMap::from_iter(transitions) }
    }

    /// Apply outputs of EVM execution.
    pub fn apply_evm_states(&mut self, evm_states: Vec<(usize, EVMState)>) {
        let mut accounts = HashMap::<Address, Vec<(usize, Account)>>::default();
        for (revision, state) in evm_states {
            for (address, account) in state {
                accounts.entry(address).or_default().push((revision, account));
            }
        }

        for (address, account_states) in accounts {
            let mut this_account =
                self.accounts.get_mut(&address).expect("account must be present");
            let previous_info = this_account.account_info();

            for (revision, account) in account_states {
                this_account.apply_account_revision(&previous_info, account, revision);
            }
        }
    }
}
