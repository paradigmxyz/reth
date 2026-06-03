//! State types used by block execution outputs.

use alloc::{
    boxed::Box,
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
    vec::Vec,
};
use alloy_primitives::{
    map::{AddressMap, AddressSet, B256Map, HashMap},
    Address, StorageValue, B256, KECCAK256_EMPTY, U256,
};
use core::{
    cmp::Ordering,
    mem,
    ops::{Deref, DerefMut, RangeInclusive},
};

pub use revm::state::{
    bal::{AccountBal, AccountInfoBal, Bal, BalError, BalWrites, BlockAccessIndex, StorageBal},
    Account, AccountInfo, AccountStatus as EvmAccountStatus, Bytecode, EvmState, EvmStorageSlot,
    TransactionId,
};
pub use revm_database_interface::{
    bal::{BalState, EvmDatabaseError},
    DBErrorMarker, Database, DatabaseCommit, DatabaseRef, EmptyDB, EmptyDBTyped, OnStateHook,
    WrapDatabaseRef,
};

type StorageKey = U256;
type StorageKeyMap<T> = HashMap<StorageKey, T>;
const BLOCK_HASH_HISTORY: u64 = 256;

/// This builder is used to initialize a [`BundleState`].
#[derive(Debug)]
pub struct BundleBuilder {
    states: AddressSet,
    state_original: AddressMap<AccountInfo>,
    state_present: AddressMap<AccountInfo>,
    state_storage: AddressMap<StorageKeyMap<(StorageValue, StorageValue)>>,
    reverts: BTreeSet<(u64, Address)>,
    revert_range: RangeInclusive<u64>,
    revert_account: HashMap<(u64, Address), Option<Option<AccountInfo>>>,
    revert_storage: HashMap<(u64, Address), Vec<(StorageKey, StorageValue)>>,
    contracts: B256Map<Bytecode>,
}

impl Default for BundleBuilder {
    fn default() -> Self {
        Self {
            states: AddressSet::default(),
            state_original: AddressMap::default(),
            state_present: AddressMap::default(),
            state_storage: AddressMap::default(),
            reverts: BTreeSet::new(),
            revert_range: 0..=0,
            revert_account: HashMap::default(),
            revert_storage: HashMap::default(),
            contracts: B256Map::default(),
        }
    }
}

impl BundleBuilder {
    /// Creates a new builder.
    pub fn new(revert_range: RangeInclusive<u64>) -> Self {
        Self { revert_range, ..Default::default() }
    }

    /// Applies a transformation to the builder.
    pub fn apply<F>(self, f: F) -> Self
    where
        F: FnOnce(Self) -> Self,
    {
        f(self)
    }

    /// Applies a mutable transformation to the builder.
    pub fn apply_mut<F>(&mut self, f: F) -> &mut Self
    where
        F: FnOnce(&mut Self),
    {
        f(self);
        self
    }

    /// Collects a state address.
    pub fn state_address(mut self, address: Address) -> Self {
        self.set_state_address(address);
        self
    }

    /// Collects original account info.
    pub fn state_original_account_info(mut self, address: Address, original: AccountInfo) -> Self {
        self.set_state_original_account_info(address, original);
        self
    }

    /// Collects present account info.
    pub fn state_present_account_info(mut self, address: Address, present: AccountInfo) -> Self {
        self.set_state_present_account_info(address, present);
        self
    }

    /// Collects storage info.
    pub fn state_storage(
        mut self,
        address: Address,
        storage: StorageKeyMap<(StorageValue, StorageValue)>,
    ) -> Self {
        self.set_state_storage(address, storage);
        self
    }

    /// Collects a revert address.
    pub fn revert_address(mut self, block_number: u64, address: Address) -> Self {
        self.set_revert_address(block_number, address);
        self
    }

    /// Collects account revert info.
    pub fn revert_account_info(
        mut self,
        block_number: u64,
        address: Address,
        account: Option<Option<AccountInfo>>,
    ) -> Self {
        self.set_revert_account_info(block_number, address, account);
        self
    }

    /// Collects storage revert info.
    pub fn revert_storage(
        mut self,
        block_number: u64,
        address: Address,
        storage: Vec<(StorageKey, StorageValue)>,
    ) -> Self {
        self.set_revert_storage(block_number, address, storage);
        self
    }

    /// Collects bytecode.
    pub fn contract(mut self, hash: B256, bytecode: Bytecode) -> Self {
        self.set_contract(hash, bytecode);
        self
    }

    /// Sets a state address.
    pub fn set_state_address(&mut self, address: Address) -> &mut Self {
        self.states.insert(address);
        self
    }

    /// Sets original account info.
    pub fn set_state_original_account_info(
        &mut self,
        address: Address,
        original: AccountInfo,
    ) -> &mut Self {
        self.states.insert(address);
        self.state_original.insert(address, original);
        self
    }

    /// Sets present account info.
    pub fn set_state_present_account_info(
        &mut self,
        address: Address,
        present: AccountInfo,
    ) -> &mut Self {
        self.states.insert(address);
        self.state_present.insert(address, present);
        self
    }

    /// Sets state storage.
    pub fn set_state_storage(
        &mut self,
        address: Address,
        storage: StorageKeyMap<(StorageValue, StorageValue)>,
    ) -> &mut Self {
        self.states.insert(address);
        self.state_storage.insert(address, storage);
        self
    }

    /// Sets a revert address.
    pub fn set_revert_address(&mut self, block_number: u64, address: Address) -> &mut Self {
        self.reverts.insert((block_number, address));
        self
    }

    /// Sets account revert info.
    pub fn set_revert_account_info(
        &mut self,
        block_number: u64,
        address: Address,
        account: Option<Option<AccountInfo>>,
    ) -> &mut Self {
        self.reverts.insert((block_number, address));
        self.revert_account.insert((block_number, address), account);
        self
    }

    /// Sets storage revert info.
    pub fn set_revert_storage(
        &mut self,
        block_number: u64,
        address: Address,
        storage: Vec<(StorageKey, StorageValue)>,
    ) -> &mut Self {
        self.reverts.insert((block_number, address));
        self.revert_storage.insert((block_number, address), storage);
        self
    }

    /// Sets bytecode.
    pub fn set_contract(&mut self, hash: B256, bytecode: Bytecode) -> &mut Self {
        self.contracts.insert(hash, bytecode);
        self
    }

    /// Creates a [`BundleState`] from collected data.
    pub fn build(mut self) -> BundleState {
        let mut state_size = 0;
        let state = self
            .states
            .into_iter()
            .map(|address| {
                let storage = self
                    .state_storage
                    .remove(&address)
                    .map(|storage| {
                        storage
                            .into_iter()
                            .map(|(key, (original, present))| {
                                (key, StorageSlot::new_changed(original, present))
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                let account = BundleAccount::new(
                    self.state_original.remove(&address),
                    self.state_present.remove(&address),
                    storage,
                    AccountStatus::Changed,
                );
                state_size += account.size_hint();
                (address, account)
            })
            .collect();

        let mut reverts_size = 0;
        let mut reverts = BTreeMap::new();
        for block_number in self.revert_range {
            reverts.insert(block_number, Vec::new());
        }
        for (block_number, address) in self.reverts {
            let account = match self.revert_account.remove(&(block_number, address)).unwrap_or(None)
            {
                Some(Some(account)) => AccountInfoRevert::RevertTo(account),
                Some(None) => AccountInfoRevert::DeleteIt,
                None => AccountInfoRevert::DoNothing,
            };
            let storage = self
                .revert_storage
                .remove(&(block_number, address))
                .map(|storage| {
                    storage
                        .into_iter()
                        .map(|(key, value)| (key, RevertToSlot::Some(value)))
                        .collect()
                })
                .unwrap_or_default();
            let account_revert = AccountRevert {
                account,
                storage,
                previous_status: AccountStatus::Changed,
                wipe_storage: false,
            };

            if let Some(block_reverts) = reverts.get_mut(&block_number) {
                reverts_size += account_revert.size_hint();
                block_reverts.push((address, account_revert));
            }
        }

        BundleState {
            state,
            contracts: self.contracts,
            reverts: Reverts::new(reverts.into_values().collect()),
            state_size,
            reverts_size,
        }
    }
}

/// Option for [`BundleState`] when converting it to plain state.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum OriginalValuesKnown {
    /// Original values are known and can be checked.
    Yes,
    /// Original values are not guaranteed.
    No,
}

impl OriginalValuesKnown {
    /// Returns true if original values are not known.
    pub const fn is_not_known(&self) -> bool {
        matches!(self, Self::No)
    }
}

/// Bundle retention policy for applying substate to the bundle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BundleRetention {
    /// Only plain state is updated.
    PlainState,
    /// Plain state and reverts are retained.
    Reverts,
}

impl BundleRetention {
    /// Returns true if reverts should be retained.
    pub const fn includes_reverts(&self) -> bool {
        matches!(self, Self::Reverts)
    }
}

/// State of accounts in transition between transaction executions.
#[derive(Clone, Default, Debug, PartialEq, Eq)]
pub struct TransitionState {
    /// Account transitions.
    pub transitions: AddressMap<TransitionAccount>,
}

impl TransitionState {
    /// Creates a transition state containing one account.
    pub fn single(address: Address, transition: TransitionAccount) -> Self {
        let mut transitions = HashMap::default();
        transitions.insert(address, transition);
        Self { transitions }
    }

    /// Takes all transitions and leaves this state empty.
    pub fn take(&mut self) -> Self {
        mem::take(self)
    }

    /// Clears the transition state.
    pub fn clear(&mut self) {
        self.transitions.clear();
    }

    /// Adds account transitions, merging duplicate addresses.
    pub fn add_transitions(
        &mut self,
        transitions: impl IntoIterator<Item = (Address, TransitionAccount)>,
    ) {
        let transitions = transitions.into_iter();
        if let Some(upper) = transitions.size_hint().1 {
            self.transitions.reserve(upper);
        }
        for (address, account) in transitions {
            match self.transitions.entry(address) {
                alloy_primitives::map::Entry::Occupied(entry) => entry.into_mut().update(account),
                alloy_primitives::map::Entry::Vacant(entry) => _ = entry.insert(account),
            }
        }
    }
}

/// Account transition accumulated between bundle merges.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct TransitionAccount {
    /// Account information if the account exists.
    pub info: Option<AccountInfo>,
    /// Current account status.
    pub status: AccountStatus,
    /// Previous account info.
    pub previous_info: Option<AccountInfo>,
    /// Previous account status.
    pub previous_status: AccountStatus,
    /// Storage with original and present values.
    pub storage: StorageWithOriginalValues,
    /// Whether storage was cleared during this transition.
    pub storage_was_destroyed: bool,
}

impl TransitionAccount {
    /// Creates a new empty EIP-161 account transition.
    pub fn new_empty_eip161(storage: StorageWithOriginalValues) -> Self {
        Self {
            info: Some(AccountInfo::default()),
            status: AccountStatus::InMemoryChange,
            previous_info: None,
            previous_status: AccountStatus::LoadedNotExisting,
            storage,
            storage_was_destroyed: false,
        }
    }

