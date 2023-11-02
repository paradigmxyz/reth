use derive_more::Deref;
use reth_primitives::{alloy_primitives::I256, B256, KECCAK_EMPTY};
use revm::{
    db::{
        states::{plain_account::PlainStorage, CacheAccount as RevmCacheAccount},
        AccountStatus, BundleAccount, PlainAccount, TransitionAccount,
    },
    primitives::{hash_map, Account, AccountInfo, Bytecode, HashMap, StorageSlot, U256},
};

/// Data alongside an index when it was last revised.
#[derive(Deref, Clone, Debug, PartialEq, Eq)]
struct RevisedData<T> {
    /// Some data.
    #[deref]
    data: T,
    /// The index of the latest revision of this data.
    revision: Option<usize>,
}

impl<T> RevisedData<T> {
    /// Create new data with revision.
    fn new(data: T) -> Self {
        Self { data, revision: None }
    }

    fn with_revision(mut self, revision: usize) -> Self {
        self.revision = Some(revision);
        self
    }

    fn update_revision(&mut self, data: T, revision: usize) {
        if Some(revision) > self.revision {
            self.data = data;
            self.revision = Some(revision);
        }
    }
}

#[derive(PartialEq, Eq, Clone, Debug)]
struct AccountCode {
    code_hash: B256,
    code: Option<Bytecode>,
}

impl Default for AccountCode {
    fn default() -> Self {
        Self { code_hash: KECCAK_EMPTY, code: None }
    }
}

impl AccountCode {
    fn new(code_hash: B256, code: Option<Bytecode>) -> Self {
        Self { code_hash, code }
    }
}

#[derive(PartialEq, Eq, Clone, Debug)]
struct AccountInfoDiff {
    /// Nonce delta.
    nonce_delta: i64,
    /// Balance delta.
    balance_delta: I256,
    /// The latest code for the account.
    code: RevisedData<Option<AccountCode>>,
}

impl Default for AccountInfoDiff {
    fn default() -> Self {
        Self { nonce_delta: 0, balance_delta: I256::ZERO, code: RevisedData::new(None) }
    }
}

impl AccountInfoDiff {
    fn with_code(mut self, code: Option<AccountCode>) -> Self {
        self.code = RevisedData::new(code);
        self
    }

    fn update_diff(
        &mut self,
        old: &Option<AccountInfo>,
        new: Option<AccountInfo>,
        revision: usize,
    ) {
        self.update_balance_delta(
            old.as_ref().map(|info| info.balance).unwrap_or_default(),
            new.as_ref().map(|info| info.balance).unwrap_or_default(),
        );
        self.update_nonce_delta(
            old.as_ref().map(|info| info.nonce).unwrap_or_default(),
            new.as_ref().map(|info| info.nonce).unwrap_or_default(),
        );
        self.update_code(new.map(|info| AccountCode::new(info.code_hash, info.code)), revision);
    }

    /// Updates the current nonce delta.
    fn update_nonce_delta(&mut self, old_nonce: u64, new_nonce: u64) {
        let mut delta = i64::try_from(old_nonce.abs_diff(new_nonce)).unwrap();
        if new_nonce < old_nonce {
            delta = delta.checked_neg().unwrap();
        }
        self.nonce_delta += delta;
    }

    /// Updates the current balance delta.
    fn update_balance_delta(&mut self, old_balance: U256, new_balance: U256) {
        let mut delta = I256::try_from(old_balance.abs_diff(new_balance)).unwrap();
        if new_balance < old_balance {
            delta = delta.checked_neg().unwrap();
        }
        self.balance_delta += delta;
    }

    /// Update code hash if it has a more recent revision.
    fn update_code(&mut self, code: Option<AccountCode>, revision: usize) {
        self.code.update_revision(code, revision);
    }
}

/// Cache account contains plain state that gets updated
/// at every transaction when evm output is applied to CacheState.
#[derive(PartialEq, Eq, Clone, Debug)]
pub struct SharedCacheAccount {
    /// Previous account information.
    previous_info: Option<AccountInfo>,
    /// Previous account status.
    previous_status: AccountStatus,
    /// Flag indicating whether the account was touched.
    touched: bool,
    /// The account differences between original and changed.
    info_diff: AccountInfoDiff,
    /// Account storage.
    storage: HashMap<U256, RevisedData<StorageSlot>>,
    /// The index of the revision where the latest selfdestruct occurred.
    selfdestruct_index: Option<usize>,
    /// The number of times the account was selfdestructed.
    selfdestruct_count: u32,
}

impl From<SharedCacheAccount> for RevmCacheAccount {
    fn from(value: SharedCacheAccount) -> Self {
        Self {
            account: value.previous_info.map(|info| PlainAccount {
                info,
                storage: value
                    .storage
                    .into_iter()
                    .map(|(slot, value)| (slot, value.data.present_value))
                    .collect(),
            }),
            status: value.previous_status,
        }
    }
}

