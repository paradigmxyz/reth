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
    BlockReverts, EvmStateChangeSink, EvmStateChangeSource, ExecutableBytecode,
    ExecutionAccountChangeRef, ExecutionAccountInfo, ExecutionStorageChange,
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
    S: EvmStateChangeSource,
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
                        .filter(|(_, value)| {
                            !storage.wiped ||
                                !matches!(**value, RevertToSlot::Some(value) if value.is_zero())
                        })
                        .map(|(key, value)| (*key, *value))
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
    S: EvmStateChangeSource,
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
    S: EvmStateChangeSource,
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
    S: EvmStateChangeSource,
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

fn execution_account_info_ref_to_reth(info: &ExecutionAccountInfo) -> Account {
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
    const fn is_unsorted(self) -> bool {
        matches!(self, Self::Unsorted)
    }
}

fn is_sorted_by_key<T, K: Ord>(items: &[T], mut key: impl FnMut(&T) -> K) -> bool {
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

impl EvmStateChangeSink for PlainStateSink {
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
        if change.original.is_some() && change.current.is_none() {
            self.storage_wipe(change.address)?;
        }
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

impl EvmStateChangeSink for PlainStateAndRevertsSink {
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
        if change.original.is_some() && change.current.is_none() {
            self.storage_wipe(change.address)?;
        }
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

impl EvmStateChangeSink for PlainRevertsSink {
    type Error = Infallible;

    fn account(&mut self, change: ExecutionAccountChangeRef<'_>) -> Result<(), Self::Error> {
        if change.original.is_some() && change.current.is_none() {
            self.storage_wipe(change.address)?;
        }
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
        execution_state_from_init, BlockReverts, EvmState, RevertToSlot, StorageReverts,
    };
    use reth_primitives_traits::{Account, Bytecode};

    #[test]
    fn plain_reverts_skip_secondary_storage_wipes() {
        let address = Address::random();
        let slot = U256::from(1);
        let value = U256::from(2);
        let state = EvmState::default();
        let block_reverts = vec![BlockReverts {
            storage: AddressMap::from_iter([(
                address,
                StorageReverts {
                    wiped: true,
                    previous_wipe: true,
                    slots: BTreeMap::from([(slot, RevertToSlot::Some(value))]),
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

#[cfg(test)]
mod storage_tests {
    use super::execution_state_to_plain_state_and_reverts;
    use crate::{
        test_utils::create_test_provider_factory, AccountReader, StorageTrieWriter, TrieWriter,
    };
    use alloy_primitives::{keccak256, map::HashMap, Address, B256, U256};
    use reth_db_api::{
        cursor::{DbCursorRO, DbCursorRW, DbDupCursorRO},
        models::{AccountBeforeTx, BlockNumberAddress},
        tables,
        transaction::{DbTx, DbTxMut},
    };
    use reth_ethereum_primitives::Receipt;
    use reth_execution_types::{
        EvmState, EvmStateChangeSink, ExecutionAccountChangeRef, ExecutionAccountInfo,
        ExecutionOutcome, ExecutionStorageChange,
    };
    use reth_primitives_traits::{Account, StorageEntry};
    use reth_storage_api::{
        DatabaseProviderFactory, HashedPostStateProvider, OriginalValuesKnown,
        PlainStorageChangeset, PlainStorageRevert, StateWriteConfig, StateWriter,
        StorageSettingsCache,
    };
    use reth_trie::{
        test_utils::{state_root, storage_root_prehashed},
        HashedPostState, HashedStorage, StateRoot, StorageRoot, StorageRootProgress,
    };
    use reth_trie_db::{
        DatabaseStateRoot, DatabaseStorageRoot, LegacyKeyAdapter, PackedKeyAdapter,
    };
    use std::{collections::BTreeMap, str::FromStr};

    #[derive(Default)]
    struct SlotUpdate {
        original_value: U256,
        present_value: U256,
    }

    impl SlotUpdate {
        fn changed(original_value: U256, present_value: U256, _transaction: u64) -> Self {
            Self { original_value, present_value }
        }
    }

    struct AccountUpdate {
        info: Account,
        created: bool,
        destroyed: bool,
        storage: HashMap<U256, SlotUpdate>,
    }

    fn native(account: Account) -> ExecutionAccountInfo {
        ExecutionAccountInfo {
            nonce: account.nonce,
            balance: account.balance,
            code_hash: account.get_bytecode_hash(),
            ..Default::default()
        }
    }

    #[derive(Default)]
    struct StateFixture {
        accounts: HashMap<Address, ExecutionAccountInfo>,
        block: EvmState,
        blocks: Vec<EvmState>,
    }

    impl StateFixture {
        fn insert_not_existing(&mut self, address: Address) {
            self.accounts.remove(&address);
        }
        fn insert_account(&mut self, address: Address, account: Account) {
            self.accounts.insert(address, native(account));
        }
        fn insert_account_with_storage(
            &mut self,
            address: Address,
            account: Account,
            _storage: HashMap<U256, U256>,
        ) {
            self.insert_account(address, account);
        }
        fn commit(&mut self, updates: HashMap<Address, AccountUpdate>) {
            for (address, update) in updates {
                let original = self.accounts.get(&address).cloned();
                let current = (!update.destroyed).then(|| native(update.info));
                self.block
                    .account(ExecutionAccountChangeRef {
                        address,
                        original: original.as_ref(),
                        current: current.as_ref(),
                        created: update.created,
                        selfdestructed: update.destroyed,
                    })
                    .unwrap();
                if update.destroyed {
                    self.accounts.remove(&address);
                } else {
                    self.accounts.insert(address, current.unwrap());
                    for (key, slot) in update.storage {
                        EvmStateChangeSink::storage(
                            &mut self.block,
                            ExecutionStorageChange {
                                address,
                                key,
                                original: slot.original_value,
                                current: slot.present_value,
                            },
                        )
                        .unwrap();
                    }
                }
            }
        }
        fn finish_block(&mut self) {
            self.blocks.push(core::mem::take(&mut self.block));
        }
        fn take_blocks(&mut self) -> Vec<EvmState> {
            core::mem::take(&mut self.blocks)
        }
        fn aggregate(&self) -> EvmState {
            reth_execution_types::ExecutionOutcomeState::from_block_states(self.blocks.clone())
                .into_execution_state()
        }
    }

    #[test]
    fn zeroed_entries_are_removed() {
        let provider_factory = create_test_provider_factory();

        let addresses = (0..10).map(|_| Address::random()).collect::<Vec<_>>();
        let destroyed_address = *addresses.first().unwrap();
        let destroyed_address_hashed = keccak256(destroyed_address);
        let slot = B256::with_last_byte(1);
        let hashed_slot = keccak256(slot);
        {
            let provider_rw = provider_factory.provider_rw().unwrap();
            let mut accounts_cursor =
                provider_rw.tx_ref().cursor_write::<tables::HashedAccounts>().unwrap();
            let mut storage_cursor =
                provider_rw.tx_ref().cursor_write::<tables::HashedStorages>().unwrap();

            for address in addresses {
                let hashed_address = keccak256(address);
                accounts_cursor
                    .insert(hashed_address, &Account { nonce: 1, ..Default::default() })
                    .unwrap();
                storage_cursor
                    .insert(
                        hashed_address,
                        &StorageEntry { key: hashed_slot, value: U256::from(1) },
                    )
                    .unwrap();
            }
            provider_rw.commit().unwrap();
        }

        let mut hashed_state = HashedPostState::default();
        hashed_state.accounts.insert(destroyed_address_hashed, None);
        hashed_state.storages.insert(
            destroyed_address_hashed,
            HashedStorage::from_iter([(hashed_slot, U256::ZERO)]),
        );

        let provider_rw = provider_factory.provider_rw().unwrap();
        assert!(matches!(provider_rw.write_hashed_state(&hashed_state.into_sorted()), Ok(())));
        provider_rw.commit().unwrap();

        let provider = provider_factory.provider().unwrap();
        assert_eq!(
            provider.tx_ref().get::<tables::HashedAccounts>(destroyed_address_hashed).unwrap(),
            None
        );
        assert_eq!(
            provider
                .tx_ref()
                .cursor_read::<tables::HashedStorages>()
                .unwrap()
                .seek_by_key_subkey(destroyed_address_hashed, hashed_slot)
                .unwrap(),
            None
        );
    }

    #[test]
    fn write_to_db_account_info() {
        let factory = create_test_provider_factory();
        let provider = factory.provider_rw().unwrap();

        let address_a = Address::ZERO;
        let address_b = Address::repeat_byte(0xff);

        let account_a = Account { balance: U256::from(1), nonce: 1, ..Default::default() };
        let account_b = Account { balance: U256::from(2), nonce: 2, ..Default::default() };
        let account_b_changed = Account { balance: U256::from(3), nonce: 3, ..Default::default() };

        let mut state = StateFixture::default();
        state.insert_not_existing(address_a);
        state.insert_account(address_b, account_b);

        // 0x00.. is created
        state.commit(HashMap::from_iter([(
            address_a,
            AccountUpdate {
                info: account_a,
                created: true,
                destroyed: false,
                storage: HashMap::default(),
            },
        )]));

        // 0xff.. is changed (balance + 1, nonce + 1)
        state.commit(HashMap::from_iter([(
            address_b,
            AccountUpdate {
                info: account_b_changed,
                created: false,
                destroyed: false,
                storage: HashMap::default(),
            },
        )]));

        state.finish_block();
        let blocks = state.take_blocks();
        let (plain_state, reverts) =
            execution_state_to_plain_state_and_reverts(&blocks[0], OriginalValuesKnown::Yes);
        assert!(plain_state.storage.is_empty());
        assert!(plain_state.contracts.is_empty());
        provider.write_state_changes(plain_state).expect("Could not write plain state to DB");

        assert_eq!(reverts.storage, [[]]);
        provider
            .write_state_reverts(reverts, 1, StateWriteConfig::default())
            .expect("Could not write reverts to DB");

        let reth_account_a = account_a;
        let reth_account_b = account_b;
        let reth_account_b_changed = account_b_changed;

        // Check plain state
        assert_eq!(
            provider.basic_account(&address_a).expect("Could not read account state"),
            Some(reth_account_a),
            "Account A state is wrong"
        );
        assert_eq!(
            provider.basic_account(&address_b).expect("Could not read account state"),
            Some(reth_account_b_changed),
            "Account B state is wrong"
        );

        // Check change set
        let mut changeset_cursor = provider
            .tx_ref()
            .cursor_dup_read::<tables::AccountChangeSets>()
            .expect("Could not open changeset cursor");
        assert_eq!(
            changeset_cursor.seek_exact(1).expect("Could not read account change set"),
            Some((1, AccountBeforeTx { address: address_a, info: None })),
            "Account A changeset is wrong"
        );
        assert_eq!(
            changeset_cursor.next_dup().expect("Changeset table is malformed"),
            Some((1, AccountBeforeTx { address: address_b, info: Some(reth_account_b) })),
            "Account B changeset is wrong"
        );

        let mut state = StateFixture::default();
        state.insert_account(address_b, account_b_changed);

        // 0xff.. is destroyed
        state.commit(HashMap::from_iter([(
            address_b,
            AccountUpdate {
                created: false,
                destroyed: true,
                info: account_b_changed,
                storage: HashMap::default(),
            },
        )]));

        state.finish_block();
        let blocks = state.take_blocks();
        let (plain_state, reverts) =
            execution_state_to_plain_state_and_reverts(&blocks[0], OriginalValuesKnown::Yes);
        // Account B selfdestructed so flag for it should be present.
        assert_eq!(
            plain_state.storage,
            [PlainStorageChangeset { address: address_b, wipe_storage: true, storage: vec![] }]
        );
        assert!(plain_state.contracts.is_empty());
        provider.write_state_changes(plain_state).expect("Could not write plain state to DB");

        assert_eq!(
            reverts.storage,
            [[PlainStorageRevert { address: address_b, wiped: true, storage_revert: vec![] }]]
        );
        provider
            .write_state_reverts(reverts, 2, StateWriteConfig::default())
            .expect("Could not write reverts to DB");

        // Check new plain state for account B
        assert_eq!(
            provider.basic_account(&address_b).expect("Could not read account state"),
            None,
            "Account B should be deleted"
        );

        // Check change set
        assert_eq!(
            changeset_cursor.seek_exact(2).expect("Could not read account change set"),
            Some((2, AccountBeforeTx { address: address_b, info: Some(reth_account_b_changed) })),
            "Account B changeset is wrong after deletion"
        );
    }

    #[test]
    fn write_to_db_storage() {
        let factory = create_test_provider_factory();
        let provider = factory.database_provider_rw().unwrap();

        let address_a = Address::ZERO;
        let address_b = Address::repeat_byte(0xff);

        let account_b = Account { balance: U256::from(2), nonce: 2, ..Default::default() };

        let mut state = StateFixture::default();
        state.insert_not_existing(address_a);
        state.insert_account_with_storage(
            address_b,
            account_b,
            HashMap::from_iter([(U256::from(1), U256::from(1))]),
        );

        state.commit(HashMap::from_iter([
            (
                address_a,
                AccountUpdate {
                    created: true,
                    destroyed: false,
                    info: Account::default(),
                    // 0x00 => 0 => 1
                    // 0x01 => 0 => 2
                    storage: HashMap::from_iter([
                        (
                            U256::from(0),
                            SlotUpdate { present_value: U256::from(1), ..Default::default() },
                        ),
                        (
                            U256::from(1),
                            SlotUpdate { present_value: U256::from(2), ..Default::default() },
                        ),
                    ]),
                },
            ),
            (
                address_b,
                AccountUpdate {
                    created: false,
                    destroyed: false,
                    info: account_b,
                    // 0x01 => 1 => 2
                    storage: HashMap::from_iter([(
                        U256::from(1),
                        SlotUpdate { present_value: U256::from(2), original_value: U256::from(1) },
                    )]),
                },
            ),
        ]));

        state.finish_block();

        let outcome = ExecutionOutcome::from_block_states(1, state.take_blocks(), Vec::new());
        provider
            .write_state(&outcome, OriginalValuesKnown::Yes, StateWriteConfig::default())
            .expect("Could not write bundle state to DB");

        // Check plain storage state
        let mut storage_cursor = provider
            .tx_ref()
            .cursor_dup_read::<tables::PlainStorageState>()
            .expect("Could not open plain storage state cursor");

        assert_eq!(
            storage_cursor.seek_exact(address_a).unwrap(),
            Some((address_a, StorageEntry { key: B256::ZERO, value: U256::from(1) })),
            "Slot 0 for account A should be 1"
        );
        assert_eq!(
            storage_cursor.next_dup().unwrap(),
            Some((
                address_a,
                StorageEntry { key: B256::from(U256::from(1).to_be_bytes()), value: U256::from(2) }
            )),
            "Slot 1 for account A should be 2"
        );
        assert_eq!(
            storage_cursor.next_dup().unwrap(),
            None,
            "Account A should only have 2 storage slots"
        );

        assert_eq!(
            storage_cursor.seek_exact(address_b).unwrap(),
            Some((
                address_b,
                StorageEntry { key: B256::from(U256::from(1).to_be_bytes()), value: U256::from(2) }
            )),
            "Slot 1 for account B should be 2"
        );
        assert_eq!(
            storage_cursor.next_dup().unwrap(),
            None,
            "Account B should only have 1 storage slot"
        );

        // Check change set
        let mut changeset_cursor = provider
            .tx_ref()
            .cursor_dup_read::<tables::StorageChangeSets>()
            .expect("Could not open storage changeset cursor");
        assert_eq!(
            changeset_cursor.seek_exact(BlockNumberAddress((1, address_a))).unwrap(),
            Some((
                BlockNumberAddress((1, address_a)),
                StorageEntry { key: B256::ZERO, value: U256::from(0) }
            )),
            "Slot 0 for account A should have changed from 0"
        );
        assert_eq!(
            changeset_cursor.next_dup().unwrap(),
            Some((
                BlockNumberAddress((1, address_a)),
                StorageEntry { key: B256::from(U256::from(1).to_be_bytes()), value: U256::from(0) }
            )),
            "Slot 1 for account A should have changed from 0"
        );
        assert_eq!(
            changeset_cursor.next_dup().unwrap(),
            None,
            "Account A should only be in the changeset 2 times"
        );

        assert_eq!(
            changeset_cursor.seek_exact(BlockNumberAddress((1, address_b))).unwrap(),
            Some((
                BlockNumberAddress((1, address_b)),
                StorageEntry { key: B256::from(U256::from(1).to_be_bytes()), value: U256::from(1) }
            )),
            "Slot 1 for account B should have changed from 1"
        );
        assert_eq!(
            changeset_cursor.next_dup().unwrap(),
            None,
            "Account B should only be in the changeset 1 time"
        );

        // Delete account A
        let mut state = StateFixture::default();
        state.insert_account(address_a, Account::default());

        state.commit(HashMap::from_iter([(
            address_a,
            AccountUpdate {
                created: false,
                destroyed: true,
                info: Account::default(),
                storage: HashMap::default(),
            },
        )]));

        state.finish_block();
        let outcome = ExecutionOutcome::from_block_states(2, state.take_blocks(), Vec::new());
        provider
            .write_state(&outcome, OriginalValuesKnown::Yes, StateWriteConfig::default())
            .expect("Could not write bundle state to DB");

        assert_eq!(
            storage_cursor.seek_exact(address_a).unwrap(),
            None,
            "Account A should have no storage slots after deletion"
        );

        assert_eq!(
            changeset_cursor.seek_exact(BlockNumberAddress((2, address_a))).unwrap(),
            Some((
                BlockNumberAddress((2, address_a)),
                StorageEntry { key: B256::ZERO, value: U256::from(1) }
            )),
            "Slot 0 for account A should have changed from 1 on deletion"
        );
        assert_eq!(
            changeset_cursor.next_dup().unwrap(),
            Some((
                BlockNumberAddress((2, address_a)),
                StorageEntry { key: B256::from(U256::from(1).to_be_bytes()), value: U256::from(2) }
            )),
            "Slot 1 for account A should have changed from 2 on deletion"
        );
        assert_eq!(
            changeset_cursor.next_dup().unwrap(),
            None,
            "Account A should only be in the changeset 2 times on deletion"
        );
    }

    #[test]
    fn write_to_db_multiple_selfdestructs() {
        let factory = create_test_provider_factory();
        let provider = factory.database_provider_rw().unwrap();

        let address1 = Address::random();
        let account_info = Account { nonce: 1, ..Default::default() };

        // Block #0: initial state.
        let mut init_state = StateFixture::default();
        init_state.insert_not_existing(address1);
        init_state.commit(HashMap::from_iter([(
            address1,
            AccountUpdate {
                info: account_info,
                created: true,
                destroyed: false,
                // 0x00 => 0 => 1
                // 0x01 => 0 => 2
                storage: HashMap::from_iter([
                    (U256::ZERO, SlotUpdate { present_value: U256::from(1), ..Default::default() }),
                    (
                        U256::from(1),
                        SlotUpdate { present_value: U256::from(2), ..Default::default() },
                    ),
                ]),
            },
        )]));
        init_state.finish_block();

        let outcome = ExecutionOutcome::from_block_states(0, init_state.take_blocks(), Vec::new());
        provider
            .write_state(&outcome, OriginalValuesKnown::Yes, StateWriteConfig::default())
            .expect("Could not write bundle state to DB");

        let mut state = StateFixture::default();
        state.insert_account_with_storage(
            address1,
            account_info,
            HashMap::from_iter([(U256::ZERO, U256::from(1)), (U256::from(1), U256::from(2))]),
        );

        // Block #1: change storage.
        state.commit(HashMap::from_iter([(
            address1,
            AccountUpdate {
                created: false,
                destroyed: false,
                info: account_info,
                // 0x00 => 1 => 2
                storage: HashMap::from_iter([(
                    U256::ZERO,
                    SlotUpdate { original_value: U256::from(1), present_value: U256::from(2) },
                )]),
            },
        )]));
        state.finish_block();

        // Block #2: destroy account.
        state.commit(HashMap::from_iter([(
            address1,
            AccountUpdate {
                created: false,
                destroyed: true,
                info: account_info,
                storage: HashMap::default(),
            },
        )]));
        state.finish_block();

        // Block #3: re-create account and change storage.
        state.commit(HashMap::from_iter([(
            address1,
            AccountUpdate {
                created: true,
                destroyed: false,
                info: account_info,
                storage: HashMap::default(),
            },
        )]));
        state.finish_block();

        // Block #4: change storage.
        state.commit(HashMap::from_iter([(
            address1,
            AccountUpdate {
                created: false,
                destroyed: false,
                info: account_info,
                // 0x00 => 0 => 2
                // 0x02 => 0 => 4
                // 0x06 => 0 => 6
                storage: HashMap::from_iter([
                    (U256::ZERO, SlotUpdate { present_value: U256::from(2), ..Default::default() }),
                    (
                        U256::from(2),
                        SlotUpdate { present_value: U256::from(4), ..Default::default() },
                    ),
                    (
                        U256::from(6),
                        SlotUpdate { present_value: U256::from(6), ..Default::default() },
                    ),
                ]),
            },
        )]));
        state.finish_block();

        // Block #5: Destroy account again.
        state.commit(HashMap::from_iter([(
            address1,
            AccountUpdate {
                created: false,
                destroyed: true,
                info: account_info,
                storage: HashMap::default(),
            },
        )]));
        state.finish_block();

        // Block #6: Create, change, destroy and re-create in the same block.
        state.commit(HashMap::from_iter([(
            address1,
            AccountUpdate {
                created: true,
                destroyed: false,
                info: account_info,
                storage: HashMap::default(),
            },
        )]));
        state.commit(HashMap::from_iter([(
            address1,
            AccountUpdate {
                created: false,
                destroyed: false,
                info: account_info,
                // 0x00 => 0 => 2
                storage: HashMap::from_iter([(
                    U256::ZERO,
                    SlotUpdate { present_value: U256::from(2), ..Default::default() },
                )]),
            },
        )]));
        state.commit(HashMap::from_iter([(
            address1,
            AccountUpdate {
                created: false,
                destroyed: true,
                info: account_info,
                storage: HashMap::default(),
            },
        )]));
        state.commit(HashMap::from_iter([(
            address1,
            AccountUpdate {
                created: true,
                destroyed: false,
                info: account_info,
                storage: HashMap::default(),
            },
        )]));
        state.finish_block();

        // Block #7: Change storage.
        state.commit(HashMap::from_iter([(
            address1,
            AccountUpdate {
                created: false,
                destroyed: false,
                info: account_info,
                // 0x00 => 0 => 9
                storage: HashMap::from_iter([(
                    U256::ZERO,
                    SlotUpdate { present_value: U256::from(9), ..Default::default() },
                )]),
            },
        )]));

        state.finish_block();

        let bundle = state.take_blocks();

        let outcome: ExecutionOutcome = ExecutionOutcome::from_block_states(1, bundle, Vec::new());
        provider
            .write_state(&outcome, OriginalValuesKnown::Yes, StateWriteConfig::default())
            .expect("Could not write bundle state to DB");

        let mut storage_changeset_cursor = provider
            .tx_ref()
            .cursor_dup_read::<tables::StorageChangeSets>()
            .expect("Could not open plain storage state cursor");
        let mut storage_changes = storage_changeset_cursor.walk_range(..).unwrap();

        // Iterate through all storage changes

        // Block <number>
        // <slot>: <expected value before>
        // ...

        // Block #0
        // 0x00: 0
        // 0x01: 0
        assert_eq!(
            storage_changes.next().transpose().unwrap(),
            Some((
                BlockNumberAddress((0, address1)),
                StorageEntry { key: B256::with_last_byte(0), value: U256::ZERO }
            ))
        );
        assert_eq!(
            storage_changes.next().transpose().unwrap(),
            Some((
                BlockNumberAddress((0, address1)),
                StorageEntry { key: B256::with_last_byte(1), value: U256::ZERO }
            ))
        );

        // Block #1
        // 0x00: 1
        assert_eq!(
            storage_changes.next().transpose().unwrap(),
            Some((
                BlockNumberAddress((1, address1)),
                StorageEntry { key: B256::with_last_byte(0), value: U256::from(1) }
            ))
        );

        // Block #2 (destroyed)
        // 0x00: 2
        // 0x01: 2
        assert_eq!(
            storage_changes.next().transpose().unwrap(),
            Some((
                BlockNumberAddress((2, address1)),
                StorageEntry { key: B256::with_last_byte(0), value: U256::from(2) }
            ))
        );
        assert_eq!(
            storage_changes.next().transpose().unwrap(),
            Some((
                BlockNumberAddress((2, address1)),
                StorageEntry { key: B256::with_last_byte(1), value: U256::from(2) }
            ))
        );

        // Block #3
        // no storage changes

        // Block #4
        // 0x00: 0
        // 0x02: 0
        // 0x06: 0
        assert_eq!(
            storage_changes.next().transpose().unwrap(),
            Some((
                BlockNumberAddress((4, address1)),
                StorageEntry { key: B256::with_last_byte(0), value: U256::ZERO }
            ))
        );
        assert_eq!(
            storage_changes.next().transpose().unwrap(),
            Some((
                BlockNumberAddress((4, address1)),
                StorageEntry { key: B256::with_last_byte(2), value: U256::ZERO }
            ))
        );
        assert_eq!(
            storage_changes.next().transpose().unwrap(),
            Some((
                BlockNumberAddress((4, address1)),
                StorageEntry { key: B256::with_last_byte(6), value: U256::ZERO }
            ))
        );

        // Block #5 (destroyed)
        // 0x00: 2
        // 0x02: 4
        // 0x06: 6
        assert_eq!(
            storage_changes.next().transpose().unwrap(),
            Some((
                BlockNumberAddress((5, address1)),
                StorageEntry { key: B256::with_last_byte(0), value: U256::from(2) }
            ))
        );
        assert_eq!(
            storage_changes.next().transpose().unwrap(),
            Some((
                BlockNumberAddress((5, address1)),
                StorageEntry { key: B256::with_last_byte(2), value: U256::from(4) }
            ))
        );
        assert_eq!(
            storage_changes.next().transpose().unwrap(),
            Some((
                BlockNumberAddress((5, address1)),
                StorageEntry { key: B256::with_last_byte(6), value: U256::from(6) }
            ))
        );

        // Block #6
        // no storage changes (only inter block changes)

        // Block #7
        // 0x00: 0
        assert_eq!(
            storage_changes.next().transpose().unwrap(),
            Some((
                BlockNumberAddress((7, address1)),
                StorageEntry { key: B256::with_last_byte(0), value: U256::ZERO }
            ))
        );
        assert_eq!(storage_changes.next().transpose().unwrap(), None);
    }

    #[test]
    fn storage_change_after_selfdestruct_within_block() {
        let factory = create_test_provider_factory();
        let provider = factory.database_provider_rw().unwrap();

        let address1 = Address::random();
        let account1 = Account { nonce: 1, ..Default::default() };

        // Block #0: initial state.
        let mut init_state = StateFixture::default();
        init_state.insert_not_existing(address1);
        init_state.commit(HashMap::from_iter([(
            address1,
            AccountUpdate {
                info: account1,
                created: true,
                destroyed: false,
                // 0x00 => 0 => 1
                // 0x01 => 0 => 2
                storage: HashMap::from_iter([
                    (U256::ZERO, SlotUpdate { present_value: U256::from(1), ..Default::default() }),
                    (
                        U256::from(1),
                        SlotUpdate { present_value: U256::from(2), ..Default::default() },
                    ),
                ]),
            },
        )]));
        init_state.finish_block();
        let outcome = ExecutionOutcome::from_block_states(0, init_state.take_blocks(), Vec::new());
        provider
            .write_state(&outcome, OriginalValuesKnown::Yes, StateWriteConfig::default())
            .expect("Could not write bundle state to DB");

        let mut state = StateFixture::default();
        state.insert_account_with_storage(
            address1,
            account1,
            HashMap::from_iter([(U256::ZERO, U256::from(1)), (U256::from(1), U256::from(2))]),
        );

        // Block #1: Destroy, re-create, change storage.
        state.commit(HashMap::from_iter([(
            address1,
            AccountUpdate {
                created: false,
                destroyed: true,
                info: account1,
                storage: HashMap::default(),
            },
        )]));

        state.commit(HashMap::from_iter([(
            address1,
            AccountUpdate {
                created: true,
                destroyed: false,
                info: account1,
                storage: HashMap::default(),
            },
        )]));

        state.commit(HashMap::from_iter([(
            address1,
            AccountUpdate {
                created: false,
                destroyed: false,
                info: account1,
                // 0x01 => 0 => 5
                storage: HashMap::from_iter([(
                    U256::from(1),
                    SlotUpdate { present_value: U256::from(5), ..Default::default() },
                )]),
            },
        )]));

        // Commit block #1 changes to the database.
        state.finish_block();
        let outcome = ExecutionOutcome::from_block_states(1, state.take_blocks(), Vec::new());
        provider
            .write_state(&outcome, OriginalValuesKnown::Yes, StateWriteConfig::default())
            .expect("Could not write bundle state to DB");

        let mut storage_changeset_cursor = provider
            .tx_ref()
            .cursor_dup_read::<tables::StorageChangeSets>()
            .expect("Could not open plain storage state cursor");
        let range = BlockNumberAddress::range(1..=1);
        let mut storage_changes = storage_changeset_cursor.walk_range(range).unwrap();

        assert_eq!(
            storage_changes.next().transpose().unwrap(),
            Some((
                BlockNumberAddress((1, address1)),
                StorageEntry { key: B256::with_last_byte(0), value: U256::from(1) }
            ))
        );
        assert_eq!(
            storage_changes.next().transpose().unwrap(),
            Some((
                BlockNumberAddress((1, address1)),
                StorageEntry { key: B256::with_last_byte(1), value: U256::from(2) }
            ))
        );
        assert_eq!(storage_changes.next().transpose().unwrap(), None);
    }

    #[test]
    fn revert_to_indices() {
        let mut base: ExecutionOutcome = ExecutionOutcome::new_empty(10);
        base.receipts = vec![vec![Receipt::default(); 2]; 7];

        let mut this = base.clone();
        assert!(this.revert_to(10));
        assert_eq!(this.receipts.len(), 1);

        let mut this = base.clone();
        assert!(!this.revert_to(9));
        assert_eq!(this.receipts.len(), 7);

        let mut this = base.clone();
        assert!(this.revert_to(15));
        assert_eq!(this.receipts.len(), 6);

        let mut this = base.clone();
        assert!(this.revert_to(16));
        assert_eq!(this.receipts.len(), 7);

        let mut this = base;
        assert!(!this.revert_to(17));
        assert_eq!(this.receipts.len(), 7);
    }

    #[test]
    fn bundle_state_state_root() {
        type PreState = BTreeMap<Address, (Account, BTreeMap<B256, U256>)>;
        let mut prestate: PreState = (0..10)
            .map(|key| {
                let account = Account { nonce: 1, balance: U256::from(key), bytecode_hash: None };
                let storage =
                    (1..11).map(|key| (B256::with_last_byte(key), U256::from(key))).collect();
                (Address::with_last_byte(key), (account, storage))
            })
            .collect();

        let provider_factory = create_test_provider_factory();
        let provider_rw = provider_factory.database_provider_rw().unwrap();

        // insert initial state to the database
        let tx = provider_rw.tx_ref();
        for (address, (account, storage)) in &prestate {
            let hashed_address = keccak256(address);
            tx.put::<tables::HashedAccounts>(hashed_address, *account).unwrap();
            for (slot, value) in storage {
                tx.put::<tables::HashedStorages>(
                    hashed_address,
                    StorageEntry { key: keccak256(slot), value: *value },
                )
                .unwrap();
            }
        }

        type TestStateRoot<'a, TX, A> = StateRoot<
            reth_trie_db::DatabaseTrieCursorFactory<&'a TX, A>,
            reth_trie_db::DatabaseHashedCursorFactory<&'a TX>,
        >;
        let is_v2 = provider_rw.cached_storage_settings().is_v2();
        let (_, updates) = if is_v2 {
            TestStateRoot::<_, PackedKeyAdapter>::from_tx(tx).root_with_updates().unwrap()
        } else {
            TestStateRoot::<_, LegacyKeyAdapter>::from_tx(tx).root_with_updates().unwrap()
        };
        provider_rw.write_trie_updates(updates).unwrap();

        let mut state = StateFixture::default();

        let assert_state_root = |state: &StateFixture, expected: &PreState, msg| {
            let overlay_root = if is_v2 {
                TestStateRoot::<_, PackedKeyAdapter>::overlay_root(
                    tx,
                    &provider_rw
                        .latest()
                        .hashed_post_state(&state.aggregate())
                        .unwrap()
                        .into_sorted(),
                )
                .unwrap()
            } else {
                TestStateRoot::<_, LegacyKeyAdapter>::overlay_root(
                    tx,
                    &provider_rw
                        .latest()
                        .hashed_post_state(&state.aggregate())
                        .unwrap()
                        .into_sorted(),
                )
                .unwrap()
            };
            assert_eq!(
                overlay_root,
                state_root(expected.clone().into_iter().map(|(address, (account, storage))| (
                    address,
                    (account, storage.into_iter())
                ))),
                "{msg}"
            );
        };

        // database only state root is correct
        assert_state_root(&state, &prestate, "empty");

        // destroy account 1
        let address1 = Address::with_last_byte(1);
        let account1_old = prestate.remove(&address1).unwrap();
        state.insert_account(address1, account1_old.0);
        state.commit(HashMap::from_iter([(
            address1,
            AccountUpdate {
                created: false,
                destroyed: true,
                info: Account::default(),
                storage: HashMap::default(),
            },
        )]));
        state.finish_block();
        assert_state_root(&state, &prestate, "destroyed account");

        // change slot 2 in account 2
        let address2 = Address::with_last_byte(2);
        let slot2 = U256::from(2);
        let slot2_key = B256::from(slot2);
        let account2 = prestate.get_mut(&address2).unwrap();
        let account2_slot2_old_value = *account2.1.get(&slot2_key).unwrap();
        state.insert_account_with_storage(
            address2,
            account2.0,
            HashMap::from_iter([(slot2, account2_slot2_old_value)]),
        );

        let account2_slot2_new_value = U256::from(100);
        account2.1.insert(slot2_key, account2_slot2_new_value);
        state.commit(HashMap::from_iter([(
            address2,
            AccountUpdate {
                created: false,
                destroyed: false,
                info: account2.0,
                storage: HashMap::from_iter([(
                    slot2,
                    SlotUpdate::changed(account2_slot2_old_value, account2_slot2_new_value, 0),
                )]),
            },
        )]));
        state.finish_block();
        assert_state_root(&state, &prestate, "changed storage");

        // change balance of account 3
        let address3 = Address::with_last_byte(3);
        let account3 = prestate.get_mut(&address3).unwrap();
        state.insert_account(address3, account3.0);

        account3.0.balance = U256::from(24);
        state.commit(HashMap::from_iter([(
            address3,
            AccountUpdate {
                created: false,
                destroyed: false,
                info: account3.0,
                storage: HashMap::default(),
            },
        )]));
        state.finish_block();
        assert_state_root(&state, &prestate, "changed balance");

        // change nonce of account 4
        let address4 = Address::with_last_byte(4);
        let account4 = prestate.get_mut(&address4).unwrap();
        state.insert_account(address4, account4.0);

        account4.0.nonce = 128;
        state.commit(HashMap::from_iter([(
            address4,
            AccountUpdate {
                created: false,
                destroyed: false,
                info: account4.0,
                storage: HashMap::default(),
            },
        )]));
        state.finish_block();
        assert_state_root(&state, &prestate, "changed nonce");

        // recreate account 1
        let account1_new =
            Account { nonce: 56, balance: U256::from(123), bytecode_hash: Some(B256::random()) };
        prestate.insert(address1, (account1_new, BTreeMap::default()));
        state.commit(HashMap::from_iter([(
            address1,
            AccountUpdate {
                created: true,
                destroyed: false,
                info: account1_new,
                storage: HashMap::default(),
            },
        )]));
        state.finish_block();
        assert_state_root(&state, &prestate, "recreated");

        // update storage for account 1
        let slot20 = U256::from(20);
        let slot20_key = B256::from(slot20);
        let account1_slot20_value = U256::from(12345);
        prestate.get_mut(&address1).unwrap().1.insert(slot20_key, account1_slot20_value);
        state.commit(HashMap::from_iter([(
            address1,
            AccountUpdate {
                created: true,
                destroyed: false,
                info: account1_new,
                storage: HashMap::from_iter([(
                    slot20,
                    SlotUpdate::changed(U256::ZERO, account1_slot20_value, 0),
                )]),
            },
        )]));
        state.finish_block();
        assert_state_root(&state, &prestate, "recreated changed storage");
    }

    #[test]
    fn prepend_state() {
        let address1 = Address::random();
        let address2 = Address::random();

        let account1 = Account { nonce: 1, ..Default::default() };
        let account1_changed = Account { nonce: 1, ..Default::default() };
        let account2 = Account { nonce: 1, ..Default::default() };

        let present_state = reth_execution_types::execution_state_from_init(
            [(address1, (None, Some(account1_changed), BTreeMap::new()))],
            [],
        );
        let previous_state = reth_execution_types::execution_state_from_init(
            [
                (address1, (None, Some(account1), BTreeMap::new())),
                (address2, (None, Some(account2), BTreeMap::new())),
            ],
            [],
        );
        let mut test: ExecutionOutcome =
            ExecutionOutcome::from_block_states(2, [present_state], Vec::new());
        test.receipts = vec![vec![Receipt::default(); 2]];
        test.prepend_state(previous_state);
        assert_eq!(test.receipts.len(), 1);
        assert_eq!(test.execution_state_ref().accounts().count(), 2);
        assert_eq!(test.block_reverts().len(), 1);
        assert_eq!(test.account_state(&address1).unwrap().current, Some(native(account1_changed)));
        assert_eq!(test.account_state(&address2).unwrap().current, Some(native(account2)));
    }

    #[test]
    fn hashed_state_storage_root() {
        let address = Address::random();
        let hashed_address = keccak256(address);
        let provider_factory = create_test_provider_factory();
        let provider_rw = provider_factory.provider_rw().unwrap();
        let tx = provider_rw.tx_ref();

        // insert initial account storage
        let init_storage = HashedStorage::from_iter(
            [
                "50000000000000000000000000000004253371b55351a08cb3267d4d265530b6",
                "512428ed685fff57294d1a9cbb147b18ae5db9cf6ae4b312fa1946ba0561882e",
                "51e6784c736ef8548f856909870b38e49ef7a4e3e77e5e945e0d5e6fcaa3037f",
            ]
            .into_iter()
            .map(|str| (B256::from_str(str).unwrap(), U256::from(1))),
        );
        let mut state = HashedPostState::default();
        state.storages.insert(hashed_address, init_storage.clone());
        provider_rw.write_hashed_state(&state.clone().into_sorted()).unwrap();

        // calculate database storage root and write intermediate storage nodes.
        let progress = if provider_rw.cached_storage_settings().is_v2() {
            TestStorageRoot::<_, PackedKeyAdapter>::from_tx_hashed(tx, hashed_address)
                .with_no_threshold()
                .calculate(true)
                .unwrap()
        } else {
            TestStorageRoot::<_, LegacyKeyAdapter>::from_tx_hashed(tx, hashed_address)
                .with_no_threshold()
                .calculate(true)
                .unwrap()
        };
        let StorageRootProgress::Complete(storage_root, _, storage_updates) = progress else {
            panic!("no threshold for root");
        };
        assert_eq!(storage_root, storage_root_prehashed(init_storage.storage.clone()));
        assert!(!storage_updates.is_empty());
        provider_rw
            .write_storage_trie_updates_sorted(core::iter::once((
                &hashed_address,
                &storage_updates.into_sorted(),
            )))
            .unwrap();

        // destroy the storage and re-create with new slots
        let mut updated_storage = HashedStorage::from_iter(
            [
                "00deb8486ad8edccfdedfc07109b3667b38a03a8009271aac250cce062d90917",
                "88d233b7380bb1bcdc866f6871c94685848f54cf0ee033b1480310b4ddb75fc9",
            ]
            .into_iter()
            .map(|str| (B256::from_str(str).unwrap(), U256::from(1))),
        );
        updated_storage
            .storage
            .extend(init_storage.storage.keys().map(|hashed_slot| (*hashed_slot, U256::ZERO)));
        let mut state = HashedPostState::default();
        state.storages.insert(hashed_address, updated_storage.clone());
        provider_rw.write_hashed_state(&state.clone().into_sorted()).unwrap();

        // re-calculate database storage root
        type TestStorageRoot<'a, TX, A> = StorageRoot<
            reth_trie_db::DatabaseTrieCursorFactory<&'a TX, A>,
            reth_trie_db::DatabaseHashedCursorFactory<&'a TX>,
        >;
        let is_v2 = provider_rw.cached_storage_settings().is_v2();
        let storage_root = if is_v2 {
            TestStorageRoot::<_, PackedKeyAdapter>::overlay_root(
                tx,
                address,
                updated_storage.clone(),
            )
            .unwrap()
        } else {
            TestStorageRoot::<_, LegacyKeyAdapter>::overlay_root(
                tx,
                address,
                updated_storage.clone(),
            )
            .unwrap()
        };
        assert_eq!(
            storage_root,
            storage_root_prehashed(
                updated_storage.storage.into_iter().filter(|(_, value)| !value.is_zero())
            )
        );
    }
}