    /// Returns new contract bytecode if account code changed.
    pub fn has_new_contract(&self) -> Option<(B256, &Bytecode)> {
        let present_hash = self.info.as_ref().map(|info| &info.code_hash);
        let previous_hash = self.previous_info.as_ref().map(|info| &info.code_hash);
        if present_hash != previous_hash {
            return self
                .info
                .as_ref()
                .and_then(|info| info.code.as_ref().map(|bytecode| (info.code_hash, bytecode)));
        }
        None
    }

    /// Returns the balance before this transition.
    pub fn previous_balance(&self) -> U256 {
        self.previous_info.as_ref().map(|info| info.balance).unwrap_or_default()
    }

    /// Returns the balance after this transition.
    pub fn current_balance(&self) -> U256 {
        self.info.as_ref().map(|info| info.balance).unwrap_or_default()
    }

    /// Updates this transition with newer values while preserving original values.
    pub fn update(&mut self, other: Self) {
        self.info = other.info;
        self.status = other.status;

        if matches!(other.status, AccountStatus::Destroyed | AccountStatus::DestroyedAgain) {
            self.storage = other.storage;
            self.storage_was_destroyed = true;
        } else {
            for (key, slot) in other.storage {
                match self.storage.entry(key) {
                    alloy_primitives::map::Entry::Vacant(entry) => {
                        entry.insert(slot);
                    }
                    alloy_primitives::map::Entry::Occupied(mut entry) => {
                        let value = entry.get_mut();
                        if value.original_value() == slot.present_value() {
                            entry.remove();
                        } else {
                            value.present_value = slot.present_value;
                        }
                    }
                }
            }
        }
    }

    /// Consumes this transition and creates the revert needed to undo it.
    pub fn create_revert(self) -> Option<AccountRevert> {
        let mut previous_account = self.original_bundle_account();
        previous_account.update_and_create_revert(self)
    }

    /// Returns the present bundle account.
    pub fn present_bundle_account(&self) -> BundleAccount {
        BundleAccount {
            info: self.info.clone(),
            original_info: self.previous_info.clone(),
            storage: self.storage.clone(),
            status: self.status,
        }
    }

    fn original_bundle_account(&self) -> BundleAccount {
        BundleAccount {
            info: self.previous_info.clone(),
            original_info: self.previous_info.clone(),
            storage: StorageWithOriginalValues::default(),
            status: self.previous_status,
        }
    }
}

/// Bundle state containing only changed values.
#[derive(Default, Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct BundleState {
    /// Account state.
    pub state: AddressMap<BundleAccount>,
    /// Created or changed contracts keyed by code hash.
    pub contracts: B256Map<Bytecode>,
    /// Changes needed to revert transitions.
    pub reverts: Reverts,
    /// Approximate size of the plain state.
    pub state_size: usize,
    /// Approximate size of the reverts.
    pub reverts_size: usize,
}

impl BundleState {
    /// Returns a builder.
    pub fn builder(revert_range: RangeInclusive<u64>) -> BundleBuilder {
        BundleBuilder::new(revert_range)
    }

    /// Creates a bundle from plain state, reverts, and contracts.
    pub fn new(
        state: impl IntoIterator<
            Item = (
                Address,
                Option<AccountInfo>,
                Option<AccountInfo>,
                HashMap<StorageKey, (StorageValue, StorageValue)>,
            ),
        >,
        reverts: impl IntoIterator<
            Item = impl IntoIterator<
                Item = (
                    Address,
                    Option<Option<AccountInfo>>,
                    impl IntoIterator<Item = (StorageKey, StorageValue)>,
                ),
            >,
        >,
        contracts: impl IntoIterator<Item = (B256, Bytecode)>,
    ) -> Self {
        let mut state_size = 0;
        let state = state
            .into_iter()
            .map(|(address, original, present, storage)| {
                let account = BundleAccount::new(
                    original,
                    present,
                    storage
                        .into_iter()
                        .map(|(key, (original, present))| {
                            (key, StorageSlot::new_changed(original, present))
                        })
                        .collect(),
                    AccountStatus::Changed,
                );
                state_size += account.size_hint();
                (address, account)
            })
            .collect();

        let mut reverts_size = 0;
        let reverts = reverts
            .into_iter()
            .map(|block_reverts| {
                block_reverts
                    .into_iter()
                    .map(|(address, account, storage)| {
                        let account = match account {
                            Some(Some(account)) => AccountInfoRevert::RevertTo(account),
                            Some(None) => AccountInfoRevert::DeleteIt,
                            None => AccountInfoRevert::DoNothing,
                        };
                        let revert = AccountRevert {
                            account,
                            storage: storage
                                .into_iter()
                                .map(|(key, value)| (key, RevertToSlot::Some(value)))
                                .collect(),
                            previous_status: AccountStatus::Changed,
                            wipe_storage: false,
                        };
                        reverts_size += revert.size_hint();
                        (address, revert)
                    })
                    .collect()
            })
            .collect();

        Self {
            state,
            contracts: contracts.into_iter().collect(),
            reverts: Reverts::new(reverts),
            state_size,
            reverts_size,
        }
    }

    /// Returns the approximate size of changes in this bundle.
    pub fn size_hint(&self) -> usize {
        self.state_size + self.reverts_size + self.contracts.len()
    }

    /// Returns the account state map.
    pub const fn state(&self) -> &AddressMap<BundleAccount> {
        &self.state
    }

    /// Returns true if the bundle is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Returns the number of changed accounts.
    pub fn len(&self) -> usize {
        self.state.len()
    }

    /// Gets an account from the bundle.
    pub fn account(&self, address: &Address) -> Option<&BundleAccount> {
        self.state.get(address)
    }

    /// Gets bytecode from the bundle.
    pub fn bytecode(&self, hash: &B256) -> Option<Bytecode> {
        self.contracts.get(hash).cloned()
    }

    /// Gets storage value from the bundle.
    pub fn storage(&self, address: &Address, storage_key: StorageKey) -> Option<StorageValue> {
        self.account(address).and_then(|account| account.storage_slot(storage_key))
    }

    /// Applies transitions to the bundle and records reverts.
    pub fn apply_transitions_and_create_reverts(
        &mut self,
        transitions: TransitionState,
        retention: BundleRetention,
    ) {
        let include_reverts = retention.includes_reverts();
        let mut reverts =
            Vec::with_capacity(if include_reverts { transitions.transitions.len() } else { 0 });

        self.state.reserve(transitions.transitions.len());
        for (address, transition) in transitions.transitions {
            if let Some((hash, bytecode)) = transition.has_new_contract() {
                self.contracts.insert(hash, bytecode.clone());
            }

            let revert = match self.state.entry(address) {
                alloy_primitives::map::Entry::Occupied(mut entry) => {
                    let account = entry.get_mut();
                    self.state_size -= account.size_hint();
                    let revert = account.update_and_create_revert(transition);
                    self.state_size += account.size_hint();
                    revert
                }
                alloy_primitives::map::Entry::Vacant(entry) => {
                    let present = transition.present_bundle_account();
                    let revert = transition.create_revert();
                    if revert.is_some() {
                        self.state_size += present.size_hint();
                        entry.insert(present);
                    }
                    revert
                }
            };

            if let Some(revert) = revert.filter(|_| include_reverts) {
                self.reverts_size += revert.size_hint();
                reverts.push((address, revert));
            }
        }

        self.reverts.push(reverts);
    }