impl From<BundleAccount> for SharedCacheAccount {
    fn from(account: BundleAccount) -> Self {
        let previous_info = account.account_info();
        let info_diff = AccountInfoDiff::default().with_code(
            previous_info.as_ref().map(|info| AccountCode::new(info.code_hash, info.code.clone())),
        );
        let storage = account
            .storage
            .into_iter()
            .map(|(k, v)| (k, RevisedData::new(StorageSlot::new(v.present_value))))
            .collect();
        Self {
            previous_info,
            previous_status: account.status,
            info_diff,
            storage,
            touched: false,
            selfdestruct_count: 0,
            selfdestruct_index: None,
        }
    }
}

impl SharedCacheAccount {
    /// Create new cache account
    pub fn new(original_status: AccountStatus) -> Self {
        Self {
            previous_info: None,
            previous_status: original_status,
            info_diff: AccountInfoDiff::default(),
            touched: false,
            storage: HashMap::default(),
            selfdestruct_count: 0,
            selfdestruct_index: None,
        }
    }

    /// Set original account info.
    pub fn with_info(mut self, info: AccountInfo) -> Self {
        self.info_diff.code =
            RevisedData::new(Some(AccountCode::new(info.code_hash, info.code.clone())));
        self.previous_info = Some(info);
        self
    }

    /// Set account storage.
    pub fn with_storage(mut self, storage: impl IntoIterator<Item = (U256, U256)>) -> Self {
        self.storage = storage
            .into_iter()
            .map(|(slot, value)| (slot, RevisedData::new(StorageSlot::new(value))))
            .collect();
        self
    }

    /// Create new account that is loaded from database.
    pub fn new_loaded(info: AccountInfo, storage: PlainStorage) -> Self {
        Self::new(AccountStatus::Loaded).with_info(info).with_storage(storage)
    }

    /// Create new account that is loaded empty from database.
    pub fn new_loaded_empty_eip161(storage: PlainStorage) -> Self {
        Self::new(AccountStatus::LoadedEmptyEIP161)
            .with_info(AccountInfo::default())
            .with_storage(storage)
    }

    /// Loaded not existing account.
    pub fn new_loaded_not_existing() -> Self {
        Self::new(AccountStatus::LoadedNotExisting)
    }

    /// Create new account that is newly created
    pub fn new_newly_created(info: AccountInfo, storage: PlainStorage) -> Self {
        Self::new(AccountStatus::InMemoryChange).with_info(info).with_storage(storage)
    }

    /// Create account that is destroyed.
    pub fn new_destroyed() -> Self {
        Self::new(AccountStatus::Destroyed)
    }

    /// Create changed account
    pub fn new_changed(info: AccountInfo, storage: PlainStorage) -> Self {
        Self::new(AccountStatus::Changed).with_info(info).with_storage(storage)
    }

    /// Return storage slot value.
    pub fn storage_slot(&self, slot: U256) -> Option<U256> {
        self.storage.get(&slot).map(|value| value.present_value)
    }

    /// Get or load and insert storage slot.
    pub fn get_or_insert_storage_slot<Error>(
        &mut self,
        slot: U256,
        load: impl FnOnce() -> Result<U256, Error>,
    ) -> Result<U256, Error> {
        match self.storage.entry(slot) {
            hash_map::Entry::Occupied(entry) => Ok(entry.get().present_value()),
            hash_map::Entry::Vacant(entry) => {
                // If account was destroyed or created, we return zero without loading.
                let value = if self.selfdestruct_index.is_some() ||
                    self.previous_status.is_storage_known()
                {
                    U256::ZERO
                } else {
                    load()?
                };
                entry.insert(RevisedData::new(StorageSlot::new(value)));
                Ok(value)
            }
        }
    }

    /// Return the status of account as it was loaded or after last transition applied
    pub fn previous_status(&self) -> AccountStatus {
        self.previous_status
    }

    /// Returns current balance as it should be reported to EVM.
    pub fn account_balance(&self) -> U256 {
        let original = self.previous_info.as_ref().map(|info| info.balance).unwrap_or_default();
        let balance_delta = self.info_diff.balance_delta;
        if balance_delta.is_negative() {
            original.checked_sub(U256::try_from(balance_delta.abs()).unwrap()).unwrap()
        } else {
            original.checked_add(U256::try_from(balance_delta).unwrap()).unwrap()
        }
    }

    /// Return current account nonce.
    pub fn account_nonce(&self) -> u64 {
        let original = self.previous_info.as_ref().map(|info| info.nonce).unwrap_or_default();
        let nonce_delta = self.info_diff.nonce_delta;
        if nonce_delta.is_negative() {
            original.checked_sub(u64::try_from(nonce_delta.abs()).unwrap()).unwrap()
        } else {
            original.checked_add(u64::try_from(nonce_delta).unwrap()).unwrap()
        }
    }

    /// Fetch account info if it exist.
    pub fn account_info(&self) -> Option<AccountInfo> {
        let code = self.info_diff.code.data.clone().unwrap_or_default();
        let info = AccountInfo {
            balance: self.account_balance(),
            nonce: self.account_nonce(),
            code_hash: code.code_hash,
            code: code.code,
        };
        // TODO: handle empty pre state clear
        if info.is_empty() {
            None
        } else {
            Some(info)
        }
    }

