use super::bundle_account::ScrollBundleAccount;
use crate::states::{
    changes::{ScrollPlainStateReverts, ScrollStateChangeset},
    reverts::ScrollReverts,
    ScrollAccountInfo, ScrollAccountInfoRevert, ScrollAccountRevert,
};
use reth_scroll_primitives::ScrollPostExecutionContext;
use revm::{
    db::{
        states::{PlainStorageChangeset, StorageSlot},
        AccountStatus, BundleState, OriginalValuesKnown, RevertToSlot,
    },
    primitives::{
        map::{Entry, HashMap, HashSet},
        Address, Bytecode, B256, KECCAK_EMPTY, U256,
    },
};
use std::{
    collections::{hash_map, BTreeMap, BTreeSet},
    ops::RangeInclusive,
};

/// A code copy of the [`BundleState`] modified with Scroll compatible fields.
#[derive(Default, Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ScrollBundleState {
    /// Account state.
    pub state: HashMap<Address, ScrollBundleAccount>,
    /// All created contracts in this block.
    pub contracts: HashMap<B256, Bytecode>,
    /// Changes to revert.
    ///
    /// Note: Inside vector is *not* sorted by address.
    /// But it is unique by address.
    pub reverts: ScrollReverts,
    /// The size of the plain state in the bundle state.
    pub state_size: usize,
    /// The size of reverts in the bundle state.
    pub reverts_size: usize,
}

impl From<(BundleState, &ScrollPostExecutionContext)> for ScrollBundleState {
    fn from((bundle, context): (BundleState, &ScrollPostExecutionContext)) -> Self {
        // we need to clone the reverts because there is no way to take ownership of
        // the inner Vec from the `Reverts` wrapper.
        let reverts = bundle
            .reverts
            .iter()
            .map(|reverts| {
                reverts
                    .iter()
                    .map(|(add, revert)| (*add, (revert.clone(), context).into()))
                    .collect()
            })
            .collect();

        let state = bundle
            .state
            .into_iter()
            .map(|(add, account)| (add, (account, context).into()))
            .collect();

        Self {
            state,
            contracts: bundle.contracts,
            reverts: ScrollReverts::new(reverts),
            state_size: bundle.state_size,
            reverts_size: bundle.reverts_size,
        }
    }
}

// This conversion can cause a loss of information since performed without additional context.
#[cfg(any(not(feature = "scroll"), feature = "test-utils"))]
impl From<BundleState> for ScrollBundleState {
    fn from(bundle: BundleState) -> Self {
        (bundle, &ScrollPostExecutionContext::default()).into()
    }
}

// This conversion can cause a loss of information since performed without additional context.
#[cfg(any(not(feature = "scroll"), feature = "test-utils"))]
impl From<(BundleState, &())> for ScrollBundleState {
    fn from((bundle, _): (BundleState, &())) -> Self {
        bundle.into()
    }
}

impl ScrollBundleState {
    /// Return builder instance for further manipulation
    pub fn builder(revert_range: RangeInclusive<u64>) -> ScrollBundleBuilder {
        ScrollBundleBuilder::new(revert_range)
    }