    /// Generates a plain state changeset without consuming the bundle.
    pub fn to_plain_state(&self, is_value_known: OriginalValuesKnown) -> StateChangeset {
        let mut accounts = Vec::with_capacity(self.state.len());
        let mut storage = Vec::with_capacity(self.state.len());

        for (address, account) in &self.state {
            let was_destroyed = account.was_destroyed();
            if is_value_known.is_not_known() || account.is_info_changed() {
                accounts
                    .push((*address, account.info.as_ref().map(AccountInfo::copy_without_code)));
            }

            let mut storage_changes = Vec::with_capacity(account.storage.len());
            for (&key, &slot) in &account.storage {
                let destroyed_and_not_zero = was_destroyed && !slot.present_value.is_zero();
                let changed = !was_destroyed && slot.is_changed();
                if is_value_known.is_not_known() || destroyed_and_not_zero || changed {
                    storage_changes.push((key, slot.present_value));
                }
            }

            if !storage_changes.is_empty() || was_destroyed {
                storage.push(PlainStorageChangeset {
                    address: *address,
                    wipe_storage: was_destroyed,
                    storage: storage_changes,
                });
            }
        }

        let contracts = self
            .contracts
            .iter()
            .filter(|(hash, _)| **hash != KECCAK256_EMPTY)
            .map(|(hash, bytecode)| (*hash, bytecode.clone()))
            .collect();

        StateChangeset { accounts, storage, contracts }
    }

    /// Generates a plain state changeset and plain reverts.
    pub fn to_plain_state_and_reverts(
        &self,
        is_value_known: OriginalValuesKnown,
    ) -> (StateChangeset, PlainStateReverts) {
        (self.to_plain_state(is_value_known), self.reverts.to_plain_state_reverts())
    }

    /// Consumes the bundle and returns plain state plus plain reverts.
    #[deprecated = "Use `to_plain_state_and_reverts` instead"]
    pub fn into_plain_state_and_reverts(
        self,
        is_value_known: OriginalValuesKnown,
    ) -> (StateChangeset, PlainStateReverts) {
        self.to_plain_state_and_reverts(is_value_known)
    }

    /// Extends the state map with another state map.
    pub fn extend_state(&mut self, other_state: AddressMap<BundleAccount>) {
        self.state.reserve(other_state.len());
        for (address, other_account) in other_state {
            match self.state.entry(address) {
                alloy_primitives::map::Entry::Occupied(mut entry) => {
                    let this = entry.get_mut();
                    self.state_size -= this.size_hint();
                    if other_account.was_destroyed() {
                        this.storage = other_account.storage;
                    } else {
                        this.storage.reserve(other_account.storage.len());
                        for (key, storage_slot) in other_account.storage {
                            this.storage.entry(key).or_insert(storage_slot).present_value =
                                storage_slot.present_value;
                        }
                    }
                    this.info = other_account.info;
                    this.status.transition(other_account.status);
                    self.state_size += this.size_hint();
                }
                alloy_primitives::map::Entry::Vacant(entry) => {
                    self.state_size += other_account.size_hint();
                    entry.insert(other_account);
                }
            }
        }
    }

    /// Extends this bundle with changes built on top of it.
    pub fn extend(&mut self, mut other: Self) {
        for (address, revert) in other.reverts.iter_mut().flatten() {
            if revert.wipe_storage &&
                let Some(this_account) = self.state.get_mut(address)
            {
                for (key, value) in this_account.storage.drain() {
                    revert.storage.entry(key).or_insert(RevertToSlot::Some(value.present_value));
                }

                if this_account.was_destroyed() {
                    revert.wipe_storage = false;
                }
            }

            self.reverts_size += revert.size_hint();
        }

        self.extend_state(other.state);
        self.contracts.extend(other.contracts);
        self.reverts.extend(other.reverts);
    }

    /// Takes first N raw reverts from the bundle.
    pub fn take_n_reverts(&mut self, reverts_to_take: usize) -> Reverts {
        if reverts_to_take > self.reverts.len() {
            return self.take_all_reverts()
        }
        let (detached, remaining) = self.reverts.split_at(reverts_to_take);
        let detached = Reverts::new(detached.to_vec());
        self.reverts_size = remaining.iter().flatten().map(|(_, revert)| revert.size_hint()).sum();
        self.reverts = Reverts::new(remaining.to_vec());
        detached
    }

    /// Returns and clears all reverts from the bundle.
    pub fn take_all_reverts(&mut self) -> Reverts {
        self.reverts_size = 0;
        mem::take(&mut self.reverts)
    }

    /// Reverts the latest transition.
    pub fn revert_latest(&mut self) -> bool {
        let Some(reverts) = self.reverts.pop() else { return false };
        for (address, revert_account) in reverts {
            self.reverts_size -= revert_account.size_hint();
            match self.state.entry(address) {
                alloy_primitives::map::Entry::Occupied(mut entry) => {
                    let account = entry.get_mut();
                    self.state_size -= account.size_hint();
                    if account.revert(revert_account) {
                        entry.remove();
                    } else {
                        self.state_size += account.size_hint();
                    }
                }
                alloy_primitives::map::Entry::Vacant(entry) => {
                    let mut account = BundleAccount::new(
                        None,
                        None,
                        HashMap::default(),
                        AccountStatus::LoadedNotExisting,
                    );
                    if !account.revert(revert_account) {
                        self.state_size += account.size_hint();
                        entry.insert(account);
                    }
                }
            }
        }
        true
    }

    /// Reverts N transitions.
    pub fn revert(&mut self, mut num_transitions: usize) {
        if num_transitions == 0 {
            return
        }
        while self.revert_latest() {
            num_transitions -= 1;
            if num_transitions == 0 {
                break
            }
        }
    }

    /// Prepends present state with another bundle state.
    pub fn prepend_state(&mut self, mut other: Self) {
        let this_bundle = mem::take(self);
        other.extend_state(this_bundle.state);
        other.contracts.extend(this_bundle.contracts);
        mem::swap(self, &mut other);
    }
}

/// Account information focused on database changesets and reverts.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct BundleAccount {
    /// Current account information.
    pub info: Option<AccountInfo>,
    /// Original account information.
    pub original_info: Option<AccountInfo>,
    /// Storage slots with original and present values.
    pub storage: StorageWithOriginalValues,
    /// Account status.
    pub status: AccountStatus,
}

impl BundleAccount {
    /// Creates a new bundle account.
    pub const fn new(
        original_info: Option<AccountInfo>,
        present_info: Option<AccountInfo>,
        storage: StorageWithOriginalValues,
        status: AccountStatus,
    ) -> Self {
        Self { info: present_info, original_info, storage, status }
    }

    /// Returns the approximate size of this account.
    pub fn size_hint(&self) -> usize {
        1 + self.storage.len()
    }

    /// Returns storage slot value if known.
    pub fn storage_slot(&self, slot: StorageKey) -> Option<StorageValue> {
        self.storage
            .get(&slot)
            .map(|slot| slot.present_value)
            .or_else(|| self.status.is_storage_known().then_some(StorageValue::ZERO))
    }

    /// Returns cloned account info.
    pub fn account_info(&self) -> Option<AccountInfo> {
        self.info.clone()
    }

    /// Returns true if this account was destroyed.
    pub const fn was_destroyed(&self) -> bool {
        self.status.was_destroyed()
    }

    /// Returns true if account info changed.
    pub fn is_info_changed(&self) -> bool {
        self.info != self.original_info
    }

    /// Returns true if contract code changed.
    pub fn is_contract_changed(&self) -> bool {
        self.info.as_ref().map(|info| info.code_hash) !=
            self.original_info.as_ref().map(|info| info.code_hash)
    }

