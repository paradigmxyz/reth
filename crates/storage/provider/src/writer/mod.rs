use crate::{
    changesets_utils::StorageRevertsIter,
    providers::{DatabaseProvider, NodeTypesForProvider},
    EitherWriter,
};
use alloy_consensus::{constants::KECCAK_EMPTY, transaction::Either};
use alloy_primitives::{Address, BlockNumber, B256, U256};
use rayon::slice::ParallelSliceMut;
use reth_db_api::{
    cursor::{DbCursorRO, DbCursorRW, DbDupCursorRO, DbDupCursorRW},
    models::{AccountBeforeTx, StorageBeforeTx},
    tables,
    transaction::{DbTx, DbTxMut},
};
use reth_execution_types::{
    BlockReverts, ExecutableBytecode, ExecutionAccountChangeRef, ExecutionAccountInfo,
    ExecutionAccountInfoRef, ExecutionStateChangeSink, ExecutionStateChangeSource,
    ExecutionStorageChange,
};
use reth_primitives_traits::{Account, Bytecode, StorageEntry};
use reth_storage_api::{
    OriginalValuesKnown, PlainStateReverts, PlainStorageChangeset, PlainStorageRevert,
    RevertToSlot, StateChangeset, StateWriteConfig, StorageSettingsCache, WriteStateInput,
};
use reth_storage_errors::provider::ProviderResult;
use std::{collections::BTreeMap, convert::Infallible};

