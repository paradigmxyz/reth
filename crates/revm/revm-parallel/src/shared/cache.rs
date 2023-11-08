use super::SharedCacheAccount;
use dashmap::DashMap;
use parking_lot::RwLock;
use reth_primitives::{BlockNumber, TransitionId};
use revm::{
    db::states::plain_account::PlainStorage,
    primitives::{Account, AccountInfo, Address, Bytecode, HashMap, B256},
    TransitionState,
};
use std::collections::HashSet;

/// Cache state contains both modified and original values.
///
/// Cache state is main state that revm uses to access state.
/// It loads all accounts from database and applies revm output to it.
///
/// It generates transitions that is used to build BundleState.
#[derive(Debug)]
pub struct SharedCacheState {
    /// Block state account with account state
    pub accounts: RwLock<HashMap<Address, SharedCacheAccount>>,
    /// Mapping of the code hash of created contracts to the respective bytecode.
    pub contracts: DashMap<B256, Bytecode>,
    /// Touched accounts.
    pub touched: HashMap<BlockNumber, HashSet<Address>>,
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
        Self {
            accounts: RwLock::new(HashMap::default()),
            contracts: DashMap::default(),
            touched: HashMap::default(),
            has_state_clear,
        }
    }

    /// Set state clear flag. EIP-161.
    pub fn set_state_clear_flag(&mut self, has_state_clear: bool) {
        self.has_state_clear = has_state_clear;
    }

    /// Insert not existing account.
    pub fn insert_not_existing(&self, address: Address) {
        self.accounts.write().insert(address, SharedCacheAccount::new_loaded_not_existing());
    }

    /// Insert Loaded (Or LoadedEmptyEip161 if account is empty) account.
    pub fn insert_account(&self, address: Address, info: AccountInfo) {
        let account = if !info.is_empty() {
            SharedCacheAccount::new_loaded(info, HashMap::default())
        } else {
            SharedCacheAccount::new_loaded_empty_eip161(HashMap::default())
        };
        self.accounts.write().insert(address, account);
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
        self.accounts.write().insert(address, account);
    }

    /// Apply outputs of EVM execution.
    pub fn apply_evm_states(
        &mut self,
        account_states: HashMap<Address, Vec<(TransitionId, Account)>>,
    ) {
        let mut accounts = self.accounts.write();
        for (address, account_states) in account_states {
            let this_account = accounts.get_mut(&address).expect("account must be present");
            let previous_info = this_account.latest_account_info();

            for (transition_id, account) in account_states {
                if account.is_touched() {
                    self.touched.entry(transition_id.0).or_default().insert(address);
                }
                this_account.apply_account_transition(&previous_info, account, transition_id);
            }
        }
    }

    /// Take account transitions from shared cache state.
    pub fn take_transitions(&mut self, block_number: BlockNumber) -> TransitionState {
        let mut accounts = self.accounts.write();
        let mut transitions = HashMap::default();
        for address in self.touched.remove(&block_number).unwrap_or_default() {
            let account = accounts.get_mut(&address).unwrap();
            if let Some(transition) =
                account.finalize_transition(block_number, self.has_state_clear)
            {
                transitions.insert(address, transition);
            }
        }
        TransitionState { transitions }
    }
}
