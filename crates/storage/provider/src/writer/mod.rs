use alloy_consensus::constants::KECCAK_EMPTY;
use alloy_primitives::{Address, B256, U256};
use reth_execution_types::{
    BlockReverts, ExecutableBytecode, ExecutionAccountChangeRef, ExecutionAccountInfo,
    ExecutionAccountInfoRef, ExecutionStateChangeSink, ExecutionStateChangeSource,
    ExecutionStorageChange,
};
use reth_primitives_traits::{Account, Bytecode};
use reth_storage_api::{
    OriginalValuesKnown, PlainStateReverts, PlainStorageChangeset, PlainStorageRevert,
    RevertToSlot, StateChangeset,
};
use std::{collections::BTreeMap, convert::Infallible};

pub(crate) fn execution_state_and_reverts_to_plain_state_and_reverts<S>(
    state: &S,
    block_reverts: &[BlockReverts],
    is_value_known: OriginalValuesKnown,
) -> (StateChangeset, PlainStateReverts)
where
    S: ExecutionStateChangeSource,
{
    let plain_state = execution_state_to_plain_state(state, is_value_known);

    let mut reverts = PlainStateReverts::with_capacity(block_reverts.len());
    for block_reverts in block_reverts {
        reverts.accounts.push(
            block_reverts
                .accounts
                .iter()
                .map(|(address, account)| {
                    (
                        *address,
                        account.as_ref().map(|account| {
                            execution_account_info_to_reth(&account.to_account_info())
                        }),
                    )
                })
                .collect(),
        );
        reverts.storage.push(
            block_reverts
                .storage
                .iter()
                .map(|(address, storage)| PlainStorageRevert {
                    address: *address,
                    wiped: storage.wiped && !storage.previous_wipe,
                    storage_revert: storage
                        .slots
                        .iter()
                        .filter(|(_, value)| !storage.wiped || !value.is_zero())
                        .map(|(key, value)| (*key, RevertToSlot::Some(*value)))
                        .collect(),
                })
                .collect(),
        );
    }

    (plain_state, reverts)
}

pub(crate) fn execution_state_to_plain_state<S>(
    state: &S,
    is_value_known: OriginalValuesKnown,
) -> StateChangeset
where
    S: ExecutionStateChangeSource,
{
    let mut sink = PlainStateSink::new(is_value_known);
    match state.visit(&mut sink) {
        Ok(()) => {}
        Err(err) => match err {},
    }
    sink.finish()
}

pub(crate) fn execution_state_to_plain_state_and_reverts<S>(
    state: &S,
    is_value_known: OriginalValuesKnown,
) -> (StateChangeset, PlainStateReverts)
where
    S: ExecutionStateChangeSource,
{
    let mut sink = PlainStateAndRevertsSink::new(is_value_known);
    match state.visit(&mut sink) {
        Ok(()) => {}
        Err(err) => match err {},
    }
    sink.finish()
}

fn execution_account_info_to_reth(info: &ExecutionAccountInfo) -> Account {
    Account {
        balance: info.balance,
        nonce: info.nonce,
        bytecode_hash: (!info.code_hash.is_zero() && info.code_hash != KECCAK_EMPTY)
            .then_some(info.code_hash),
    }
}