fn execution_state_and_reverts_to_plain_state_and_reverts<S>(
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

fn execution_state_to_plain_state<S>(
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

fn execution_state_to_plain_state_and_reverts<S>(
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

pub(crate) fn execution_state_to_plain_reverts<S>(state: &S) -> PlainStateReverts
where
    S: ExecutionStateChangeSource,
{
    let mut sink = PlainRevertsSink::new();
    match state.visit(&mut sink) {
        Ok(()) => {}
        Err(err) => match err {},
    }
    sink.finish()
}

pub(crate) fn write_state_input_to_plain_state_and_reverts<R>(
    input: &WriteStateInput<'_, R>,
    is_value_known: OriginalValuesKnown,
) -> (StateChangeset, PlainStateReverts, PlainStateInputOrder, PlainStateInputOrder) {
    match input {
        WriteStateInput::Single { outcome, .. } => {
            let (plain_state, reverts) =
                execution_state_to_plain_state_and_reverts(&outcome.state, is_value_known);
            (plain_state, reverts, PlainStateInputOrder::Sorted, PlainStateInputOrder::Sorted)
        }
        WriteStateInput::Multiple(outcome) => {
            let (plain_state, reverts) = execution_state_and_reverts_to_plain_state_and_reverts(
                outcome.execution_state_ref(),
                outcome.block_reverts(),
                is_value_known,
            );
            (plain_state, reverts, PlainStateInputOrder::Unsorted, PlainStateInputOrder::Unsorted)
        }
    }
}

pub(crate) fn write_state_input_bytecodes<'a, R>(
    input: &'a WriteStateInput<'_, R>,
) -> impl Iterator<Item = (B256, Bytecode)> + 'a {
    match input {
        WriteStateInput::Single { outcome, .. } => Either::Left(
            outcome
                .state
                .code()
                .filter(|(hash, _)| **hash != KECCAK_EMPTY)
                .map(|(hash, bytecode)| (*hash, bytecode.clone().into())),
        ),
        WriteStateInput::Multiple(outcome) => Either::Right(outcome.bytecodes()),
    }
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

pub(crate) fn write_state_reverts_with_order<TX, N>(
    provider: &DatabaseProvider<TX, N>,
    reverts: PlainStateReverts,
    first_block: BlockNumber,
    config: StateWriteConfig,
    input_order: PlainStateInputOrder,
) -> ProviderResult<()>
where
    TX: DbTxMut + DbTx + 'static,
    N: NodeTypesForProvider,
{
    if config.write_storage_changesets {
        tracing::trace!("Writing storage changes");
        let mut storages_cursor =
            provider.tx_ref().cursor_dup_write::<tables::PlainStorageState>()?;
        for (block_index, mut storage_changes) in reverts.storage.into_iter().enumerate() {
            let block_number = first_block + block_index as BlockNumber;

            tracing::trace!(block_number, "Writing block change");
            if input_order.is_unsorted() {
                storage_changes.par_sort_unstable_by_key(|a| a.address);
            } else {
                debug_assert!(is_sorted_by_key(&storage_changes, |change| change.address));
            }

            let total_changes =
                storage_changes.iter().map(|change| change.storage_revert.len()).sum();
            let mut changeset = Vec::with_capacity(total_changes);
            for PlainStorageRevert { address, wiped, storage_revert } in storage_changes {
                let mut storage = storage_revert
                    .into_iter()
                    .map(|(k, v)| (B256::from(k.to_be_bytes()), v))
                    .collect::<Vec<_>>();
                if input_order.is_unsorted() {
                    storage.par_sort_unstable_by_key(|a| a.0);
                } else {
                    debug_assert!(is_sorted_by_key(&storage, |(key, _)| *key));
                }

                // If we are writing the primary storage wipe transition, the pre-existing
                // storage state has to be taken from the database and written to storage
                // history. See [StorageWipe::Primary] for more details.
                //
                // TODO(mediocregopher): This could be rewritten in a way which doesn't
                // require collecting wiped entries into a Vec like this, see
                // `write_storage_trie_changesets`.
                let mut wiped_storage = Vec::new();
                if wiped {
                    tracing::trace!(?address, "Wiping storage");
                    if let Some((_, entry)) = storages_cursor.seek_exact(address)? {
                        wiped_storage.push((entry.key, entry.value));
                        while let Some(entry) = storages_cursor.next_dup_val()? {
                            wiped_storage.push((entry.key, entry.value))
                        }
                    }
                }

                tracing::trace!(?address, ?storage, "Writing storage reverts");
                for (key, value) in StorageRevertsIter::new(storage, wiped_storage) {
                    changeset.push(StorageBeforeTx { address, key, value });
                }
            }

            let mut storage_changesets_writer =
                EitherWriter::new_storage_changesets(provider, block_number)?;
            storage_changesets_writer.append_storage_changeset_sorted(block_number, changeset)?;
        }
    }

    if !config.write_account_changesets {
        return Ok(())
    }

    tracing::trace!(?first_block, "Writing account changes");
    for (block_index, mut account_block_reverts) in reverts.accounts.into_iter().enumerate() {
        let block_number = first_block + block_index as BlockNumber;
        if input_order.is_unsorted() {
            account_block_reverts.par_sort_by_key(|(address, _)| *address);
        } else {
            debug_assert!(is_sorted_by_key(&account_block_reverts, |(address, _)| *address));
        }

        let changeset = account_block_reverts
            .into_iter()
            .map(|(address, info)| AccountBeforeTx { address, info })
            .collect::<Vec<_>>();
        let mut account_changesets_writer =
            EitherWriter::new_account_changesets(provider, block_number)?;

        account_changesets_writer.append_account_changeset_sorted(block_number, changeset)?;
    }

    Ok(())
}

pub(crate) fn write_state_changes_with_order<TX, N>(
    provider: &DatabaseProvider<TX, N>,
    mut changes: StateChangeset,
    input_order: PlainStateInputOrder,
) -> ProviderResult<()>
where
    TX: DbTxMut + DbTx + 'static,
    N: NodeTypesForProvider,
{
    if !provider.cached_storage_settings().use_hashed_state() {
        if input_order.is_unsorted() {
            changes.accounts.par_sort_by_key(|a| a.0);
            changes.storage.par_sort_by_key(|a| a.address);
        } else {
            debug_assert!(is_sorted_by_key(&changes.accounts, |(address, _)| *address));
            debug_assert!(is_sorted_by_key(&changes.storage, |change| change.address));
        }

        tracing::trace!(len = changes.accounts.len(), "Writing new account state");
        let mut accounts_cursor = provider.tx_ref().cursor_write::<tables::PlainAccountState>()?;
        for (address, account) in changes.accounts {
            if let Some(account) = account {
                tracing::trace!(?address, "Updating plain state account");
                accounts_cursor.upsert(address, &account)?;
            } else if accounts_cursor.seek_exact(address)?.is_some() {
                tracing::trace!(?address, "Deleting plain state account");
                accounts_cursor.delete_current()?;
            }
        }

        tracing::trace!(len = changes.storage.len(), "Writing new storage state");
        let mut storages_cursor =
            provider.tx_ref().cursor_dup_write::<tables::PlainStorageState>()?;
        for PlainStorageChangeset { address, wipe_storage, storage } in changes.storage {
            if wipe_storage && storages_cursor.seek_exact(address)?.is_some() {
                storages_cursor.delete_current_duplicates()?;
            }

            let mut storage: Vec<StorageEntry> = storage
                .into_iter()
                .map(|(k, value)| StorageEntry { key: k.into(), value })
                .collect::<Vec<_>>();
            if input_order.is_unsorted() {
                storage.par_sort_unstable_by_key(|a| a.key);
            } else {
                debug_assert!(is_sorted_by_key(&storage, |entry| entry.key));
            }

            for entry in storage {
                tracing::trace!(?address, ?entry.key, "Updating plain state storage");
                if let Some(db_entry) = storages_cursor.seek_by_key_subkey(address, entry.key)? &&
                    db_entry.key == entry.key
                {
                    storages_cursor.delete_current()?;
                }

                if !entry.value.is_zero() {
                    storages_cursor.upsert(address, &entry)?;
                }
            }
        }
    }

    if input_order.is_unsorted() {
        changes.contracts.par_sort_by_key(|a| a.0);
    } else {
        debug_assert!(is_sorted_by_key(&changes.contracts, |(code_hash, _)| *code_hash));
    }

    tracing::trace!(len = changes.contracts.len(), "Writing bytecodes");
    provider.write_bytecodes(changes.contracts)?;

    Ok(())
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

struct PlainRevertsSink {
    account_reverts: Vec<(Address, Option<Account>)>,
    storage_reverts: BTreeMap<Address, (bool, Vec<(U256, RevertToSlot)>)>,
}

impl PlainRevertsSink {
    const fn new() -> Self {
        Self { account_reverts: Vec::new(), storage_reverts: BTreeMap::new() }
    }

    fn finish(self) -> PlainStateReverts {
        let Self { account_reverts, storage_reverts } = self;

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
        reverts
    }
}

impl ExecutionStateChangeSink for PlainRevertsSink {
    type Error = Infallible;

    fn account(&mut self, change: ExecutionAccountChangeRef<'_>) -> Result<(), Self::Error> {
        self.account_reverts
            .push((change.address, change.original.map(execution_account_info_ref_to_reth)));
        Ok(())
    }

    fn storage_wipe(&mut self, address: Address) -> Result<(), Self::Error> {
        self.storage_reverts.entry(address).or_default().0 = true;
        Ok(())
    }

    fn storage(&mut self, change: ExecutionStorageChange) -> Result<(), Self::Error> {
        let revert_entry = self.storage_reverts.entry(change.address).or_default();
        if !revert_entry.0 || !change.original.is_zero() {
            revert_entry.1.push((change.key, RevertToSlot::Some(change.original)));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{map::AddressMap, Bytes, B256, U256};
    use reth_execution_types::{
        execution_state_from_init, BlockReverts, ExecutionState, StorageReverts,
    };
    use reth_primitives_traits::{Account, Bytecode};

    #[test]
    fn plain_reverts_skip_secondary_storage_wipes() {
        let address = Address::random();
        let slot = U256::from(1);
        let value = U256::from(2);
        let state = ExecutionState::default();
        let block_reverts = vec![BlockReverts {
            storage: AddressMap::from_iter([(
                address,
                StorageReverts {
                    wiped: true,
                    previous_wipe: true,
                    slots: BTreeMap::from([(slot, value)]),
                },
            )]),
            ..Default::default()
        }];

        let (_, reverts) = execution_state_and_reverts_to_plain_state_and_reverts(
            &state,
            &block_reverts,
            OriginalValuesKnown::Yes,
        );

        assert_eq!(
            reverts.storage,
            [vec![PlainStorageRevert {
                address,
                wiped: false,
                storage_revert: vec![(slot, RevertToSlot::Some(value))],
            }]]
        );
    }

    #[test]
    fn plain_state_conversion_streams_accounts_and_storage() {
        let address = Address::random();
        let slot = U256::from(1);
        let original_slot = U256::from(2);
        let current_slot = U256::from(3);
        let code_hash = B256::repeat_byte(0x42);
        let bytecode = Bytecode::new_raw(Bytes::from_static(&[0x60, 0x00]));
        let original_account = Account { balance: U256::from(4), nonce: 1, bytecode_hash: None };
        let current_account = Account { balance: U256::from(5), nonce: 2, bytecode_hash: None };
        let state = execution_state_from_init(
            [(
                address,
                (
                    Some(original_account),
                    Some(current_account),
                    BTreeMap::from([(slot, (original_slot, current_slot))]),
                ),
            )],
            [(code_hash, bytecode.clone())],
        );

        let (plain_state, reverts) =
            execution_state_to_plain_state_and_reverts(&state, OriginalValuesKnown::Yes);
        let reverts_only = execution_state_to_plain_reverts(&state);
        let plain_state_only = execution_state_to_plain_state(&state, OriginalValuesKnown::Yes);

        assert_eq!(plain_state_only.accounts, plain_state.accounts);
        assert_eq!(plain_state_only.storage, plain_state.storage);
        assert_eq!(plain_state_only.contracts, plain_state.contracts);
        assert_eq!(reverts_only.accounts, reverts.accounts);
        assert_eq!(reverts_only.storage, reverts.storage);
        assert_eq!(plain_state.accounts, vec![(address, Some(current_account))]);
        assert_eq!(
            plain_state.storage,
            vec![PlainStorageChangeset {
                address,
                wipe_storage: false,
                storage: vec![(slot, current_slot)],
            }]
        );
        assert_eq!(plain_state.contracts, vec![(code_hash, bytecode)]);
        assert_eq!(reverts.accounts, vec![vec![(address, Some(original_account))]]);
        assert_eq!(
            reverts.storage,
            vec![vec![PlainStorageRevert {
                address,
                wiped: false,
                storage_revert: vec![(slot, RevertToSlot::Some(original_slot))],
            }]]
        );
    }

    #[test]
    fn plain_state_conversion_returns_sorted_changes() {
        let low_address = Address::with_last_byte(1);
        let high_address = Address::with_last_byte(2);
        let low_code_hash = B256::with_last_byte(1);
        let high_code_hash = B256::with_last_byte(2);
        let bytecode = Bytecode::new_raw(Bytes::from_static(&[0x60, 0x00]));
        let account = Account::default();

        let state = execution_state_from_init(
            [
                (
                    high_address,
                    (
                        None,
                        Some(account),
                        BTreeMap::from([
                            (U256::from(3), (U256::from(30), U256::from(300))),
                            (U256::from(1), (U256::from(10), U256::from(100))),
                        ]),
                    ),
                ),
                (
                    low_address,
                    (
                        None,
                        Some(account),
                        BTreeMap::from([(U256::from(2), (U256::from(20), U256::from(200)))]),
                    ),
                ),
            ],
            [(high_code_hash, bytecode.clone()), (low_code_hash, bytecode)],
        );

        let (plain_state, reverts) =
            execution_state_to_plain_state_and_reverts(&state, OriginalValuesKnown::Yes);

        assert_eq!(
            plain_state.accounts.iter().map(|(address, _)| *address).collect::<Vec<_>>(),
            vec![low_address, high_address]
        );
        assert_eq!(
            plain_state.storage.iter().map(|changes| changes.address).collect::<Vec<_>>(),
            vec![low_address, high_address]
        );
        assert_eq!(
            plain_state.storage[1].storage.iter().map(|(slot, _)| *slot).collect::<Vec<_>>(),
            vec![U256::from(1), U256::from(3)]
        );
        assert_eq!(
            plain_state.contracts.iter().map(|(code_hash, _)| *code_hash).collect::<Vec<_>>(),
            vec![low_code_hash, high_code_hash]
        );
        assert_eq!(
            reverts.accounts[0].iter().map(|(address, _)| *address).collect::<Vec<_>>(),
            vec![low_address, high_address]
        );
        assert_eq!(
            reverts.storage[0].iter().map(|changes| changes.address).collect::<Vec<_>>(),
            vec![low_address, high_address]
        );
        assert_eq!(
            reverts.storage[0][1].storage_revert.iter().map(|(slot, _)| *slot).collect::<Vec<_>>(),
            vec![U256::from(1), U256::from(3)]
        );
    }
}
