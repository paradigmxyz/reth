use derive_more::Deref;
use reth_primitives::{
    alloy_primitives::I256, BlockNumber, TransitionId, TransitionType, B256, KECCAK_EMPTY,
};
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
    /// The transition id of the latest revision of this data.
    revision: Option<TransitionId>,
}

impl<T> RevisedData<T> {
    /// Create new data with revision.
    fn new(data: T) -> Self {
        Self { data, revision: None }
    }

    fn with_revision(mut self, revision: TransitionId) -> Self {
        self.revision = Some(revision);
        self
    }

    fn update_with_revision(&mut self, data: T, revision: TransitionId) {
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
pub struct AccountInfoDiff {
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
        revision: TransitionId,
        update_code: bool,
    ) {
        self.update_balance_delta(
            old.as_ref().map(|info| info.balance).unwrap_or_default(),
            new.as_ref().map(|info| info.balance).unwrap_or_default(),
        );
        self.update_nonce_delta(
            old.as_ref().map(|info| info.nonce).unwrap_or_default(),
            new.as_ref().map(|info| info.nonce).unwrap_or_default(),
        );
        if update_code {
            self.update_code(new.map(|info| AccountCode::new(info.code_hash, info.code)), revision);
        }
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
    fn update_code(&mut self, code: Option<AccountCode>, revision: TransitionId) {
        self.code.update_with_revision(code, revision);
    }
}

#[derive(PartialEq, Eq, Clone, Default, Debug)]
pub struct SharedAccountTransition {
    /// The account differences between original and changed.
    info_diff: AccountInfoDiff,
    /// Account storage.
    storage: HashMap<U256, RevisedData<StorageSlot>>,
    /// The index of the revision where the latest selfdestruct occurred.
    selfdestruct_transition: Option<TransitionId>,
    /// The number of times the account was selfdestructed.
    selfdestruct_count: u32,
}

/// Cache account contains plain state that gets updated
/// at every transaction when evm output is applied to CacheState.
#[derive(PartialEq, Eq, Clone, Debug)]
pub struct SharedCacheAccount {
    /// Previous account information.
    previous_info: Option<AccountInfo>,
    /// Previous account status.
    previous_status: AccountStatus,
    /// The account differences between original and changed.
    info_diff: AccountInfoDiff,
    /// The latest account storage.
    storage: HashMap<U256, RevisedData<StorageSlot>>,
    /// The latest transition id at which the account was destroyed.
    selfdestruct_transition: Option<TransitionId>,
    /// Account transitions by block number.
    transitions: HashMap<BlockNumber, SharedAccountTransition>,
}

impl From<SharedCacheAccount> for RevmCacheAccount {
    fn from(value: SharedCacheAccount) -> Self {
        Self {
            account: value.previous_info.map(|info| PlainAccount {
                info,
                storage: value
                    .storage
                    .into_iter()
                    .map(|(slot, value)| (slot, value.present_value))
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
            selfdestruct_transition: None,
            transitions: HashMap::default(),
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
            storage: HashMap::default(),
            selfdestruct_transition: None,
            transitions: HashMap::default(),
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

    /// Return the status of account as it was loaded or after last transition applied
    pub fn previous_status(&self) -> AccountStatus {
        self.previous_status
    }

    /// Return storage slot value.
    pub fn storage_slot(&self, slot: U256) -> Option<U256> {
        self.storage.get(&slot).map(|value| value.present_value)
    }

    /// Insert storage slot.
    pub fn insert_storage_slot(&mut self, slot: U256, value: U256) {
        self.storage.insert(slot, RevisedData::new(StorageSlot::new(value)));
    }

    /// Returns current balance as it should be reported to EVM.
    pub fn account_balance(&self, delta: I256) -> U256 {
        let original = self.previous_info.as_ref().map(|info| info.balance).unwrap_or_default();
        if delta.is_negative() {
            original.checked_sub(U256::try_from(delta.abs()).unwrap()).unwrap()
        } else {
            original.checked_add(U256::try_from(delta).unwrap()).unwrap()
        }
    }

    /// Return current account nonce.
    pub fn account_nonce(&self, delta: i64) -> u64 {
        let original = self.previous_info.as_ref().map(|info| info.nonce).unwrap_or_default();
        if delta.is_negative() {
            original.checked_sub(u64::try_from(delta.abs()).unwrap()).unwrap()
        } else {
            original.checked_add(u64::try_from(delta).unwrap()).unwrap()
        }
    }

    /// Return latest account info.
    pub fn latest_account_info(&self) -> Option<AccountInfo> {
        self.account_info(&self.info_diff)
    }

    /// Fetch account info if it exist.
    pub fn account_info(&self, diff: &AccountInfoDiff) -> Option<AccountInfo> {
        let code = diff.code.data.clone().unwrap_or_default();
        let info = AccountInfo {
            balance: self.account_balance(diff.balance_delta),
            nonce: self.account_nonce(diff.nonce_delta),
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
    pub fn increment_balance(&mut self, block_number: BlockNumber, increment: u128) {
        if increment != 0 {
            let increment = I256::try_from(increment).unwrap();
            self.info_diff.balance_delta += increment;
            self.transitions.entry(block_number).or_default().info_diff.balance_delta += increment;
        }
    }

    /// Drain balance from account and return drained amount and transition.
    ///
    /// Used for DAO hardfork transition.
    pub fn drain_balance(&mut self, block_number: BlockNumber) -> u128 {
        let drained = I256::try_from(self.account_balance(self.info_diff.balance_delta)).unwrap();
        self.info_diff.balance_delta -= drained;
        self.transitions.entry(block_number).or_default().info_diff.balance_delta -= drained;
        drained.try_into().unwrap()
    }

    /// Apply single account revision. Revisions are expected to come out of order.
    pub fn apply_account_transition(
        &mut self,
        previous_info: &Option<AccountInfo>,
        account: Account,
        transition: TransitionId,
    ) {
        // Untouched account is never changed.
        if !account.is_touched() {
            return
        }

        let block_transition = self.transitions.entry(transition.0).or_default();

        // If it is marked as selfdestructed inside revm we need to change destroy the state
        // unless later revisions have already been applied.
        if account.is_selfdestructed() {
            self.info_diff.update_diff(previous_info, None, transition, true);
            self.storage.retain(|_slot, value| value.revision > Some(transition));
            self.selfdestruct_transition = Some(match self.selfdestruct_transition {
                Some(prev) => prev.max(transition),
                None => transition,
            });

            block_transition.info_diff.update_diff(previous_info, None, transition, true);
            block_transition.selfdestruct_count += 1;
            block_transition.selfdestruct_transition =
                Some(match block_transition.selfdestruct_transition {
                    Some(prev) => prev.max(transition),
                    None => transition,
                });
            return
        }

        let is_created = account.is_created();
        self.info_diff.update_diff(
            previous_info,
            Some(account.info.clone()),
            transition,
            is_created,
        );
        block_transition.info_diff.update_diff(
            previous_info,
            Some(account.info),
            transition,
            is_created,
        );

        let is_destroyed =
            self.selfdestruct_transition.map_or(false, |revision| transition < revision);
        let is_destroyed_at_block = block_transition
            .selfdestruct_transition
            .map_or(false, |revision| transition < revision);
        for (slot, value) in account.storage {
            // The value might have been destroyed in the further revision.
            let current_value = if is_destroyed { U256::ZERO } else { value.present_value };
            let block_value = if is_destroyed_at_block { U256::ZERO } else { value.present_value };
            match self.storage.entry(slot) {
                hash_map::Entry::Vacant(entry) => {
                    entry.insert(
                        RevisedData::new(StorageSlot::new_changed(
                            value.original_value(),
                            current_value,
                        ))
                        .with_revision(transition),
                    );
                }
                hash_map::Entry::Occupied(mut entry) => {
                    let existing = entry.get_mut();
                    existing.update_with_revision(
                        StorageSlot::new_changed(existing.original_value(), current_value),
                        transition,
                    );
                }
            };
            match block_transition.storage.entry(slot) {
                hash_map::Entry::Vacant(entry) => {
                    entry.insert(
                        RevisedData::new(StorageSlot::new_changed(
                            value.original_value(),
                            block_value,
                        ))
                        .with_revision(transition),
                    );
                }
                hash_map::Entry::Occupied(mut entry) => {
                    let existing = entry.get_mut();
                    existing.update_with_revision(
                        StorageSlot::new_changed(existing.original_value(), block_value),
                        transition,
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
        transition: &SharedAccountTransition,
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
        } else if transition.selfdestruct_transition.is_some() {
            if current_info.is_some() {
                AccountStatus::DestroyedChanged
            } else if self.previous_status.was_destroyed() || transition.selfdestruct_count > 1 {
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
    pub fn finalize_transition(
        &mut self,
        block_number: BlockNumber,
        state_clear_enabled: bool,
    ) -> Option<TransitionAccount> {
        let transition = self.transitions.remove(&block_number)?;

        let mut transition_storage = HashMap::<U256, StorageSlot>::default();
        for (slot, value) in &transition.storage {
            if value.is_changed() {
                transition_storage.insert(*slot, value.data.clone());
            }
            self.storage.get_mut(slot).unwrap().data.previous_or_original_value =
                value.present_value;
        }

        let mut info = self.account_info(&transition.info_diff);

        // The account code might have been read before update.
        // Since the code might have been written in any adjacent transition,
        // preserve it if it was not modified at this transition.
        if let Some(previous_info) = self
            .previous_info
            .as_ref()
            .filter(|info| info.code_hash != KECCAK_EMPTY && transition.info_diff.code.is_none())
        {
            info = Some(AccountInfo {
                balance: info.as_ref().map(|info| info.balance).unwrap_or_default(),
                nonce: info.as_ref().map(|info| info.nonce).unwrap_or_default(),
                code_hash: previous_info.code_hash,
                code: previous_info.code.clone(),
            })
        }

        let next_status = self.next_status(
            &info,
            &transition,
            !transition_storage.is_empty(),
            state_clear_enabled,
        );

        let transition_account = if next_status != self.previous_status ||
            info != self.previous_info ||
            !transition_storage.is_empty()
        {
            Some(TransitionAccount {
                info: info.clone(),
                status: next_status,
                previous_info: self.previous_info.clone(),
                previous_status: self.previous_status,
                storage: transition_storage,
                storage_was_destroyed: transition.selfdestruct_transition.is_some(),
            })
        } else {
            None
        };

        self.previous_info = info;
        self.previous_status = next_status;
        self.info_diff.balance_delta -= transition.info_diff.balance_delta;
        self.info_diff.nonce_delta -= transition.info_diff.nonce_delta;
        if self
            .selfdestruct_transition
            .map(|transition| transition.0 <= block_number)
            .unwrap_or_default()
        {
            self.selfdestruct_transition = None;
        }

        transition_account
    }
}