    /// Updates this account to a transition and returns the revert needed to undo it.
    pub fn update_and_create_revert(
        &mut self,
        transition: TransitionAccount,
    ) -> Option<AccountRevert> {
        let updated_info = transition.info;
        let updated_storage = transition.storage;
        let updated_status = transition.status;

        let extend_storage = |storage: &mut StorageWithOriginalValues,
                              update: StorageWithOriginalValues| {
            storage.reserve(update.len());
            for (key, value) in update {
                storage.entry(key).or_insert(value).present_value = value.present_value;
            }
        };

        let previous_storage_from_update =
            |updated_storage: &StorageWithOriginalValues| -> StorageKeyMap<RevertToSlot> {
                updated_storage
                    .iter()
                    .filter(|(_, value)| value.is_changed())
                    .map(|(key, value)| {
                        (*key, RevertToSlot::Some(value.previous_or_original_value))
                    })
                    .collect()
            };

        let info_revert = if self.info == updated_info {
            AccountInfoRevert::DoNothing
        } else {
            AccountInfoRevert::RevertTo(self.info.clone().unwrap_or_default())
        };

        let account_revert = match updated_status {
            AccountStatus::Changed => {
                let previous_storage = previous_storage_from_update(&updated_storage);
                match self.status {
                    AccountStatus::Changed | AccountStatus::Loaded => {
                        extend_storage(&mut self.storage, updated_storage);
                    }
                    AccountStatus::LoadedEmptyEIP161 => {}
                    _ => unreachable!("invalid state transfer to changed from {self:?}"),
                }
                let previous_status = self.status;
                self.status = AccountStatus::Changed;
                self.info = updated_info;
                Some(AccountRevert {
                    account: info_revert,
                    storage: previous_storage,
                    previous_status,
                    wipe_storage: false,
                })
            }
            AccountStatus::InMemoryChange => {
                let previous_storage = previous_storage_from_update(&updated_storage);
                let account = match self.status {
                    AccountStatus::Loaded | AccountStatus::InMemoryChange => {
                        extend_storage(&mut self.storage, updated_storage);
                        info_revert
                    }
                    AccountStatus::LoadedEmptyEIP161 => {
                        self.storage = updated_storage;
                        info_revert
                    }
                    AccountStatus::LoadedNotExisting => {
                        self.storage = updated_storage;
                        AccountInfoRevert::DeleteIt
                    }
                    _ => unreachable!("invalid state transfer to in-memory change from {self:?}"),
                };
                let previous_status = self.status;
                self.status = AccountStatus::InMemoryChange;
                self.info = updated_info;
                Some(AccountRevert {
                    account,
                    storage: previous_storage,
                    previous_status,
                    wipe_storage: false,
                })
            }
            AccountStatus::Loaded |
            AccountStatus::LoadedNotExisting |
            AccountStatus::LoadedEmptyEIP161 => None,
            AccountStatus::Destroyed => {
                let storage = mem::take(&mut self.storage);
                let revert = match self.status {
                    AccountStatus::InMemoryChange |
                    AccountStatus::Changed |
                    AccountStatus::Loaded |
                    AccountStatus::LoadedEmptyEIP161 => {
                        Some(AccountRevert::new_selfdestructed(self.status, info_revert, storage))
                    }
                    AccountStatus::LoadedNotExisting => None,
                    _ => unreachable!("invalid transition to destroyed from {self:?}"),
                };

                if revert.is_some() {
                    self.status = AccountStatus::Destroyed;
                    self.info = None;
                }

                revert
            }
            AccountStatus::DestroyedChanged => {
                if let Some(revert) = AccountRevert::new_selfdestructed_from_bundle(
                    info_revert.clone(),
                    self,
                    &updated_storage,
                ) {
                    self.status = AccountStatus::DestroyedChanged;
                    self.info = updated_info;
                    self.storage = updated_storage;
                    Some(revert)
                } else {
                    let revert = match self.status {
                        AccountStatus::Destroyed | AccountStatus::LoadedNotExisting => {
                            Some(AccountRevert {
                                account: AccountInfoRevert::DeleteIt,
                                storage: previous_storage_from_update(&updated_storage),
                                previous_status: self.status,
                                wipe_storage: false,
                            })
                        }
                        AccountStatus::DestroyedChanged => {
                            let previous_storage = if transition.storage_was_destroyed {
                                let mut storage = mem::take(&mut self.storage)
                                    .into_iter()
                                    .map(|(key, value)| {
                                        (key, RevertToSlot::Some(value.present_value))
                                    })
                                    .collect::<StorageKeyMap<_>>();
                                for key in updated_storage.keys() {
                                    storage.entry(*key).or_insert(RevertToSlot::Destroyed);
                                }
                                storage
                            } else {
                                previous_storage_from_update(&updated_storage)
                            };

                            Some(AccountRevert {
                                account: info_revert,
                                storage: previous_storage,
                                previous_status: AccountStatus::DestroyedChanged,
                                wipe_storage: false,
                            })
                        }
                        AccountStatus::DestroyedAgain => {
                            Some(AccountRevert::new_selfdestructed_again(
                                AccountStatus::DestroyedAgain,
                                AccountInfoRevert::DeleteIt,
                                HashMap::default(),
                                updated_storage.clone(),
                            ))
                        }
                        _ => unreachable!("invalid transition to destroyed changed from {self:?}"),
                    };
                    self.status = AccountStatus::DestroyedChanged;
                    self.info = updated_info;
                    extend_storage(&mut self.storage, updated_storage);
                    revert
                }
            }
            AccountStatus::DestroyedAgain => {
                let revert = if let Some(revert) = AccountRevert::new_selfdestructed_from_bundle(
                    info_revert,
                    self,
                    &HashMap::default(),
                ) {
                    Some(revert)
                } else {
                    match self.status {
                        AccountStatus::Destroyed |
                        AccountStatus::DestroyedAgain |
                        AccountStatus::LoadedNotExisting => None,
                        AccountStatus::DestroyedChanged => {
                            Some(AccountRevert::new_selfdestructed_again(
                                AccountStatus::DestroyedChanged,
                                AccountInfoRevert::RevertTo(self.info.clone().unwrap_or_default()),
                                mem::take(&mut self.storage),
                                HashMap::default(),
                            ))
                        }
                        _ => unreachable!("invalid transition to destroyed again from {self:?}"),
                    }
                };
                self.status = AccountStatus::DestroyedAgain;
                self.info = None;
                self.storage.clear();
                revert
            }
        };

        account_revert.and_then(|revert| (!revert.is_empty()).then_some(revert))
    }

    /// Reverts this account and returns true if it can be removed.
    pub fn revert(&mut self, revert: AccountRevert) -> bool {
        self.status = revert.previous_status;

        match revert.account {
            AccountInfoRevert::DoNothing => {}
            AccountInfoRevert::DeleteIt => {
                self.info = None;
                if self.original_info.is_none() {
                    self.storage = HashMap::default();
                    return true
                }
                for slot in self.storage.values_mut() {
                    slot.present_value = StorageValue::ZERO;
                }
                return false
            }
            AccountInfoRevert::RevertTo(info) => self.info = Some(info),
        }

        for (key, slot) in revert.storage {
            match slot {
                RevertToSlot::Some(value) => {
                    self.storage
                        .entry(key)
                        .or_insert_with(|| StorageSlot::new(value))
                        .present_value = value;
                }
                RevertToSlot::Destroyed => {
                    self.storage.remove(&key);
                }
            }
        }

        false
    }
}

/// Account status used by bundle state transitions.
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum AccountStatus {
    /// Account was loaded but does not exist.
    #[default]
    LoadedNotExisting,
    /// Account was loaded and exists.
    Loaded,
    /// Account was loaded and empty under EIP-161.
    LoadedEmptyEIP161,
    /// Account changed entirely in memory.
    InMemoryChange,
    /// Account was modified.
    Changed,
    /// Account was destroyed.
    Destroyed,
    /// Account was destroyed and then modified.
    DestroyedChanged,
    /// Account was destroyed again.
    DestroyedAgain,
}

impl AccountStatus {
    /// Returns true if account is not modified.
    pub const fn is_not_modified(&self) -> bool {
        matches!(self, Self::LoadedNotExisting | Self::Loaded | Self::LoadedEmptyEIP161)
    }

    /// Returns true if account was destroyed.
    pub const fn was_destroyed(&self) -> bool {
        matches!(self, Self::Destroyed | Self::DestroyedChanged | Self::DestroyedAgain)
    }

    /// Returns true if storage is known from memory.
    pub const fn is_storage_known(&self) -> bool {
        matches!(
            self,
            Self::LoadedNotExisting |
                Self::InMemoryChange |
                Self::Destroyed |
                Self::DestroyedChanged |
                Self::DestroyedAgain
        )
    }

    /// Returns true if account is modified but not destroyed.
    pub const fn is_modified_and_not_destroyed(&self) -> bool {
        matches!(self, Self::Changed | Self::InMemoryChange)
    }

    /// Returns the next account status on creation.
    pub const fn on_created(&self) -> Self {
        match self {
            Self::DestroyedAgain | Self::Destroyed | Self::DestroyedChanged => {
                Self::DestroyedChanged
            }
            Self::LoadedNotExisting |
            Self::LoadedEmptyEIP161 |
            Self::Loaded |
            Self::Changed |
            Self::InMemoryChange => Self::InMemoryChange,
        }
    }

    /// Returns the next account status after touching an empty account post EIP-161.
    pub const fn on_touched_empty_post_eip161(&self) -> Self {
        match self {
            Self::LoadedNotExisting => Self::LoadedNotExisting,
            Self::InMemoryChange |
            Self::Destroyed |
            Self::LoadedEmptyEIP161 |
            Self::Changed |
            Self::Loaded => Self::Destroyed,
            Self::DestroyedAgain | Self::DestroyedChanged => Self::DestroyedAgain,
        }
    }

    /// Returns the next account status after an account change.
    pub const fn on_changed(&self, had_no_nonce_and_code: bool) -> Self {
        match self {
            Self::LoadedNotExisting | Self::LoadedEmptyEIP161 => Self::InMemoryChange,
            Self::Loaded => {
                if had_no_nonce_and_code {
                    Self::InMemoryChange
                } else {
                    Self::Changed
                }
            }
            Self::Changed | Self::InMemoryChange => *self,
            Self::DestroyedChanged => Self::DestroyedChanged,
            Self::Destroyed | Self::DestroyedAgain => Self::DestroyedChanged,
        }
    }

    /// Returns the next account status after selfdestruct.
    pub const fn on_selfdestructed(&self) -> Self {
        match self {
            Self::LoadedNotExisting => Self::LoadedNotExisting,
            Self::DestroyedChanged | Self::DestroyedAgain | Self::Destroyed => Self::DestroyedAgain,
            _ => Self::Destroyed,
        }
    }

    /// Transitions this status while preserving destroyed/in-memory invariants.
    pub fn transition(&mut self, other: Self) {
        *self = match (self.was_destroyed(), other.was_destroyed()) {
            (true, false) => Self::DestroyedChanged,
            (false, false) if *self == Self::InMemoryChange => Self::InMemoryChange,
            _ => other,
        };
    }
}

/// Contains reverts for multiple transitions.
#[derive(Clone, Debug, Default, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Reverts(Vec<Vec<(Address, AccountRevert)>>);