fn execution_account_info_ref_to_reth(info: ExecutionAccountInfoRef<'_>) -> Account {
    Account {
        balance: info.balance,
        nonce: info.nonce,
        bytecode_hash: (!info.code_hash.is_zero() && info.code_hash != KECCAK_EMPTY)
            .then_some(info.code_hash),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PlainStateInputOrder {
    Sorted,
    Unsorted,
}

impl PlainStateInputOrder {
    pub(crate) const fn is_unsorted(self) -> bool {
        matches!(self, Self::Unsorted)
    }
}

pub(crate) fn is_sorted_by_key<T, K: Ord>(items: &[T], mut key: impl FnMut(&T) -> K) -> bool {
    items.windows(2).all(|window| key(&window[0]) <= key(&window[1]))
}

struct PlainStateSink {
    is_value_known: OriginalValuesKnown,
    accounts: Vec<(Address, Option<Account>)>,
    storage_by_address: BTreeMap<Address, (bool, Vec<(U256, U256)>)>,
    contracts: Vec<(B256, Bytecode)>,
}

impl PlainStateSink {
    const fn new(is_value_known: OriginalValuesKnown) -> Self {
        Self {
            is_value_known,
            accounts: Vec::new(),
            storage_by_address: BTreeMap::new(),
            contracts: Vec::new(),
        }
    }

    fn finish(self) -> StateChangeset {
        let Self { accounts, storage_by_address, contracts, .. } = self;

        let storage = storage_by_address
            .into_iter()
            .filter_map(|(address, (wipe_storage, changed_storage))| {
                (!changed_storage.is_empty() || wipe_storage).then_some(PlainStorageChangeset {
                    address,
                    wipe_storage,
                    storage: changed_storage,
                })
            })
            .collect();

        StateChangeset { accounts, storage, contracts }
    }
}

impl ExecutionStateChangeSink for PlainStateSink {
    type Error = Infallible;

    fn bytecode(
        &mut self,
        code_hash: B256,
        bytecode: &ExecutableBytecode,
    ) -> Result<(), Self::Error> {
        if code_hash != KECCAK_EMPTY {
            self.contracts.push((code_hash, bytecode.clone().into()));
        }
        Ok(())
    }

    fn account(&mut self, change: ExecutionAccountChangeRef<'_>) -> Result<(), Self::Error> {
        if self.is_value_known.is_not_known() || change.original != change.current {
            self.accounts
                .push((change.address, change.current.map(execution_account_info_ref_to_reth)));
        }
        Ok(())
    }

    fn storage_wipe(&mut self, address: Address) -> Result<(), Self::Error> {
        self.storage_by_address.entry(address).or_default().0 = true;
        Ok(())
    }

    fn storage(&mut self, change: ExecutionStorageChange) -> Result<(), Self::Error> {
        let entry = self.storage_by_address.entry(change.address).or_default();
        let wipe_and_not_zero = entry.0 && !change.current.is_zero();
        let not_wiped_and_changed = !entry.0 && change.original != change.current;
        if self.is_value_known.is_not_known() || wipe_and_not_zero || not_wiped_and_changed {
            entry.1.push((change.key, change.current));
        }
        Ok(())
    }
}

struct PlainStateAndRevertsSink {
    is_value_known: OriginalValuesKnown,
    accounts: Vec<(Address, Option<Account>)>,
    account_reverts: Vec<(Address, Option<Account>)>,
    storage_by_address: BTreeMap<Address, (bool, Vec<(U256, U256)>)>,
    storage_reverts: BTreeMap<Address, (bool, Vec<(U256, RevertToSlot)>)>,
    contracts: Vec<(B256, Bytecode)>,
}

impl PlainStateAndRevertsSink {
    const fn new(is_value_known: OriginalValuesKnown) -> Self {
        Self {
            is_value_known,
            accounts: Vec::new(),
            account_reverts: Vec::new(),
            storage_by_address: BTreeMap::new(),
            storage_reverts: BTreeMap::new(),
            contracts: Vec::new(),
        }
    }

    fn finish(self) -> (StateChangeset, PlainStateReverts) {
        let Self {
            accounts, account_reverts, storage_by_address, storage_reverts, contracts, ..
        } = self;

        let storage = storage_by_address
            .into_iter()
            .filter_map(|(address, (wipe_storage, changed_storage))| {
                (!changed_storage.is_empty() || wipe_storage).then_some(PlainStorageChangeset {
                    address,
                    wipe_storage,
                    storage: changed_storage,
                })
            })
            .collect();

        let mut reverts = PlainStateReverts::with_capacity(1);
        reverts.accounts.push(account_reverts);
        reverts.storage.push(
            storage_reverts
                .into_iter()
                .map(|(address, (wiped, storage_revert))| PlainStorageRevert {
                    address,
                    wiped,
                    storage_revert,
                })
                .collect(),
        );

        (StateChangeset { accounts, storage, contracts }, reverts)
    }
}

impl ExecutionStateChangeSink for PlainStateAndRevertsSink {
    type Error = Infallible;

    fn bytecode(
        &mut self,
        code_hash: B256,
        bytecode: &ExecutableBytecode,
    ) -> Result<(), Self::Error> {
        if code_hash != KECCAK_EMPTY {
            self.contracts.push((code_hash, bytecode.clone().into()));
        }
        Ok(())
    }

    fn account(&mut self, change: ExecutionAccountChangeRef<'_>) -> Result<(), Self::Error> {
        if self.is_value_known.is_not_known() || change.original != change.current {
            self.accounts
                .push((change.address, change.current.map(execution_account_info_ref_to_reth)));
        }
        self.account_reverts
            .push((change.address, change.original.map(execution_account_info_ref_to_reth)));
        Ok(())
    }

    fn storage_wipe(&mut self, address: Address) -> Result<(), Self::Error> {
        self.storage_by_address.entry(address).or_default().0 = true;
        self.storage_reverts.entry(address).or_default().0 = true;
        Ok(())
    }

    fn storage(&mut self, change: ExecutionStorageChange) -> Result<(), Self::Error> {
        let entry = self.storage_by_address.entry(change.address).or_default();
        let wipe_and_not_zero = entry.0 && !change.current.is_zero();
        let not_wiped_and_changed = !entry.0 && change.original != change.current;
        if self.is_value_known.is_not_known() || wipe_and_not_zero || not_wiped_and_changed {
            entry.1.push((change.key, change.current));
        }

        let revert_entry = self.storage_reverts.entry(change.address).or_default();
        if !revert_entry.0 || !change.original.is_zero() {
            revert_entry.1.push((change.key, RevertToSlot::Some(change.original)));
        }
        Ok(())
    }
}