    /// Create it with new and old values of both Storage and [`ScrollAccountInfo`].
    pub fn new(
        state: impl IntoIterator<
            Item = (
                Address,
                Option<ScrollAccountInfo>,
                Option<ScrollAccountInfo>,
                HashMap<U256, (U256, U256)>,
            ),
        >,
        reverts: impl IntoIterator<
            Item = impl IntoIterator<
                Item = (
                    Address,
                    Option<Option<ScrollAccountInfo>>,
                    impl IntoIterator<Item = (U256, U256)>,
                ),
            >,
        >,
        contracts: impl IntoIterator<Item = (B256, Bytecode)>,
    ) -> Self {
        // Create state from iterator.
        let mut state_size = 0;
        let state = state
            .into_iter()
            .map(|(address, original, present, storage)| {
                let account = ScrollBundleAccount::new(
                    original,
                    present,
                    storage
                        .into_iter()
                        .map(|(k, (o_val, p_val))| (k, StorageSlot::new_changed(o_val, p_val)))
                        .collect(),
                    AccountStatus::Changed,
                );
                state_size += account.size_hint();
                (address, account)
            })
            .collect();

        // Create reverts from iterator.
        let mut reverts_size = 0;
        let reverts = reverts
            .into_iter()
            .map(|block_reverts| {
                block_reverts
                    .into_iter()
                    .map(|(address, account, storage)| {
                        let account = match account {
                            Some(Some(account)) => ScrollAccountInfoRevert::RevertTo(account),
                            Some(None) => ScrollAccountInfoRevert::DeleteIt,
                            None => ScrollAccountInfoRevert::DoNothing,
                        };
                        let revert = ScrollAccountRevert {
                            account,
                            storage: storage
                                .into_iter()
                                .map(|(k, v)| (k, RevertToSlot::Some(v)))
                                .collect(),
                            previous_status: AccountStatus::Changed,
                            wipe_storage: false,
                        };
                        reverts_size += revert.size_hint();
                        (address, revert)
                    })
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();

        Self {
            state,
            contracts: contracts.into_iter().collect(),
            reverts: ScrollReverts::new(reverts),
            state_size,
            reverts_size,
        }
    }

    /// Returns the approximate size of changes in the bundle state.
    /// The estimation is not precise, because the information about the number of
    /// destroyed entries that need to be removed is not accessible to the bundle state.
    pub fn size_hint(&self) -> usize {
        self.state_size + self.reverts_size + self.contracts.len()
    }

    /// Return reference to the state.
    pub const fn state(&self) -> &HashMap<Address, ScrollBundleAccount> {
        &self.state
    }

    /// Is bundle state empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Return number of changed accounts.
    pub fn len(&self) -> usize {
        self.state.len()
    }

    /// Get account from state
    pub fn account(&self, address: &Address) -> Option<&ScrollBundleAccount> {
        self.state.get(address)
    }

    /// Get bytecode from state
    pub fn bytecode(&self, hash: &B256) -> Option<Bytecode> {
        self.contracts.get(hash).cloned()
    }

    /// Extend the bundle with other state
    ///
    /// Update the `other` state only if `other` is not flagged as destroyed.
    pub fn extend_state(&mut self, other_state: HashMap<Address, ScrollBundleAccount>) {
        for (address, other_account) in other_state {
            match self.state.entry(address) {
                Entry::Occupied(mut entry) => {
                    let this = entry.get_mut();
                    self.state_size -= this.size_hint();

                    // if other was destroyed. replace `this` storage with
                    // the `other one.
                    if other_account.was_destroyed() {
                        this.storage = other_account.storage;
                    } else {
                        // otherwise extend this storage with other
                        for (key, storage_slot) in other_account.storage {
                            // update present value or insert storage slot.
                            this.storage.entry(key).or_insert(storage_slot).present_value =
                                storage_slot.present_value;
                        }
                    }
                    this.info = other_account.info;
                    this.status.transition(other_account.status);

                    // Update the state size
                    self.state_size += this.size_hint();
                }
                hash_map::Entry::Vacant(entry) => {
                    // just insert if empty
                    self.state_size += other_account.size_hint();
                    entry.insert(other_account);
                }
            }
        }
    }

    /// Extend the state with state that is build on top of it.
    ///
    /// If storage was wiped in `other` state, copy `this` plain state
    /// and put it inside `other` revert (if there is no duplicates of course).
    ///
    /// If `this` and `other` accounts were both destroyed invalidate second
    /// wipe flag (from `other`). As wiping from database should be done only once
    /// and we already transferred all potentially missing storages to the `other` revert.
    pub fn extend(&mut self, mut other: Self) {
        // iterate over reverts and if its storage is wiped try to add previous bundle
        // state as there is potential missing slots.
        for (address, revert) in other.reverts.iter_mut().flatten() {
            if revert.wipe_storage {
                // If there is wipe storage in `other` revert
                // we need to move storage from present state.
                if let Some(this_account) = self.state.get_mut(address) {
                    // As this account was destroyed inside `other` bundle.
                    // we are fine to wipe/drain this storage and put it inside revert.
                    for (key, value) in this_account.storage.drain() {
                        revert
                            .storage
                            .entry(key)
                            .or_insert(RevertToSlot::Some(value.present_value));
                    }

                    // nullify `other` wipe as primary database wipe is done in `this`.
                    if this_account.was_destroyed() {
                        revert.wipe_storage = false;
                    }
                }
            }

            // Increment reverts size for each of the updated reverts.
            self.reverts_size += revert.size_hint();
        }
        // Extension of state
        self.extend_state(other.state);
        // Contract can be just extended, when counter is introduced we will take into account that.
        self.contracts.extend(other.contracts);
        // Reverts can be just extended
        self.reverts.extend(other.reverts);
    }

    /// Generate a [`ScrollStateChangeset`] from the bundle state without consuming
    /// it.
    pub fn to_plain_state(&self, is_value_known: OriginalValuesKnown) -> ScrollStateChangeset {
        // pessimistically pre-allocate assuming _all_ accounts changed.
        let state_len = self.state.len();
        let mut accounts = Vec::with_capacity(state_len);
        let mut storage = Vec::with_capacity(state_len);

        for (address, account) in &self.state {
            // append account info if it is changed.
            let was_destroyed = account.was_destroyed();
            if is_value_known.is_not_known() || account.is_info_changed() {
                let info = account.info.as_ref().map(ScrollAccountInfo::copy_without_code);
                accounts.push((*address, info));
            }

            // append storage changes

            // NOTE: Assumption is that revert is going to remove whole plain storage from
            // database so we can check if plain state was wiped or not.
            let mut account_storage_changed = Vec::with_capacity(account.storage.len());

            for (key, slot) in account.storage.iter().map(|(k, v)| (*k, *v)) {
                // If storage was destroyed that means that storage was wiped.
                // In that case we need to check if present storage value is different then ZERO.
                let destroyed_and_not_zero = was_destroyed && !slot.present_value.is_zero();

                // If account is not destroyed check if original values was changed,
                // so we can update it.
                let not_destroyed_and_changed = !was_destroyed && slot.is_changed();

                if is_value_known.is_not_known() ||
                    destroyed_and_not_zero ||
                    not_destroyed_and_changed
                {
                    account_storage_changed.push((key, slot.present_value));
                }
            }

            if !account_storage_changed.is_empty() || was_destroyed {
                // append storage changes to account.
                storage.push(PlainStorageChangeset {
                    address: *address,
                    wipe_storage: was_destroyed,
                    storage: account_storage_changed,
                });
            }
        }

        let contracts = self
            .contracts
            .iter()
            // remove empty bytecodes
            .filter(|(b, _)| **b != KECCAK_EMPTY)
            .map(|(b, code)| (*b, code.clone()))
            .collect::<Vec<_>>();
        ScrollStateChangeset { accounts, storage, contracts }
    }

    /// Generate a [`ScrollStateChangeset`] and [`ScrollPlainStateReverts`] from the bundle
    /// state.
    pub fn to_plain_state_and_reverts(
        &self,
        is_value_known: OriginalValuesKnown,
    ) -> (ScrollStateChangeset, ScrollPlainStateReverts) {
        (self.to_plain_state(is_value_known), self.reverts.to_plain_state_reverts())
    }

    /// Take first N raw reverts from the [`ScrollBundleState`].
    pub fn take_n_reverts(&mut self, reverts_to_take: usize) -> ScrollReverts {
        // split is done as [0, num) and [num, len].
        if reverts_to_take > self.reverts.len() {
            return self.take_all_reverts();
        }
        let (detach, this) = self.reverts.split_at(reverts_to_take);
        let detached_reverts = ScrollReverts::new(detach.to_vec());
        self.reverts_size =
            this.iter().flatten().fold(0, |acc, (_, revert)| acc + revert.size_hint());
        self.reverts = ScrollReverts::new(this.to_vec());
        detached_reverts
    }

    /// Return and clear all reverts from [`ScrollBundleState`]
    pub fn take_all_reverts(&mut self) -> ScrollReverts {
        self.reverts_size = 0;
        core::mem::take(&mut self.reverts)
    }

    /// Reverts the state changes by N transitions back.
    ///
    /// See also [`Self::revert_latest`]
    pub fn revert(&mut self, mut num_transitions: usize) {
        if num_transitions == 0 {
            return;
        }

        while self.revert_latest() {
            num_transitions -= 1;
            if num_transitions == 0 {
                // break the loop.
                break;
            }
        }
    }

    /// Reverts the state changes of the latest transition
    ///
    /// Note: This is the same as `BundleState::revert(1)`
    ///
    /// Returns true if the state was reverted.
    pub fn revert_latest(&mut self) -> bool {
        // revert the latest recorded state
        if let Some(reverts) = self.reverts.pop() {
            for (address, revert_account) in reverts {
                self.reverts_size -= revert_account.size_hint();
                match self.state.entry(address) {
                    Entry::Occupied(mut entry) => {
                        let account = entry.get_mut();
                        self.state_size -= account.size_hint();
                        if account.revert(revert_account) {
                            entry.remove();
                        } else {
                            self.state_size += account.size_hint();
                        }
                    }
                    Entry::Vacant(entry) => {
                        // create empty account that we will revert on.
                        // Only place where this account is not existing is if revert is DeleteIt.
                        let mut account = ScrollBundleAccount::new(
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
            return true;
        }

        false
    }
}

/// This builder is used to help to facilitate the initialization of [`ScrollBundleState`] struct.
/// This is a code copy of the [`revm::db::states::BundleBuilder`] to accommodate the
/// [`ScrollBundleState`].
#[derive(Debug)]
pub struct ScrollBundleBuilder {
    states: HashSet<Address>,
    state_original: HashMap<Address, ScrollAccountInfo>,
    state_present: HashMap<Address, ScrollAccountInfo>,
    state_storage: HashMap<Address, HashMap<U256, (U256, U256)>>,

    reverts: BTreeSet<(u64, Address)>,
    revert_range: RangeInclusive<u64>,
    revert_account: HashMap<(u64, Address), Option<Option<ScrollAccountInfo>>>,
    revert_storage: HashMap<(u64, Address), Vec<(U256, U256)>>,

    contracts: HashMap<B256, Bytecode>,
}

impl Default for ScrollBundleBuilder {
    fn default() -> Self {
        Self {
            states: HashSet::default(),
            state_original: HashMap::default(),
            state_present: HashMap::default(),
            state_storage: HashMap::default(),
            reverts: BTreeSet::new(),
            revert_range: 0..=0,
            revert_account: HashMap::default(),
            revert_storage: HashMap::default(),
            contracts: HashMap::default(),
        }
    }
}

impl ScrollBundleBuilder {
    /// Create builder instance
    ///
    /// `revert_range` indicates the size of [`ScrollBundleState`] `reverts` field
    pub fn new(revert_range: RangeInclusive<u64>) -> Self {
        Self { revert_range, ..Default::default() }
    }

    /// Apply a transformation to the builder.
    pub fn apply<F>(self, f: F) -> Self
    where
        F: FnOnce(Self) -> Self,
    {
        f(self)
    }

    /// Apply a mutable transformation to the builder.
    pub fn apply_mut<F>(&mut self, f: F) -> &mut Self
    where
        F: FnOnce(&mut Self),
    {
        f(self);
        self
    }

    /// Collect address info of [`ScrollBundleState`] state
    pub fn state_address(mut self, address: Address) -> Self {
        self.set_state_address(address);
        self
    }

    /// Collect account info of [`ScrollBundleState`] state
    pub fn state_original_account_info(
        mut self,
        address: Address,
        original: ScrollAccountInfo,
    ) -> Self {
        self.set_state_original_account_info(address, original);
        self
    }

    /// Collect account info of [`ScrollBundleState`] state
    pub fn state_present_account_info(
        mut self,
        address: Address,
        present: ScrollAccountInfo,
    ) -> Self {
        self.set_state_present_account_info(address, present);
        self
    }

    /// Collect storage info of [`ScrollBundleState`] state
    pub fn state_storage(mut self, address: Address, storage: HashMap<U256, (U256, U256)>) -> Self {
        self.set_state_storage(address, storage);
        self
    }

    /// Collect address info of [`ScrollBundleState`] reverts
    ///
    /// `block_number` must respect `revert_range`, or the input
    /// will be ignored during the final build process
    pub fn revert_address(mut self, block_number: u64, address: Address) -> Self {
        self.set_revert_address(block_number, address);
        self
    }

    /// Collect account info of [`ScrollBundleState`] reverts
    ///
    /// `block_number` must respect `revert_range`, or the input
    /// will be ignored during the final build process
    pub fn revert_account_info(
        mut self,
        block_number: u64,
        address: Address,
        account: Option<Option<ScrollAccountInfo>>,
    ) -> Self {
        self.set_revert_account_info(block_number, address, account);
        self
    }

    /// Collect storage info of [`ScrollBundleState`] reverts
    ///
    /// `block_number` must respect `revert_range`, or the input
    /// will be ignored during the final build process
    pub fn revert_storage(
        mut self,
        block_number: u64,
        address: Address,
        storage: Vec<(U256, U256)>,
    ) -> Self {
        self.set_revert_storage(block_number, address, storage);
        self
    }

    /// Collect contracts info
    pub fn contract(mut self, address: B256, bytecode: Bytecode) -> Self {
        self.set_contract(address, bytecode);
        self
    }

    /// Set address info of [`ScrollBundleState`] state.
    pub fn set_state_address(&mut self, address: Address) -> &mut Self {
        self.states.insert(address);
        self
    }

    /// Set original account info of [`ScrollBundleState`] state.
    pub fn set_state_original_account_info(
        &mut self,
        address: Address,
        original: ScrollAccountInfo,
    ) -> &mut Self {
        self.states.insert(address);
        self.state_original.insert(address, original);
        self
    }

    /// Set present account info of [`ScrollBundleState`] state.
    pub fn set_state_present_account_info(
        &mut self,
        address: Address,
        present: ScrollAccountInfo,
    ) -> &mut Self {
        self.states.insert(address);
        self.state_present.insert(address, present);
        self
    }

    /// Set storage info of [`ScrollBundleState`] state.
    pub fn set_state_storage(
        &mut self,
        address: Address,
        storage: HashMap<U256, (U256, U256)>,
    ) -> &mut Self {
        self.states.insert(address);
        self.state_storage.insert(address, storage);
        self
    }

    /// Set address info of [`ScrollBundleState`] reverts.
    pub fn set_revert_address(&mut self, block_number: u64, address: Address) -> &mut Self {
        self.reverts.insert((block_number, address));
        self
    }

    /// Set account info of [`ScrollBundleState`] reverts.
    pub fn set_revert_account_info(
        &mut self,
        block_number: u64,
        address: Address,
        account: Option<Option<ScrollAccountInfo>>,
    ) -> &mut Self {
        self.reverts.insert((block_number, address));
        self.revert_account.insert((block_number, address), account);
        self
    }

    /// Set storage info of [`ScrollBundleState`] reverts.
    pub fn set_revert_storage(
        &mut self,
        block_number: u64,
        address: Address,
        storage: Vec<(U256, U256)>,
    ) -> &mut Self {
        self.reverts.insert((block_number, address));
        self.revert_storage.insert((block_number, address), storage);
        self
    }

    /// Set contracts info.
    pub fn set_contract(&mut self, address: B256, bytecode: Bytecode) -> &mut Self {
        self.contracts.insert(address, bytecode);
        self
    }

    /// Create [`ScrollBundleState`] instance based on collected information
    pub fn build(mut self) -> ScrollBundleState {
        let mut state_size = 0;
        let state = self
            .states
            .into_iter()
            .map(|address| {
                let storage = self
                    .state_storage
                    .remove(&address)
                    .map(|s| {
                        s.into_iter()
                            .map(|(k, (o_val, p_val))| (k, StorageSlot::new_changed(o_val, p_val)))
                            .collect()
                    })
                    .unwrap_or_default();
                let bundle_account = ScrollBundleAccount::new(
                    self.state_original.remove(&address),
                    self.state_present.remove(&address),
                    storage,
                    AccountStatus::Changed,
                );
                state_size += bundle_account.size_hint();
                (address, bundle_account)
            })
            .collect();

        let mut reverts_size = 0;
        let mut reverts_map = BTreeMap::new();
        for block_number in self.revert_range {
            reverts_map.insert(block_number, Vec::new());
        }
        self.reverts.into_iter().for_each(|(block_number, address)| {
            let account =
                match self.revert_account.remove(&(block_number, address)).unwrap_or_default() {
                    Some(Some(account)) => ScrollAccountInfoRevert::RevertTo(account),
                    Some(None) => ScrollAccountInfoRevert::DeleteIt,
                    None => ScrollAccountInfoRevert::DoNothing,
                };
            let storage = self
                .revert_storage
                .remove(&(block_number, address))
                .map(|s| s.into_iter().map(|(k, v)| (k, RevertToSlot::Some(v))).collect())
                .unwrap_or_default();
            let account_revert = ScrollAccountRevert {
                account,
                storage,
                previous_status: AccountStatus::Changed,
                wipe_storage: false,
            };

            if reverts_map.contains_key(&block_number) {
                reverts_size += account_revert.size_hint();
                reverts_map
                    .entry(block_number)
                    .or_insert(Vec::new())
                    .push((address, account_revert));
            }
        });

        ScrollBundleState {
            state,
            contracts: self.contracts,
            reverts: ScrollReverts::new(reverts_map.into_values().collect()),
            state_size,
            reverts_size,
        }
    }

    /// Getter for `states` field
    pub const fn get_states(&self) -> &HashSet<Address> {
        &self.states
    }

    /// Mutable getter for `states` field
    pub fn get_states_mut(&mut self) -> &mut HashSet<Address> {
        &mut self.states
    }

    /// Mutable getter for `state_original` field
    pub fn get_state_original_mut(&mut self) -> &mut HashMap<Address, ScrollAccountInfo> {
        &mut self.state_original
    }

    /// Mutable getter for `state_present` field
    pub fn get_state_present_mut(&mut self) -> &mut HashMap<Address, ScrollAccountInfo> {
        &mut self.state_present
    }

    /// Mutable getter for `state_storage` field
    pub fn get_state_storage_mut(&mut self) -> &mut HashMap<Address, HashMap<U256, (U256, U256)>> {
        &mut self.state_storage
    }

    /// Mutable getter for `reverts` field
    pub fn get_reverts_mut(&mut self) -> &mut BTreeSet<(u64, Address)> {
        &mut self.reverts
    }

    /// Mutable getter for `revert_range` field
    pub fn get_revert_range_mut(&mut self) -> &mut RangeInclusive<u64> {
        &mut self.revert_range
    }

    /// Mutable getter for `revert_account` field
    pub fn get_revert_account_mut(
        &mut self,
    ) -> &mut HashMap<(u64, Address), Option<Option<ScrollAccountInfo>>> {
        &mut self.revert_account
    }

    /// Mutable getter for `revert_storage` field
    pub fn get_revert_storage_mut(&mut self) -> &mut HashMap<(u64, Address), Vec<(U256, U256)>> {
        &mut self.revert_storage
    }

    /// Mutable getter for `contracts` field
    pub fn get_contracts_mut(&mut self) -> &mut HashMap<B256, Bytecode> {
        &mut self.contracts
    }
}