impl Deref for Reverts {
    type Target = Vec<Vec<(Address, AccountRevert)>>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for Reverts {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl Reverts {
    /// Creates new reverts.
    pub const fn new(reverts: Vec<Vec<(Address, AccountRevert)>>) -> Self {
        Self(reverts)
    }

    /// Sorts accounts inside each transition by address.
    pub fn sort(&mut self) {
        for revert in &mut self.0 {
            revert.sort_by_key(|(address, _)| *address);
        }
    }

    /// Extends reverts with another set.
    pub fn extend(&mut self, other: Self) {
        self.0.extend(other.0);
    }

    /// Generates plain state reverts.
    pub fn to_plain_state_reverts(&self) -> PlainStateReverts {
        let mut state_reverts = PlainStateReverts::with_capacity(self.0.len());
        for reverts in &self.0 {
            let mut accounts = Vec::with_capacity(reverts.len());
            let mut storage = Vec::with_capacity(reverts.len());
            for (address, revert_account) in reverts {
                match &revert_account.account {
                    AccountInfoRevert::RevertTo(account) => {
                        accounts.push((*address, Some(account.clone())));
                    }
                    AccountInfoRevert::DeleteIt => accounts.push((*address, None)),
                    AccountInfoRevert::DoNothing => {}
                }
                if revert_account.wipe_storage || !revert_account.storage.is_empty() {
                    storage.push(PlainStorageRevert {
                        address: *address,
                        wiped: revert_account.wipe_storage,
                        storage_revert: revert_account
                            .storage
                            .iter()
                            .map(|(key, value)| (*key, *value))
                            .collect(),
                    });
                }
            }
            state_reverts.accounts.push(accounts);
            state_reverts.storage.push(storage);
        }
        state_reverts
    }

    /// Compares reverts while ignoring account order inside transitions.
    pub fn content_eq(&self, other: &Self) -> bool {
        if self.0.len() != other.0.len() {
            return false
        }

        self.0.iter().zip(&other.0).all(|(left, right)| {
            if left.len() != right.len() {
                return false
            }
            let mut left = left.clone();
            let mut right = right.clone();
            left.sort_by(|(left_address, left_revert), (right_address, right_revert)| {
                left_address.cmp(right_address).then_with(|| left_revert.cmp(right_revert))
            });
            right.sort_by(|(left_address, left_revert), (right_address, right_revert)| {
                left_address.cmp(right_address).then_with(|| left_revert.cmp(right_revert))
            });
            left == right
        })
    }

    /// Consumes reverts and creates plain state reverts.
    #[deprecated = "Use `to_plain_state_reverts` instead"]
    pub fn into_plain_state_reverts(self) -> PlainStateReverts {
        self.to_plain_state_reverts()
    }
}

impl PartialEq for Reverts {
    fn eq(&self, other: &Self) -> bool {
        self.content_eq(other)
    }
}

/// Revert data for one account.
#[derive(Clone, Default, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct AccountRevert {
    /// Account information revert.
    pub account: AccountInfoRevert,
    /// Storage slots to revert.
    pub storage: StorageKeyMap<RevertToSlot>,
    /// Previous account status.
    pub previous_status: AccountStatus,
    /// Whether storage should be wiped.
    pub wipe_storage: bool,
}

impl AccountRevert {
    /// Returns approximate size of this revert.
    pub fn size_hint(&self) -> usize {
        1 + self.storage.len()
    }

    /// Creates a revert for an account selfdestructed and recreated in-memory.
    pub fn new_selfdestructed_again(
        status: AccountStatus,
        account: AccountInfoRevert,
        previous_storage: StorageWithOriginalValues,
        updated_storage: StorageWithOriginalValues,
    ) -> Self {
        let mut storage = previous_storage
            .into_iter()
            .map(|(key, value)| (key, RevertToSlot::Some(value.present_value)))
            .collect::<StorageKeyMap<_>>();
        for key in updated_storage.keys() {
            storage.entry(*key).or_insert(RevertToSlot::Destroyed);
        }
        Self { account, storage, previous_status: status, wipe_storage: false }
    }

    /// Creates a selfdestruct revert from an existing bundle account.
    pub fn new_selfdestructed_from_bundle(
        account: AccountInfoRevert,
        bundle_account: &mut BundleAccount,
        updated_storage: &StorageWithOriginalValues,
    ) -> Option<Self> {
        match bundle_account.status {
            AccountStatus::InMemoryChange |
            AccountStatus::Changed |
            AccountStatus::LoadedEmptyEIP161 |
            AccountStatus::Loaded => {
                let mut revert = Self::new_selfdestructed_again(
                    bundle_account.status,
                    account,
                    bundle_account.storage.drain().collect(),
                    updated_storage.clone(),
                );
                revert.wipe_storage = true;
                Some(revert)
            }
            _ => None,
        }
    }

    /// Creates a new selfdestruct revert.
    pub fn new_selfdestructed(
        status: AccountStatus,
        account: AccountInfoRevert,
        storage: StorageWithOriginalValues,
    ) -> Self {
        let storage = storage
            .into_iter()
            .map(|(key, value)| (key, RevertToSlot::Some(value.present_value)))
            .collect();

        Self { account, storage, previous_status: status, wipe_storage: true }
    }

    /// Returns true if this revert does nothing.
    pub fn is_empty(&self) -> bool {
        self.account == AccountInfoRevert::DoNothing &&
            self.storage.is_empty() &&
            !self.wipe_storage
    }
}

impl PartialOrd for AccountRevert {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for AccountRevert {
    fn cmp(&self, other: &Self) -> Ordering {
        self.account
            .cmp(&other.account)
            .then_with(|| sorted_storage_cmp(&self.storage, &other.storage))
            .then_with(|| self.previous_status.cmp(&other.previous_status))
            .then_with(|| self.wipe_storage.cmp(&other.wipe_storage))
    }
}

/// Account info revert action.
#[derive(Clone, Default, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum AccountInfoRevert {
    /// Nothing changed.
    #[default]
    DoNothing,
    /// Delete account on revert.
    DeleteIt,
    /// Revert to previous account info.
    RevertTo(AccountInfo),
}

/// Storage revert target.
#[derive(Clone, Debug, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum RevertToSlot {
    /// Revert to this value.
    Some(StorageValue),
    /// Storage was destroyed.
    Destroyed,
}

impl RevertToSlot {
    /// Returns the previous value to set on revert.
    pub const fn to_previous_value(self) -> StorageValue {
        match self {
            Self::Some(value) => value,
            Self::Destroyed => StorageValue::ZERO,
        }
    }
}

/// Plain state changes for database inclusion.
#[derive(Clone, Debug, Default)]
pub struct StateChangeset {
    /// Unsorted account changes.
    pub accounts: Vec<(Address, Option<AccountInfo>)>,
    /// Unsorted storage changes.
    pub storage: Vec<PlainStorageChangeset>,
    /// Unsorted contracts by bytecode hash.
    pub contracts: Vec<(B256, Bytecode)>,
}

/// Plain storage changeset.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct PlainStorageChangeset {
    /// Account address.
    pub address: Address,
    /// Whether to wipe storage.
    pub wipe_storage: bool,
    /// Storage key-value pairs.
    pub storage: Vec<(StorageKey, StorageValue)>,
}

/// Plain storage revert.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct PlainStorageRevert {
    /// Account address.
    pub address: Address,
    /// Whether storage is wiped.
    pub wiped: bool,
    /// Storage key-value pairs to revert.
    pub storage_revert: Vec<(StorageKey, RevertToSlot)>,
}

/// Plain state reverts for database inclusion.
#[derive(Clone, Debug, Default)]
pub struct PlainStateReverts {
    /// Account reverts per transition.
    pub accounts: Vec<Vec<(Address, Option<AccountInfo>)>>,
    /// Storage reverts per transition.
    pub storage: Vec<Vec<PlainStorageRevert>>,
}

impl PlainStateReverts {
    /// Creates reverts with pre-allocated capacity.
    pub fn with_capacity(capacity: usize) -> Self {
        Self { accounts: Vec::with_capacity(capacity), storage: Vec::with_capacity(capacity) }
    }
}

/// Storage slot with original and present values.
#[derive(Debug, Copy, Clone, Default, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct StorageSlot {
    /// Value before the slot was changed.
    pub previous_or_original_value: StorageValue,
    /// Present value.
    pub present_value: StorageValue,
}

impl StorageSlot {
    /// Creates a new unchanged slot.
    pub const fn new(original: StorageValue) -> Self {
        Self { previous_or_original_value: original, present_value: original }
    }

    /// Creates a new changed slot.
    pub const fn new_changed(
        previous_or_original_value: StorageValue,
        present_value: StorageValue,
    ) -> Self {
        Self { previous_or_original_value, present_value }
    }

    /// Returns true if this slot changed.
    pub fn is_changed(&self) -> bool {
        self.previous_or_original_value != self.present_value
    }

    /// Returns the original value.
    pub const fn original_value(&self) -> StorageValue {
        self.previous_or_original_value
    }

    /// Returns the present value.
    pub const fn present_value(&self) -> StorageValue {
        self.present_value
    }
}

impl From<EvmStorageSlot> for StorageSlot {
    fn from(value: EvmStorageSlot) -> Self {
        Self::new_changed(value.original_value, value.present_value)
    }
}

/// Storage with original values.
pub type StorageWithOriginalValues = StorageKeyMap<StorageSlot>;

/// Plain account cached by state.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PlainAccount {
    /// Account information.
    pub info: AccountInfo,
    /// Account storage.
    pub storage: PlainStorage,
}

impl PlainAccount {
    /// Creates an empty account with storage.
    pub fn new_empty_with_storage(storage: PlainStorage) -> Self {
        Self { info: AccountInfo::default(), storage }
    }

    /// Consumes the account and returns its components.
    pub fn into_components(self) -> (AccountInfo, PlainStorage) {
        (self.info, self.storage)
    }
}

impl From<AccountInfo> for PlainAccount {
    fn from(info: AccountInfo) -> Self {
        Self { info, storage: HashMap::default() }
    }
}

/// Plain storage without previous values.
pub type PlainStorage = StorageKeyMap<StorageValue>;