    /// Increment balance by `balance` amount. Assume that balance will not
    /// overflow or be zero.
    ///
    /// Note: only if balance is zero we would return None as no transition would be made.
    pub fn increment_balance(&mut self, increment: u128) {
        if increment != 0 {
            self.account_balance_change(|balance| balance.saturating_add(U256::from(increment)));
        }
    }

    /// Drain balance from account and return drained amount and transition.
    ///
    /// Used for DAO hardfork transition.
    pub fn drain_balance(&mut self) -> u128 {
        self.account_balance_change(|_balance| U256::ZERO).try_into().unwrap()
    }

    fn account_balance_change<F: FnOnce(U256) -> U256>(&mut self, compute_balance: F) -> U256 {
        self.touched |= true;
        let current_balance = self.account_balance();
        let new_balance = compute_balance(current_balance);
        self.info_diff.update_balance_delta(current_balance, new_balance);
        new_balance
    }

    /// Apply single account revision. Revisions are expected to come out of order.
    pub fn apply_account_revision(
        &mut self,
        previous_info: &Option<AccountInfo>,
        account: Account,
        revision: usize,
    ) {
        // Untouched account is never changed.
        if !account.is_touched() {
            return
        }

        self.touched |= true;

        // If it is marked as selfdestructed inside revm we need to change destroy the state
        // unless later revisions have already been applied.
        if account.is_selfdestructed() {
            self.info_diff.update_diff(previous_info, None, revision);
            self.storage.retain(|_slot, value| value.revision > Some(revision));
            self.selfdestruct_count += 1;
            self.selfdestruct_index = Some(revision);
            return
        }

        self.info_diff.update_diff(previous_info, Some(account.info), revision);

        let is_destroyed = self.selfdestruct_index.map_or(false, |index| revision < index);
        for (slot, value) in account.storage {
            // The value might have been destroyed in the further revision.
            let current = if is_destroyed { U256::ZERO } else { value.present_value };
            match self.storage.entry(slot) {
                hash_map::Entry::Vacant(entry) => {
                    entry.insert(
                        RevisedData::new(StorageSlot::new_changed(value.original_value(), current))
                            .with_revision(revision),
                    );
                }
                hash_map::Entry::Occupied(mut entry) => {
                    let existing = entry.get_mut();
                    existing.update_revision(
                        StorageSlot::new_changed(existing.original_value(), current),
                        revision,
                    );
                }
            };
        }
    }

    /// Get the next account status based on changes.
    /// TODO: Handle empty accounts
    pub fn next_status(
        &self,
        current_info: &Option<AccountInfo>,
        storage_changed: bool,
        _state_clear_enabled: bool,
    ) -> AccountStatus {
        let changed = current_info != &self.previous_info || storage_changed;
        let had_no_nonce_and_code =
            self.previous_info.as_ref().map(AccountInfo::has_no_code_and_nonce).unwrap_or_default();

        if self.previous_status == AccountStatus::LoadedNotExisting {
            if changed {
                AccountStatus::InMemoryChange
            } else {
                AccountStatus::LoadedNotExisting
            }
        } else if self.selfdestruct_index.is_some() {
            if current_info.is_some() {
                AccountStatus::DestroyedChanged
            } else if self.previous_status.was_destroyed() || self.selfdestruct_count > 1 {
                AccountStatus::DestroyedAgain
            } else {
                AccountStatus::Destroyed
            }
        } else if self.previous_info.is_none() && current_info.is_none() {
            self.previous_status.on_touched_empty_post_eip161()
        } else {
            self.previous_status.on_changed(had_no_nonce_and_code)
        }
    }

    /// Finalize account state change and return the transition if any occurred.
    pub fn finalize_transition(&mut self, state_clear_enabled: bool) -> Option<TransitionAccount> {
        // Untouched accounts do not have transitions.
        if !self.touched {
            return None
        }

        let mut transition_storage = HashMap::<U256, StorageSlot>::default();
        for (slot, value) in self.storage.iter_mut() {
            if value.is_changed() {
                transition_storage.insert(*slot, value.data.clone());
            }
            *value = RevisedData::new(StorageSlot::new(value.present_value));
        }

        let info = self.account_info();
        let next_status =
            self.next_status(&info, !transition_storage.is_empty(), state_clear_enabled);

        let transition = if next_status != self.previous_status ||
            info != self.previous_info ||
            !transition_storage.is_empty()
        {
            Some(TransitionAccount {
                info: info.clone(),
                status: next_status,
                previous_info: self.previous_info.clone(),
                previous_status: self.previous_status,
                storage: transition_storage,
                storage_was_destroyed: self.selfdestruct_index.is_some(),
            })
        } else {
            None
        };

        self.info_diff = AccountInfoDiff::default().with_code(
            info.as_ref().map(|info| AccountCode::new(info.code_hash, info.code.clone())),
        );
        self.previous_info = info;
        self.previous_status = next_status;
        self.touched = false;
        self.selfdestruct_count = 0;
        self.selfdestruct_index = None;

        transition
    }
}