/// Cache account used by [`CacheState`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CacheAccount {
    /// Account information and storage, if account exists.
    pub account: Option<PlainAccount>,
    /// Account status.
    pub status: AccountStatus,
}

impl From<BundleAccount> for CacheAccount {
    fn from(account: BundleAccount) -> Self {
        Self::from(&account)
    }
}

impl From<&BundleAccount> for CacheAccount {
    fn from(account: &BundleAccount) -> Self {
        let storage =
            account.storage.iter().map(|(key, value)| (*key, value.present_value)).collect();
        let account_info = account.account_info().map(|info| PlainAccount { info, storage });
        Self { account: account_info, status: account.status }
    }
}

impl CacheAccount {
    /// Creates an account loaded from the database.
    pub const fn new_loaded(info: AccountInfo, storage: PlainStorage) -> Self {
        Self { account: Some(PlainAccount { info, storage }), status: AccountStatus::Loaded }
    }

    /// Creates an empty loaded account.
    pub fn new_loaded_empty_eip161(storage: PlainStorage) -> Self {
        Self {
            account: Some(PlainAccount::new_empty_with_storage(storage)),
            status: AccountStatus::LoadedEmptyEIP161,
        }
    }

    /// Creates a loaded missing account.
    pub const fn new_loaded_not_existing() -> Self {
        Self { account: None, status: AccountStatus::LoadedNotExisting }
    }

    /// Returns true if account exists in this cache entry.
    pub const fn is_some(&self) -> bool {
        matches!(
            self.status,
            AccountStatus::Changed |
                AccountStatus::InMemoryChange |
                AccountStatus::DestroyedChanged |
                AccountStatus::Loaded |
                AccountStatus::LoadedEmptyEIP161
        )
    }

    /// Returns a cached storage slot.
    pub fn storage_slot(&self, slot: StorageKey) -> Option<StorageValue> {
        self.account.as_ref().and_then(|account| account.storage.get(&slot).copied())
    }

    /// Returns account info if this account exists.
    pub fn account_info(&self) -> Option<AccountInfo> {
        self.account.as_ref().map(|account| account.info.clone())
    }

    /// Converts the cache account into components.
    pub fn into_components(self) -> (Option<(AccountInfo, PlainStorage)>, AccountStatus) {
        (self.account.map(PlainAccount::into_components), self.status)
    }

    /// Marks an empty EIP-161 account as touched.
    pub fn touch_empty_eip161(&mut self) -> Option<TransitionAccount> {
        let previous_status = self.status;
        let previous_info = self.account.take().map(|account| account.info);
        self.status = self.status.on_touched_empty_post_eip161();

        if matches!(
            previous_status,
            AccountStatus::LoadedNotExisting |
                AccountStatus::Destroyed |
                AccountStatus::DestroyedAgain
        ) {
            None
        } else {
            Some(TransitionAccount {
                info: None,
                status: self.status,
                previous_info,
                previous_status,
                storage: HashMap::default(),
                storage_was_destroyed: true,
            })
        }
    }

    /// Marks this account as selfdestructed.
    pub fn selfdestruct(&mut self) -> Option<TransitionAccount> {
        let previous_info = self.account.take().map(|account| account.info);
        let previous_status = self.status;
        self.status = self.status.on_selfdestructed();

        if previous_status == AccountStatus::LoadedNotExisting {
            None
        } else {
            Some(TransitionAccount {
                info: None,
                status: self.status,
                previous_info,
                previous_status,
                storage: HashMap::default(),
                storage_was_destroyed: true,
            })
        }
    }

    /// Applies a newly created account.
    pub fn newly_created(
        &mut self,
        new_info: AccountInfo,
        new_storage: StorageWithOriginalValues,
    ) -> TransitionAccount {
        let previous_status = self.status;
        let previous_info = self.account.take().map(|account| account.info);
        let new_bundle_storage =
            new_storage.iter().map(|(key, slot)| (*key, slot.present_value)).collect();

        self.status = self.status.on_created();
        let transition = TransitionAccount {
            info: Some(new_info.clone()),
            status: self.status,
            previous_status,
            previous_info,
            storage: new_storage,
            storage_was_destroyed: false,
        };
        self.account = Some(PlainAccount { info: new_info, storage: new_bundle_storage });
        transition
    }

    /// Increments account balance.
    pub fn increment_balance(&mut self, balance: u128) -> Option<TransitionAccount> {
        if balance == 0 {
            return None
        }
        let (_, transition) = self.account_info_change(|info| {
            info.balance = info.balance.saturating_add(U256::from(balance));
        });
        Some(transition)
    }

    fn account_info_change<T, F: FnOnce(&mut AccountInfo) -> T>(
        &mut self,
        change: F,
    ) -> (T, TransitionAccount) {
        let previous_status = self.status;
        let previous_info = self.account_info();
        let mut account = self.account.take().unwrap_or_default();
        let output = change(&mut account.info);
        self.account = Some(account);

        let had_no_nonce_and_code =
            previous_info.as_ref().is_some_and(AccountInfo::has_no_code_and_nonce);
        self.status = self.status.on_changed(had_no_nonce_and_code);

        (
            output,
            TransitionAccount {
                info: self.account_info(),
                status: self.status,
                previous_info,
                previous_status,
                storage: HashMap::default(),
                storage_was_destroyed: false,
            },
        )
    }

    /// Drains account balance.
    pub fn drain_balance(&mut self) -> (u128, TransitionAccount) {
        self.account_info_change(|info| {
            let output = info.balance;
            info.balance = U256::ZERO;
            output.try_into().unwrap()
        })
    }

    /// Applies changed account info and storage.
    pub fn change(
        &mut self,
        new: AccountInfo,
        storage: StorageWithOriginalValues,
    ) -> TransitionAccount {
        let previous_status = self.status;
        let (previous_info, mut current_storage) = if let Some(account) = self.account.take() {
            (Some(account.info), account.storage)
        } else {
            (None, Default::default())
        };

        current_storage.extend(storage.iter().map(|(key, slot)| (*key, slot.present_value)));
        let had_no_nonce_and_code =
            previous_info.as_ref().is_some_and(AccountInfo::has_no_code_and_nonce);
        self.status = self.status.on_changed(had_no_nonce_and_code);
        self.account = Some(PlainAccount { info: new, storage: current_storage });

        TransitionAccount {
            info: self.account.as_ref().map(|account| account.info.clone()),
            status: self.status,
            previous_info,
            previous_status,
            storage,
            storage_was_destroyed: false,
        }
    }
}

/// Cache state contains loaded accounts and contracts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CacheState {
    /// Cached accounts.
    pub accounts: AddressMap<CacheAccount>,
    /// Cached contracts.
    pub contracts: B256Map<Bytecode>,
}

impl Default for CacheState {
    fn default() -> Self {
        Self::new()
    }
}

impl CacheState {
    /// Creates an empty cache.
    pub fn new() -> Self {
        Self { accounts: HashMap::default(), contracts: HashMap::default() }
    }

    /// Clears the cache.
    pub fn clear(&mut self) {
        self.accounts.clear();
        self.contracts.clear();
    }

    /// Returns accounts for trie construction.
    pub fn trie_account(&self) -> impl IntoIterator<Item = (Address, &PlainAccount)> {
        self.accounts.iter().filter_map(|(address, account)| {
            account.account.as_ref().map(|plain_account| (*address, plain_account))
        })
    }

    /// Inserts a missing account.
    pub fn insert_not_existing(&mut self, address: Address) {
        self.accounts.insert(address, CacheAccount::new_loaded_not_existing());
    }

    /// Inserts an account.
    pub fn insert_account(&mut self, address: Address, info: AccountInfo) {
        let account = if info.is_empty() {
            CacheAccount::new_loaded_empty_eip161(HashMap::default())
        } else {
            CacheAccount::new_loaded(info, HashMap::default())
        };
        self.accounts.insert(address, account);
    }

    /// Inserts an account with storage.
    pub fn insert_account_with_storage(
        &mut self,
        address: Address,
        info: AccountInfo,
        storage: PlainStorage,
    ) {
        let account = if info.is_empty() {
            CacheAccount::new_loaded_empty_eip161(storage)
        } else {
            CacheAccount::new_loaded(info, storage)
        };
        self.accounts.insert(address, account);
    }

    /// Applies EVM account changes and returns transitions.
    pub fn apply_evm_state<F>(
        &mut self,
        evm_state: impl IntoIterator<Item = (Address, Account)>,
        inspect: F,
    ) -> Vec<(Address, TransitionAccount)>
    where
        F: FnMut(&Address, &Account),
    {
        self.apply_evm_state_iter(evm_state, inspect).collect()
    }

    /// Applies EVM account changes and returns transition iterator.
    pub fn apply_evm_state_iter<'a, F, T>(
        &'a mut self,
        evm_state: T,
        mut inspect: F,
    ) -> impl Iterator<Item = (Address, TransitionAccount)> + use<'a, F, T>
    where
        F: FnMut(&Address, &Account),
        T: IntoIterator<Item = (Address, Account)>,
    {
        evm_state.into_iter().filter_map(move |(address, account)| {
            inspect(&address, &account);
            self.apply_account_state(address, account).map(|transition| (address, transition))
        })
    }

    fn apply_account_state(
        &mut self,
        address: Address,
        account: Account,
    ) -> Option<TransitionAccount> {
        if !account.is_touched() {
            return None
        }

        let cached_account = match self.accounts.entry(address) {
            alloy_primitives::map::Entry::Occupied(entry) => entry.into_mut(),
            alloy_primitives::map::Entry::Vacant(entry) => {
                let cached = if account.is_loaded_as_not_existing() {
                    CacheAccount::new_loaded_not_existing()
                } else {
                    let original = account.original_info();
                    if original.is_empty() {
                        CacheAccount::new_loaded_empty_eip161(HashMap::default())
                    } else {
                        CacheAccount::new_loaded(original, HashMap::default())
                    }
                };
                entry.insert(cached)
            }
        };

        if account.is_selfdestructed() {
            return cached_account.selfdestruct()
        }

        let is_created = account.is_created();
        let is_empty = account.is_empty();
        let changed_storage = account
            .storage
            .into_iter()
            .filter(|(_, slot)| slot.is_changed())
            .map(|(key, slot)| (key, slot.into()))
            .collect();

        if is_created {
            return Some(cached_account.newly_created(account.info, changed_storage))
        }

        if is_empty {
            cached_account.touch_empty_eip161()
        } else {
            Some(cached_account.change(account.info, changed_storage))
        }
    }
}

/// Fixed-size block hash cache for recent EVM block hash lookups.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockHashCache {
    hashes: Box<[(Option<u64>, B256); BLOCK_HASH_HISTORY as usize]>,
}

impl Default for BlockHashCache {
    fn default() -> Self {
        Self::new()
    }
}

impl BlockHashCache {
    /// Creates an empty block hash cache.
    pub fn new() -> Self {
        Self { hashes: Box::new([(None, B256::ZERO); BLOCK_HASH_HISTORY as usize]) }
    }

    /// Inserts a block hash.
    pub fn insert(&mut self, block_number: u64, block_hash: B256) {
        let index = (block_number % BLOCK_HASH_HISTORY) as usize;
        self.hashes[index] = (Some(block_number), block_hash);
    }

    /// Gets a cached block hash.
    pub fn get(&self, block_number: u64) -> Option<B256> {
        let index = (block_number % BLOCK_HASH_HISTORY) as usize;
        let (stored_block_number, stored_hash) = self.hashes[index];
        (stored_block_number == Some(block_number)).then_some(stored_hash)
    }

    /// Returns cached block hashes.
    pub fn iter(&self) -> impl Iterator<Item = (u64, B256)> + '_ {
        self.hashes.iter().filter_map(|(block_number, hash)| {
            block_number.map(|block_number| (block_number, *hash))
        })
    }

    /// Extends the cache.
    pub fn extend(&mut self, iter: impl IntoIterator<Item = (u64, B256)>) {
        for (block_number, block_hash) in iter {
            self.insert(block_number, block_hash);
        }
    }

    /// Returns the lowest cached block hash.
    pub fn lowest(&self) -> Option<(u64, B256)> {
        self.iter().min_by_key(|(block_number, _)| *block_number)
    }
}

/// Database boxed with a lifetime and Send.
pub type DBBox<'a, E> = Box<dyn Database<Error = E> + Send + 'a>;

/// More constrained version of [`State`] using a boxed database.
pub type StateDBBox<'a, E> = State<DBBox<'a, E>>;

/// State cache and bundle accumulator used by EVM execution.
#[derive(derive_more::Debug)]
pub struct State<DB> {
    /// Cached loaded and changed state.
    pub cache: CacheState,
    /// Backing database.
    pub database: DB,
    /// Pending transition state.
    pub transition_state: Option<TransitionState>,
    /// Accumulated bundle state.
    pub bundle_state: BundleState,
    /// Whether to read from the preloaded bundle.
    pub use_preloaded_bundle: bool,
    /// Cached block hashes.
    pub block_hashes: BlockHashCache,
    /// BAL state.
    pub bal_state: BalState,
    /// Hook invoked whenever state is committed.
    #[debug(skip)]
    pub state_hook: Option<Box<dyn OnStateHook>>,
}

impl State<EmptyDB> {
    /// Returns a state builder.
    pub fn builder() -> StateBuilder<EmptyDB> {
        StateBuilder::default()
    }
}

impl<DB: Database> State<DB> {
    /// Returns the bundle size hint.
    pub fn bundle_size_hint(&self) -> usize {
        self.bundle_state.size_hint()
    }

    /// Inserts a missing account into cache.
    pub fn insert_not_existing(&mut self, address: Address) {
        self.cache.insert_not_existing(address);
    }

    /// Inserts an account into cache.
    pub fn insert_account(&mut self, address: Address, info: AccountInfo) {
        self.cache.insert_account(address, info);
    }

    /// Inserts an account with storage into cache.
    pub fn insert_account_with_storage(
        &mut self,
        address: Address,
        info: AccountInfo,
        storage: PlainStorage,
    ) {
        self.cache.insert_account_with_storage(address, info, storage);
    }

    /// Applies transitions to the pending transition state.
    pub fn apply_transition(
        &mut self,
        transitions: impl IntoIterator<Item = (Address, TransitionAccount)>,
    ) {
        if let Some(state) = self.transition_state.as_mut() {
            state.add_transitions(transitions);
        }
    }

    /// Merges pending transitions into the bundle.
    pub fn merge_transitions(&mut self, retention: BundleRetention) {
        if let Some(transition_state) = self.transition_state.as_mut().map(TransitionState::take) {
            self.bundle_state.apply_transitions_and_create_reverts(transition_state, retention);
        }
    }

    /// Loads a mutable cached account.
    pub fn load_cache_account(&mut self, address: Address) -> Result<&mut CacheAccount, DB::Error> {
        Self::load_cache_account_with(
            &mut self.cache,
            self.use_preloaded_bundle,
            &self.bundle_state,
            &mut self.database,
            address,
        )
    }

    fn load_cache_account_with<'a>(
        cache: &'a mut CacheState,
        use_preloaded_bundle: bool,
        bundle_state: &BundleState,
        database: &mut DB,
        address: Address,
    ) -> Result<&'a mut CacheAccount, DB::Error> {
        Ok(match cache.accounts.entry(address) {
            alloy_primitives::map::Entry::Vacant(entry) => {
                if use_preloaded_bundle &&
                    let Some(account) = bundle_state.account(&address).map(Into::into)
                {
                    return Ok(entry.insert(account));
                }
                let info = database.basic(address)?;
                let account = match info {
                    None => CacheAccount::new_loaded_not_existing(),
                    Some(info) if info.is_empty() => {
                        CacheAccount::new_loaded_empty_eip161(HashMap::default())
                    }
                    Some(info) => CacheAccount::new_loaded(info, HashMap::default()),
                };
                entry.insert(account)
            }
            alloy_primitives::map::Entry::Occupied(entry) => entry.into_mut(),
        })
    }

    /// Takes the accumulated bundle.
    pub fn take_bundle(&mut self) -> BundleState {
        mem::take(&mut self.bundle_state)
    }

    /// Takes built BAL.
    pub const fn take_built_bal(&mut self) -> Option<Bal> {
        self.bal_state.take_built_bal()
    }

    /// Takes built alloy BAL.
    pub fn take_built_alloy_bal(&mut self) -> Option<alloy_eip7928::BlockAccessList> {
        self.bal_state.take_built_alloy_bal()
    }

    /// Bumps BAL index.
    pub const fn bump_bal_index(&mut self) {
        self.bal_state.bump_bal_index();
    }

    /// Sets BAL index.
    pub const fn set_bal_index(&mut self, index: BlockAccessIndex) {
        self.bal_state.bal_index = index;
    }

    /// Resets BAL index.
    pub const fn reset_bal_index(&mut self) {
        self.bal_state.reset_bal_index();
    }

    /// Sets BAL.
    pub fn set_bal(&mut self, bal: Option<Arc<Bal>>) {
        self.bal_state.bal = bal;
    }

    /// Sets state hook.
    pub fn set_state_hook(&mut self, hook: Option<Box<dyn OnStateHook>>) {
        self.state_hook = hook;
    }

    /// Sets state hook.
    pub fn with_state_hook(mut self, hook: Option<Box<dyn OnStateHook>>) -> Self {
        self.set_state_hook(hook);
        self
    }

    /// Returns true if BAL is configured.
    pub const fn has_bal(&self) -> bool {
        self.bal_state.bal.is_some()
    }

    fn storage(&mut self, address: Address, index: StorageKey) -> Result<StorageValue, DB::Error> {
        let account = Self::load_cache_account_with(
            &mut self.cache,
            self.use_preloaded_bundle,
            &self.bundle_state,
            &mut self.database,
            address,
        )?;
        let is_storage_known = account.status.is_storage_known();
        Ok(account
            .account
            .as_mut()
            .map(|account| match account.storage.entry(index) {
                alloy_primitives::map::Entry::Occupied(entry) => Ok(*entry.get()),
                alloy_primitives::map::Entry::Vacant(entry) => {
                    let value = if is_storage_known {
                        StorageValue::ZERO
                    } else {
                        self.database.storage(address, index)?
                    };
                    entry.insert(value);
                    Ok(value)
                }
            })
            .transpose()?
            .unwrap_or_default())
    }
}

impl<DB: Database> Database for State<DB> {
    type Error = EvmDatabaseError<DB::Error>;

    fn basic(&mut self, address: Address) -> Result<Option<AccountInfo>, Self::Error> {
        let account_id = self.bal_state.get_account_id(&address).map_err(EvmDatabaseError::Bal)?;
        let mut basic = self
            .load_cache_account(address)
            .map(|account| account.account_info())
            .map_err(EvmDatabaseError::Database)?;
        if let Some(account_id) = account_id {
            self.bal_state
                .basic_by_account_id(account_id, &mut basic)
                .map_err(EvmDatabaseError::Bal)?;
        }
        Ok(basic)
    }

    fn code_by_hash(&mut self, code_hash: B256) -> Result<Bytecode, Self::Error> {
        match self.cache.contracts.entry(code_hash) {
            alloy_primitives::map::Entry::Occupied(entry) => Ok(entry.get().clone()),
            alloy_primitives::map::Entry::Vacant(entry) => {
                if self.use_preloaded_bundle &&
                    let Some(code) = self.bundle_state.contracts.get(&code_hash)
                {
                    entry.insert(code.clone());
                    return Ok(code.clone())
                }
                let code =
                    self.database.code_by_hash(code_hash).map_err(EvmDatabaseError::Database)?;
                entry.insert(code.clone());
                Ok(code)
            }
        }
    }

    fn storage(
        &mut self,
        address: Address,
        index: StorageKey,
    ) -> Result<StorageValue, Self::Error> {
        if let Some(storage) =
            self.bal_state.storage(&address, index).map_err(EvmDatabaseError::Bal)?
        {
            return Ok(storage)
        }
        Self::storage(self, address, index).map_err(EvmDatabaseError::Database)
    }

    fn storage_by_account_id(
        &mut self,
        address: Address,
        account_id: revm::state::AccountId,
        key: StorageKey,
    ) -> Result<StorageValue, Self::Error> {
        if let Some(storage) = self.bal_state.storage_by_account_id(account_id, key)? {
            return Ok(storage)
        }
        Self::storage(self, address, key).map_err(EvmDatabaseError::Database)
    }

    fn block_hash(&mut self, number: u64) -> Result<B256, Self::Error> {
        if let Some(hash) = self.block_hashes.get(number) {
            return Ok(hash)
        }
        let hash = self.database.block_hash(number).map_err(EvmDatabaseError::Database)?;
        self.block_hashes.insert(number, hash);
        Ok(hash)
    }
}

impl<DB: Database> DatabaseCommit for State<DB> {
    fn commit(&mut self, changes: AddressMap<Account>) {
        if let Some(hook) = self.state_hook.as_mut() {
            hook.on_state(&changes);
        }
        self.bal_state.commit(&changes);
        let transitions = self.cache.apply_evm_state_iter(changes, |_, _| {});
        if let Some(state) = self.transition_state.as_mut() {
            state.add_transitions(transitions);
        } else {
            transitions.for_each(|_| {});
        }
    }

    fn commit_iter(&mut self, changes: &mut dyn Iterator<Item = (Address, Account)>) {
        if self.state_hook.is_some() {
            let changes = changes.collect::<AddressMap<_>>();
            self.commit(changes);
            return
        }

        let transitions = self.cache.apply_evm_state_iter(changes, |address, account| {
            self.bal_state.commit_one(*address, account);
        });
        if let Some(state) = self.transition_state.as_mut() {
            state.add_transitions(transitions);
        } else {
            transitions.for_each(|_| {});
        }
    }
}

impl<DB: DatabaseRef> DatabaseRef for State<DB> {
    type Error = EvmDatabaseError<DB::Error>;

    fn basic_ref(&self, address: Address) -> Result<Option<AccountInfo>, Self::Error> {
        let account_id = self.bal_state.get_account_id(&address)?;
        let mut loaded_account = self.cache.accounts.get(&address).map(CacheAccount::account_info);

        if self.use_preloaded_bundle &&
            loaded_account.is_none() &&
            let Some(account) = self.bundle_state.account(&address)
        {
            loaded_account = Some(account.account_info());
        }

        if loaded_account.is_none() {
            loaded_account =
                Some(self.database.basic_ref(address).map_err(EvmDatabaseError::Database)?);
        }

        let mut account = loaded_account.unwrap();
        if let Some(account_id) = account_id {
            self.bal_state
                .basic_by_account_id(account_id, &mut account)
                .map_err(EvmDatabaseError::Bal)?;
        }
        Ok(account)
    }

    fn code_by_hash_ref(&self, code_hash: B256) -> Result<Bytecode, Self::Error> {
        if let Some(code) = self.cache.contracts.get(&code_hash) {
            return Ok(code.clone())
        }
        if self.use_preloaded_bundle &&
            let Some(code) = self.bundle_state.contracts.get(&code_hash)
        {
            return Ok(code.clone())
        }
        self.database.code_by_hash_ref(code_hash).map_err(EvmDatabaseError::Database)
    }

    fn storage_ref(
        &self,
        address: Address,
        index: StorageKey,
    ) -> Result<StorageValue, Self::Error> {
        if let Some(storage) = self.bal_state.storage(&address, index)? {
            return Ok(storage)
        }

        if let Some(account) = self.cache.accounts.get(&address) &&
            let Some(plain_account) = &account.account
        {
            if let Some(storage) = plain_account.storage.get(&index) {
                return Ok(*storage)
            }
            if account.status.is_storage_known() {
                return Ok(StorageValue::ZERO)
            }
        }

        self.database.storage_ref(address, index).map_err(EvmDatabaseError::Database)
    }

    fn block_hash_ref(&self, number: u64) -> Result<B256, Self::Error> {
        if let Some(hash) = self.block_hashes.get(number) {
            return Ok(hash)
        }
        self.database.block_hash_ref(number).map_err(EvmDatabaseError::Database)
    }
}

/// Builder for [`State`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StateBuilder<DB> {
    database: DB,
    with_bundle_prestate: Option<BundleState>,
    with_cache_prestate: Option<CacheState>,
    with_bundle_update: bool,
    with_block_hashes: BlockHashCache,
    bal_state: BalState,
}

impl StateBuilder<EmptyDB> {
    /// Creates a builder with an empty database.
    pub fn new() -> Self {
        Self::default()
    }
}

impl<DB: Database + Default> Default for StateBuilder<DB> {
    fn default() -> Self {
        Self::new_with_database(DB::default())
    }
}

impl<DB: Database> StateBuilder<DB> {
    /// Creates a builder with a database.
    pub fn new_with_database(database: DB) -> Self {
        Self {
            database,
            with_cache_prestate: None,
            with_bundle_prestate: None,
            with_bundle_update: false,
            with_block_hashes: BlockHashCache::new(),
            bal_state: BalState::default(),
        }
    }

    /// Sets the database.
    pub fn with_database<ODB: Database>(self, database: ODB) -> StateBuilder<ODB> {
        StateBuilder {
            database,
            with_cache_prestate: self.with_cache_prestate,
            with_bundle_prestate: self.with_bundle_prestate,
            with_bundle_update: self.with_bundle_update,
            with_block_hashes: self.with_block_hashes,
            bal_state: self.bal_state,
        }
    }

    /// Wraps a database ref.
    pub fn with_database_ref<ODB: DatabaseRef>(
        self,
        database: ODB,
    ) -> StateBuilder<WrapDatabaseRef<ODB>> {
        self.with_database(WrapDatabaseRef(database))
    }

    /// Sets a boxed database.
    pub fn with_database_boxed<Error: DBErrorMarker>(
        self,
        database: DBBox<'_, Error>,
    ) -> StateBuilder<DBBox<'_, Error>> {
        self.with_database(database)
    }

    /// Sets a bundle prestate.
    pub fn with_bundle_prestate(self, bundle: BundleState) -> Self {
        Self { with_bundle_prestate: Some(bundle), ..self }
    }

    /// Enables bundle updates.
    pub fn with_bundle_update(self) -> Self {
        Self { with_bundle_update: true, ..self }
    }

    /// Sets cache prestate.
    pub fn with_cached_prestate(self, cache: CacheState) -> Self {
        Self { with_cache_prestate: Some(cache), ..self }
    }

    /// Sets block hashes.
    pub fn with_block_hashes(self, block_hashes: BlockHashCache) -> Self {
        Self { with_block_hashes: block_hashes, ..self }
    }

    /// Sets BAL.
    pub fn with_bal(mut self, bal: Arc<Bal>) -> Self {
        self.bal_state.bal = Some(bal);
        self
    }

    /// Enables BAL builder.
    pub fn with_bal_builder(mut self) -> Self {
        self.bal_state.bal_builder = Some(Bal::new());
        self
    }

    /// Conditionally enables BAL builder.
    pub fn with_bal_builder_if(mut self, enable: bool) -> Self {
        if enable {
            self.bal_state.bal_builder = Some(Bal::new());
        }
        self
    }

    /// Builds state.
    pub fn build(mut self) -> State<DB> {
        let use_preloaded_bundle = if self.with_cache_prestate.is_some() {
            self.with_bundle_prestate = None;
            false
        } else {
            self.with_bundle_prestate.is_some()
        };
        State {
            cache: self.with_cache_prestate.unwrap_or_default(),
            database: self.database,
            transition_state: self.with_bundle_update.then(TransitionState::default),
            bundle_state: self.with_bundle_prestate.unwrap_or_default(),
            use_preloaded_bundle,
            block_hashes: self.with_block_hashes,
            bal_state: self.bal_state,
            state_hook: None,
        }
    }
}

fn sorted_storage_cmp<T: Ord>(left: &StorageKeyMap<T>, right: &StorageKeyMap<T>) -> Ordering {
    let mut left = left.iter().collect::<Vec<_>>();
    let mut right = right.iter().collect::<Vec<_>>();
    left.sort_by(|(left_key, left_value), (right_key, right_value)| {
        left_key.cmp(right_key).then_with(|| left_value.cmp(right_value))
    });
    right.sort_by(|(left_key, left_value), (right_key, right_value)| {
        left_key.cmp(right_key).then_with(|| left_value.cmp(right_value))
    });
    left.cmp(&right)
}
