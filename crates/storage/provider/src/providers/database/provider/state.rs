//! Execution state, hashed state, and trie writes and unwinds.

use super::DatabaseProvider;
use crate::{
    changesets_utils::StorageRevertsIter, providers::NodeTypesForProvider, BlockNumReader,
    BundleStateInit, ChangeSetReader, EitherWriter, HashingWriter, OriginalValuesKnown,
    ProviderError, ReceiptProvider, RevertsInit, StateWriter, StaticFileProviderFactory,
    StaticFileWriter, StorageTrieWriter, TrieWriter,
};
use alloy_consensus::TxReceipt;
use alloy_primitives::{
    keccak256,
    map::{hash_map, AddressSet, B256Map, HashMap},
    Address, BlockNumber, StorageKey, StorageValue, B256,
};
use itertools::Itertools;
use rayon::slice::ParallelSliceMut;
use reth_db_api::{
    cursor::{DbCursorRO, DbCursorRW, DbDupCursorRO, DbDupCursorRW},
    models::{AccountBeforeTx, BlockNumberAddress, StorageBeforeTx},
    tables,
    transaction::{DbTx, DbTxMut},
};
use reth_execution_types::ExecutionOutcome;
use reth_node_types::{NodeTypes, ReceiptTy};
use reth_primitives_traits::{Account, Bytecode, StorageEntry};
use reth_prune_types::PruneMode;
use reth_static_file_types::StaticFileSegment;
use reth_storage_api::{
    BlockBodyIndicesProvider, DBProvider, StateWriteConfig, StorageChangeSetReader,
    StorageSettingsCache, WriteStateInput,
};
use reth_storage_errors::provider::ProviderResult;
use reth_trie::{
    bundle_state::{PlainStateReverts, PlainStorageChangeset, PlainStorageRevert, StateChangeset},
    updates::{StorageTrieUpdatesSorted, TrieUpdatesSorted},
    HashedPostStateSorted,
};
use reth_trie_db::{DatabaseStorageTrieCursor, TrieTableAdapter};
use std::{
    collections::{BTreeMap, BTreeSet},
    ops::RangeBounds,
};
use tracing::instrument;

impl<TX: DbTx + 'static, N: NodeTypesForProvider> DatabaseProvider<TX, N> {
    /// Populate a [`BundleStateInit`] and [`RevertsInit`] using cursors over the
    /// [`tables::PlainAccountState`] and [`tables::PlainStorageState`] tables, based on the given
    /// storage and account changesets.
    #[allow(clippy::clone_on_copy)]
    pub(crate) fn populate_bundle_state(
        &self,
        account_changeset: Vec<(u64, AccountBeforeTx)>,
        storage_changeset: Vec<(BlockNumberAddress, StorageEntry)>,
        mut get_account: impl FnMut(Address) -> ProviderResult<Option<Account>>,
        mut get_storage: impl FnMut(Address, StorageKey) -> ProviderResult<Option<StorageValue>>,
    ) -> ProviderResult<(BundleStateInit, RevertsInit)> {
        // iterate previous value and get plain state value to create changeset
        // Double option around Account represent if Account state is know (first option) and
        // account is removed (Second Option)
        let mut state: BundleStateInit = HashMap::default();

        // This is not working for blocks that are not at tip. as plain state is not the last
        // state of end range. We should rename the functions or add support to access
        // History state. Accessing history state can be tricky but we are not gaining
        // anything.

        let mut reverts: RevertsInit = HashMap::default();

        // add account changeset changes
        for (block_number, account_before) in account_changeset.into_iter().rev() {
            let AccountBeforeTx { info: old_info, address } = account_before;
            match state.entry(address) {
                hash_map::Entry::Vacant(entry) => {
                    let new_info = get_account(address)?;
                    entry.insert((old_info.clone(), new_info, HashMap::default()));
                }
                hash_map::Entry::Occupied(mut entry) => {
                    // overwrite old account state.
                    entry.get_mut().0 = old_info.clone();
                }
            }
            // insert old info into reverts.
            reverts.entry(block_number).or_default().entry(address).or_default().0 = Some(old_info);
        }

        // add storage changeset changes
        for (block_and_address, old_storage) in storage_changeset.into_iter().rev() {
            let BlockNumberAddress((block_number, address)) = block_and_address;
            // get account state or insert from plain state.
            let account_state = match state.entry(address) {
                hash_map::Entry::Vacant(entry) => {
                    let present_info = get_account(address)?;
                    entry.insert((present_info.clone(), present_info, HashMap::default()))
                }
                hash_map::Entry::Occupied(entry) => entry.into_mut(),
            };

            // match storage.
            match account_state.2.entry(old_storage.key) {
                hash_map::Entry::Vacant(entry) => {
                    let new_storage = get_storage(address, old_storage.key)?.unwrap_or_default();
                    entry.insert((old_storage.value, new_storage));
                }
                hash_map::Entry::Occupied(mut entry) => {
                    entry.get_mut().0 = old_storage.value;
                }
            };

            reverts
                .entry(block_number)
                .or_default()
                .entry(address)
                .or_default()
                .1
                .push(old_storage);
        }

        Ok((state, reverts))
    }

    /// Invokes [`populate_bundle_state`](Self::populate_bundle_state) with the given plain state
    /// cursors.
    fn populate_bundle_state_plain(
        &self,
        account_changeset: Vec<(u64, AccountBeforeTx)>,
        storage_changeset: Vec<(BlockNumberAddress, StorageEntry)>,
        plain_accounts_cursor: &mut impl DbCursorRO<tables::PlainAccountState>,
        plain_storage_cursor: &mut impl DbDupCursorRO<tables::PlainStorageState>,
    ) -> ProviderResult<(BundleStateInit, RevertsInit)> {
        self.populate_bundle_state(
            account_changeset,
            storage_changeset,
            |address| Ok(plain_accounts_cursor.seek_exact(address)?.map(|kv| kv.1)),
            |address, storage_key| {
                Ok(plain_storage_cursor
                    .seek_by_key_subkey(address, storage_key)?
                    .filter(|s| s.key == storage_key)
                    .map(|s| s.value))
            },
        )
    }

    /// Like [`populate_bundle_state`](Self::populate_bundle_state), but reads current values from
    /// `HashedAccounts`/`HashedStorages`. Addresses and storage keys are hashed via `keccak256`
    /// for DB lookups. The output `BundleStateInit`/`RevertsInit` structures remain keyed by
    /// plain address and plain storage key.
    fn populate_bundle_state_hashed(
        &self,
        account_changeset: Vec<(u64, AccountBeforeTx)>,
        storage_changeset: Vec<(BlockNumberAddress, StorageEntry)>,
        hashed_accounts_cursor: &mut impl DbCursorRO<tables::HashedAccounts>,
        hashed_storage_cursor: &mut impl DbDupCursorRO<tables::HashedStorages>,
    ) -> ProviderResult<(BundleStateInit, RevertsInit)> {
        self.populate_bundle_state(
            account_changeset,
            storage_changeset,
            |address| Ok(hashed_accounts_cursor.seek_exact(keccak256(address))?.map(|kv| kv.1)),
            |address, storage_key| {
                let hashed_storage_key = keccak256(storage_key);
                Ok(hashed_storage_cursor
                    .seek_by_key_subkey(keccak256(address), hashed_storage_key)?
                    .filter(|s| s.key == hashed_storage_key)
                    .map(|s| s.value))
            },
        )
    }
}

impl<TX: DbTx + DbTxMut + 'static, N: NodeTypesForProvider> DatabaseProvider<TX, N> {
    /// Writes bytecodes to MDBX.
    fn write_bytecodes(
        &self,
        bytecodes: impl IntoIterator<Item = (B256, Bytecode)>,
    ) -> ProviderResult<()> {
        let mut bytecodes_cursor = self.tx_ref().cursor_write::<tables::Bytecodes>()?;
        for (hash, bytecode) in bytecodes {
            bytecodes_cursor.upsert(hash, &bytecode)?;
        }
        Ok(())
    }
}

impl<TX: DbTxMut + DbTx + 'static, N: NodeTypesForProvider> StateWriter
    for DatabaseProvider<TX, N>
{
    type Receipt = ReceiptTy<N>;

    #[instrument(level = "debug", target = "providers::db", skip_all)]
    fn write_state<'a>(
        &self,
        execution_outcome: impl Into<WriteStateInput<'a, Self::Receipt>>,
        is_value_known: OriginalValuesKnown,
        config: StateWriteConfig,
    ) -> ProviderResult<()> {
        let execution_outcome = execution_outcome.into();

        if self.cached_storage_settings().use_hashed_state() &&
            !config.write_receipts &&
            !config.write_account_changesets &&
            !config.write_storage_changesets
        {
            // In storage v2 with all outputs directed to static files, plain state and changesets
            // are written elsewhere. Only bytecodes need MDBX writes, so skip the expensive
            // to_plain_state_and_reverts conversion that iterates all accounts and storage.
            self.write_bytecodes(
                execution_outcome.state().contracts.iter().map(|(h, b)| (*h, Bytecode(b.clone()))),
            )?;
            return Ok(());
        }

        let first_block = execution_outcome.first_block();
        let (plain_state, reverts) =
            execution_outcome.state().to_plain_state_and_reverts(is_value_known);

        self.write_state_reverts(reverts, first_block, config)?;
        self.write_state_changes(plain_state)?;

        if !config.write_receipts {
            return Ok(());
        }

        let block_count = execution_outcome.len() as u64;
        let last_block = execution_outcome.last_block();
        let block_range = first_block..=last_block;

        let tip = self.last_block_number()?.max(last_block);

        // Fetch the first transaction number for each block in the range
        let block_indices: Vec<_> = self
            .block_body_indices_range(block_range)?
            .into_iter()
            .map(|b| b.first_tx_num)
            .collect();

        // Ensure all expected blocks are present.
        if block_indices.len() < block_count as usize {
            let missing_blocks = block_count - block_indices.len() as u64;
            return Err(ProviderError::BlockBodyIndicesNotFound(
                last_block.saturating_sub(missing_blocks - 1),
            ));
        }

        let mut receipts_writer = EitherWriter::new_receipts(self, first_block)?;

        let has_contract_log_filter = !self.prune_modes.receipts_log_filter.is_empty();
        let contract_log_pruner = self.prune_modes.receipts_log_filter.group_by_block(tip, None)?;

        // All receipts from the last 128 blocks are required for blockchain tree, even with
        // [`PruneSegment::ContractLogs`].
        //
        // Receipts can only be skipped if we're dealing with legacy nodes that write them to
        // Database, OR if receipts_in_static_files is enabled but no receipts exist in static
        // files yet. Once receipts exist in static files, we must continue writing to maintain
        // continuity and have no gaps.
        let prunable_receipts = (EitherWriter::receipts_destination(self).is_database() ||
            self.static_file_provider()
                .get_highest_static_file_tx(StaticFileSegment::Receipts)
                .is_none()) &&
            PruneMode::Distance(self.minimum_pruning_distance).should_prune(first_block, tip);

        // Prepare set of addresses which logs should not be pruned.
        let mut allowed_addresses: AddressSet = AddressSet::default();
        for (_, addresses) in contract_log_pruner.range(..first_block) {
            allowed_addresses.extend(addresses.iter().copied());
        }

        for (idx, (receipts, first_tx_index)) in
            execution_outcome.receipts().zip(block_indices).enumerate()
        {
            let block_number = first_block + idx as u64;

            // Increment block number for receipts static file writer
            receipts_writer.increment_block(block_number)?;

            // Skip writing receipts if pruning configuration requires us to.
            if prunable_receipts &&
                self.prune_modes
                    .receipts
                    .is_some_and(|mode| mode.should_prune(block_number, tip))
            {
                continue
            }

            // If there are new addresses to retain after this block number, track them
            if let Some(new_addresses) = contract_log_pruner.get(&block_number) {
                allowed_addresses.extend(new_addresses.iter().copied());
            }

            for (idx, receipt) in receipts.iter().enumerate() {
                let receipt_idx = first_tx_index + idx as u64;
                // Skip writing receipt if log filter is active and it does not have any logs to
                // retain
                if prunable_receipts &&
                    has_contract_log_filter &&
                    !receipt.logs().iter().any(|log| allowed_addresses.contains(&log.address))
                {
                    continue
                }

                receipts_writer.append_receipt(receipt_idx, receipt)?;
            }
        }

        Ok(())
    }

    fn write_state_reverts(
        &self,
        reverts: PlainStateReverts,
        first_block: BlockNumber,
        config: StateWriteConfig,
    ) -> ProviderResult<()> {
        // Write storage changes
        if config.write_storage_changesets {
            tracing::trace!("Writing storage changes");
            let mut storages_cursor =
                self.tx_ref().cursor_dup_write::<tables::PlainStorageState>()?;
            for (block_index, mut storage_changes) in reverts.storage.into_iter().enumerate() {
                let block_number = first_block + block_index as BlockNumber;

                tracing::trace!(block_number, "Writing block change");
                // sort changes by address.
                storage_changes.par_sort_unstable_by_key(|a| a.address);
                let total_changes =
                    storage_changes.iter().map(|change| change.storage_revert.len()).sum();
                let mut changeset = Vec::with_capacity(total_changes);
                for PlainStorageRevert { address, wiped, storage_revert } in storage_changes {
                    let mut storage = storage_revert
                        .into_iter()
                        .map(|(k, v)| (B256::from(k), v))
                        .collect::<Vec<_>>();
                    // sort storage slots by key.
                    storage.par_sort_unstable_by_key(|a| a.0);

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
                    EitherWriter::new_storage_changesets(self, block_number)?;
                storage_changesets_writer.append_storage_changeset(block_number, changeset)?;
            }
        }

        if !config.write_account_changesets {
            return Ok(());
        }

        // Write account changes
        tracing::trace!(?first_block, "Writing account changes");
        for (block_index, account_block_reverts) in reverts.accounts.into_iter().enumerate() {
            let block_number = first_block + block_index as BlockNumber;
            let changeset = account_block_reverts
                .into_iter()
                .map(|(address, info)| AccountBeforeTx { address, info: info.map(Into::into) })
                .collect::<Vec<_>>();
            let mut account_changesets_writer =
                EitherWriter::new_account_changesets(self, block_number)?;

            account_changesets_writer.append_account_changeset(block_number, changeset)?;
        }

        Ok(())
    }

    fn write_state_changes(&self, mut changes: StateChangeset) -> ProviderResult<()> {
        // sort all entries so they can be written to database in more performant way.
        // and take smaller memory footprint.
        changes.accounts.par_sort_by_key(|a| a.0);
        changes.storage.par_sort_by_key(|a| a.address);
        changes.contracts.par_sort_by_key(|a| a.0);

        if !self.cached_storage_settings().use_hashed_state() {
            // Write new account state
            tracing::trace!(len = changes.accounts.len(), "Writing new account state");
            let mut accounts_cursor = self.tx_ref().cursor_write::<tables::PlainAccountState>()?;
            // write account to database.
            for (address, account) in changes.accounts {
                if let Some(account) = account {
                    tracing::trace!(?address, "Updating plain state account");
                    accounts_cursor.upsert(address, &account.into())?;
                } else if accounts_cursor.seek_exact(address)?.is_some() {
                    tracing::trace!(?address, "Deleting plain state account");
                    accounts_cursor.delete_current()?;
                }
            }

            // Write new storage state and wipe storage if needed.
            tracing::trace!(len = changes.storage.len(), "Writing new storage state");
            let mut storages_cursor =
                self.tx_ref().cursor_dup_write::<tables::PlainStorageState>()?;
            for PlainStorageChangeset { address, wipe_storage, storage } in changes.storage {
                // Wiping of storage.
                if wipe_storage && storages_cursor.seek_exact(address)?.is_some() {
                    storages_cursor.delete_current_duplicates()?;
                }
                // cast storages to B256.
                let mut storage = storage
                    .into_iter()
                    .map(|(k, value)| StorageEntry { key: k.into(), value })
                    .collect::<Vec<_>>();
                // sort storage slots by key.
                storage.par_sort_unstable_by_key(|a| a.key);

                for entry in storage {
                    tracing::trace!(?address, ?entry.key, "Updating plain state storage");
                    if let Some(db_entry) =
                        storages_cursor.seek_by_key_subkey(address, entry.key)? &&
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

        // Write bytecode
        tracing::trace!(len = changes.contracts.len(), "Writing bytecodes");
        self.write_bytecodes(
            changes.contracts.into_iter().map(|(hash, bytecode)| (hash, Bytecode(bytecode))),
        )?;

        Ok(())
    }

    #[instrument(level = "debug", target = "providers::db", skip_all)]
    fn write_hashed_state(&self, hashed_state: &HashedPostStateSorted) -> ProviderResult<()> {
        // Write hashed account updates.
        let mut hashed_accounts_cursor = self.tx_ref().cursor_write::<tables::HashedAccounts>()?;
        for (hashed_address, account) in hashed_state.accounts() {
            if let Some(account) = account {
                hashed_accounts_cursor.upsert(*hashed_address, account)?;
            } else if hashed_accounts_cursor.seek_exact(*hashed_address)?.is_some() {
                hashed_accounts_cursor.delete_current()?;
            }
        }

        // Write hashed storage changes.
        let sorted_storages = hashed_state.account_storages().iter().sorted_by_key(|(key, _)| *key);
        let mut hashed_storage_cursor =
            self.tx_ref().cursor_dup_write::<tables::HashedStorages>()?;
        for (hashed_address, storage) in sorted_storages {
            for (hashed_slot, value) in storage.storage_slots_ref() {
                let entry = StorageEntry { key: *hashed_slot, value: *value };

                if let Some(db_entry) =
                    hashed_storage_cursor.seek_by_key_subkey(*hashed_address, entry.key)? &&
                    db_entry.key == entry.key
                {
                    hashed_storage_cursor.delete_current()?;
                }

                if !entry.value.is_zero() {
                    hashed_storage_cursor.upsert(*hashed_address, &entry)?;
                }
            }
        }

        Ok(())
    }

    /// Remove the last N blocks of state.
    ///
    /// The latest state will be unwound
    ///
    /// 1. Read the retained block's [`BlockBodyIndices`][tables::BlockBodyIndices] entry to get the
    ///    first transaction id to remove.
    /// 2. Iterate over the [`StorageChangeSets`][tables::StorageChangeSets] table and the
    ///    [`AccountChangeSets`][tables::AccountChangeSets] tables in reverse order to reconstruct
    ///    the changesets.
    ///    - In order to have both the old and new values in the changesets, we also access the
    ///      plain state tables.
    /// 3. While iterating over the changeset tables, if we encounter a new account or storage slot,
    ///    we:
    ///     1. Take the old value from the changeset
    ///     2. Take the new value from the plain state
    ///     3. Save the old value to the local state
    /// 4. While iterating over the changeset tables, if we encounter an account/storage slot we
    ///    have seen before we:
    ///     1. Take the old value from the changeset
    ///     2. Take the new value from the local state
    ///     3. Set the local state to the value in the changeset
    fn remove_state_above(&self, block: BlockNumber) -> ProviderResult<()> {
        let range = block + 1..=self.last_block_number()?;

        if range.is_empty() {
            return Ok(());
        }

        // get transaction receipts
        let from_transaction_num = self.next_tx_num_after_block(block)?;

        let storage_range = BlockNumberAddress::range(range.clone());
        let storage_changeset = if self.cached_storage_settings().storage_v2 {
            let changesets = self.storage_changesets_range(range.clone())?;
            let mut changeset_writer =
                self.static_file_provider.latest_writer(StaticFileSegment::StorageChangeSets)?;
            changeset_writer.prune_storage_changesets(block)?;
            changesets
        } else {
            self.take::<tables::StorageChangeSets>(storage_range)?.into_iter().collect()
        };
        let account_changeset = if self.cached_storage_settings().storage_v2 {
            let changesets = self.account_changesets_range(range)?;
            let mut changeset_writer =
                self.static_file_provider.latest_writer(StaticFileSegment::AccountChangeSets)?;
            changeset_writer.prune_account_changesets(block)?;
            changesets
        } else {
            self.take::<tables::AccountChangeSets>(range)?
        };

        if self.cached_storage_settings().use_hashed_state() {
            let mut hashed_accounts_cursor = self.tx.cursor_write::<tables::HashedAccounts>()?;
            let mut hashed_storage_cursor = self.tx.cursor_dup_write::<tables::HashedStorages>()?;

            let (state, _) = self.populate_bundle_state_hashed(
                account_changeset,
                storage_changeset,
                &mut hashed_accounts_cursor,
                &mut hashed_storage_cursor,
            )?;

            for (address, (old_account, new_account, storage)) in &state {
                if old_account != new_account {
                    let hashed_address = keccak256(address);
                    let existing_entry = hashed_accounts_cursor.seek_exact(hashed_address)?;
                    if let Some(account) = old_account {
                        hashed_accounts_cursor.upsert(hashed_address, account)?;
                    } else if existing_entry.is_some() {
                        hashed_accounts_cursor.delete_current()?;
                    }
                }

                for (storage_key, (old_storage_value, _new_storage_value)) in storage {
                    let hashed_address = keccak256(address);
                    let hashed_storage_key = keccak256(storage_key);
                    let storage_entry =
                        StorageEntry { key: hashed_storage_key, value: *old_storage_value };
                    if hashed_storage_cursor
                        .seek_by_key_subkey(hashed_address, hashed_storage_key)?
                        .is_some_and(|s| s.key == hashed_storage_key)
                    {
                        hashed_storage_cursor.delete_current()?
                    }

                    if !old_storage_value.is_zero() {
                        hashed_storage_cursor.upsert(hashed_address, &storage_entry)?;
                    }
                }
            }
        } else {
            // This is not working for blocks that are not at tip. as plain state is not the last
            // state of end range. We should rename the functions or add support to access
            // History state. Accessing history state can be tricky but we are not gaining
            // anything.
            let mut plain_accounts_cursor = self.tx.cursor_write::<tables::PlainAccountState>()?;
            let mut plain_storage_cursor =
                self.tx.cursor_dup_write::<tables::PlainStorageState>()?;

            let (state, _) = self.populate_bundle_state_plain(
                account_changeset,
                storage_changeset,
                &mut plain_accounts_cursor,
                &mut plain_storage_cursor,
            )?;

            for (address, (old_account, new_account, storage)) in &state {
                if old_account != new_account {
                    let existing_entry = plain_accounts_cursor.seek_exact(*address)?;
                    if let Some(account) = old_account {
                        plain_accounts_cursor.upsert(*address, account)?;
                    } else if existing_entry.is_some() {
                        plain_accounts_cursor.delete_current()?;
                    }
                }

                for (storage_key, (old_storage_value, _new_storage_value)) in storage {
                    let storage_entry =
                        StorageEntry { key: *storage_key, value: *old_storage_value };
                    if plain_storage_cursor
                        .seek_by_key_subkey(*address, *storage_key)?
                        .is_some_and(|s| s.key == *storage_key)
                    {
                        plain_storage_cursor.delete_current()?
                    }

                    if !old_storage_value.is_zero() {
                        plain_storage_cursor.upsert(*address, &storage_entry)?;
                    }
                }
            }
        }

        self.remove_receipts_from(from_transaction_num, block)?;

        Ok(())
    }

    /// Take the last N blocks of state, recreating the [`ExecutionOutcome`].
    ///
    /// The latest state will be unwound and returned back with all the blocks
    ///
    /// 1. Iterate over the [`BlockBodyIndices`][tables::BlockBodyIndices] table to get all the
    ///    transaction ids.
    /// 2. Iterate over the [`StorageChangeSets`][tables::StorageChangeSets] table and the
    ///    [`AccountChangeSets`][tables::AccountChangeSets] tables in reverse order to reconstruct
    ///    the changesets.
    ///    - In order to have both the old and new values in the changesets, we also access the
    ///      plain state tables.
    /// 3. While iterating over the changeset tables, if we encounter a new account or storage slot,
    ///    we:
    ///     1. Take the old value from the changeset
    ///     2. Take the new value from the plain state
    ///     3. Save the old value to the local state
    /// 4. While iterating over the changeset tables, if we encounter an account/storage slot we
    ///    have seen before we:
    ///     1. Take the old value from the changeset
    ///     2. Take the new value from the local state
    ///     3. Set the local state to the value in the changeset
    fn take_state_above(
        &self,
        block: BlockNumber,
        end: BlockNumber,
    ) -> ProviderResult<ExecutionOutcome<Self::Receipt>> {
        let range = block + 1..=end;

        if range.is_empty() {
            return Ok(ExecutionOutcome::default())
        }
        let start_block_number = *range.start();

        // We are not removing block meta as it is used to get block changesets.
        let block_bodies = self.block_body_indices_range(range)?;

        // get transaction receipts
        let from_transaction_num =
            block_bodies.first().expect("already checked if there are blocks").first_tx_num();
        let to_transaction_num =
            block_bodies.last().expect("already checked if there are blocks").last_tx_num();

        let storage_changeset = if let Some(highest_block) = self
            .static_file_provider
            .get_highest_static_file_block(StaticFileSegment::StorageChangeSets) &&
            self.cached_storage_settings().storage_v2
        {
            let changesets = self.storage_changesets_range(block + 1..=highest_block)?;
            let mut changeset_writer =
                self.static_file_provider.latest_writer(StaticFileSegment::StorageChangeSets)?;
            changeset_writer.prune_storage_changesets(block)?;
            changesets
        } else {
            // Revert every changeset above `block`, including any above `end`.
            self.take::<tables::StorageChangeSets>(
                BlockNumberAddress((block + 1, Address::ZERO))..,
            )?
            .into_iter()
            .collect()
        };

        // if there are static files for this segment, prune them.
        let highest_changeset_block = self
            .static_file_provider
            .get_highest_static_file_block(StaticFileSegment::AccountChangeSets);
        let account_changeset = if let Some(highest_block) = highest_changeset_block &&
            self.cached_storage_settings().storage_v2
        {
            // TODO: add a `take` method that removes and returns the items instead of doing this
            let changesets = self.account_changesets_range(block + 1..highest_block + 1)?;
            let mut changeset_writer =
                self.static_file_provider.latest_writer(StaticFileSegment::AccountChangeSets)?;
            changeset_writer.prune_account_changesets(block)?;

            changesets
        } else {
            // Have to remove from static files if they exist, otherwise remove using `take` for the
            // changeset tables
            self.take::<tables::AccountChangeSets>(block + 1..)?
        };

        let (state, reverts) = if self.cached_storage_settings().use_hashed_state() {
            let mut hashed_accounts_cursor = self.tx.cursor_write::<tables::HashedAccounts>()?;
            let mut hashed_storage_cursor = self.tx.cursor_dup_write::<tables::HashedStorages>()?;

            let (state, reverts) = self.populate_bundle_state_hashed(
                account_changeset,
                storage_changeset,
                &mut hashed_accounts_cursor,
                &mut hashed_storage_cursor,
            )?;

            for (address, (old_account, new_account, storage)) in &state {
                if old_account != new_account {
                    let hashed_address = keccak256(address);
                    let existing_entry = hashed_accounts_cursor.seek_exact(hashed_address)?;
                    if let Some(account) = old_account {
                        hashed_accounts_cursor.upsert(hashed_address, account)?;
                    } else if existing_entry.is_some() {
                        hashed_accounts_cursor.delete_current()?;
                    }
                }

                for (storage_key, (old_storage_value, _new_storage_value)) in storage {
                    let hashed_address = keccak256(address);
                    let hashed_storage_key = keccak256(storage_key);
                    let storage_entry =
                        StorageEntry { key: hashed_storage_key, value: *old_storage_value };
                    if hashed_storage_cursor
                        .seek_by_key_subkey(hashed_address, hashed_storage_key)?
                        .is_some_and(|s| s.key == hashed_storage_key)
                    {
                        hashed_storage_cursor.delete_current()?
                    }

                    if !old_storage_value.is_zero() {
                        hashed_storage_cursor.upsert(hashed_address, &storage_entry)?;
                    }
                }
            }

            (state, reverts)
        } else {
            // This is not working for blocks that are not at tip. as plain state is not the last
            // state of end range. We should rename the functions or add support to access
            // History state. Accessing history state can be tricky but we are not gaining
            // anything.
            let mut plain_accounts_cursor = self.tx.cursor_write::<tables::PlainAccountState>()?;
            let mut plain_storage_cursor =
                self.tx.cursor_dup_write::<tables::PlainStorageState>()?;

            let (state, reverts) = self.populate_bundle_state_plain(
                account_changeset,
                storage_changeset,
                &mut plain_accounts_cursor,
                &mut plain_storage_cursor,
            )?;

            for (address, (old_account, new_account, storage)) in &state {
                if old_account != new_account {
                    let existing_entry = plain_accounts_cursor.seek_exact(*address)?;
                    if let Some(account) = old_account {
                        plain_accounts_cursor.upsert(*address, account)?;
                    } else if existing_entry.is_some() {
                        plain_accounts_cursor.delete_current()?;
                    }
                }

                for (storage_key, (old_storage_value, _new_storage_value)) in storage {
                    let storage_entry =
                        StorageEntry { key: *storage_key, value: *old_storage_value };
                    if plain_storage_cursor
                        .seek_by_key_subkey(*address, *storage_key)?
                        .is_some_and(|s| s.key == *storage_key)
                    {
                        plain_storage_cursor.delete_current()?
                    }

                    if !old_storage_value.is_zero() {
                        plain_storage_cursor.upsert(*address, &storage_entry)?;
                    }
                }
            }

            (state, reverts)
        };

        // Collect receipts into tuples (tx_num, receipt) to correctly handle pruned receipts
        let mut receipts_iter = self
            .static_file_provider
            .get_range_with_static_file_or_database(
                StaticFileSegment::Receipts,
                from_transaction_num..to_transaction_num + 1,
                |static_file, range, _| {
                    static_file
                        .receipts_by_tx_range(range.clone())
                        .map(|r| range.into_iter().zip(r).collect())
                },
                |range, _| {
                    self.tx
                        .cursor_read::<tables::Receipts<Self::Receipt>>()?
                        .walk_range(range)?
                        .map(|r| r.map_err(Into::into))
                        .collect()
                },
                |_| true,
            )?
            .into_iter()
            .peekable();

        let mut receipts = Vec::with_capacity(block_bodies.len());
        // loop break if we are at the end of the blocks.
        for block_body in block_bodies {
            let mut block_receipts = Vec::with_capacity(block_body.tx_count as usize);
            for num in block_body.tx_num_range() {
                if receipts_iter.peek().is_some_and(|(n, _)| *n == num) {
                    block_receipts.push(receipts_iter.next().unwrap().1);
                }
            }
            receipts.push(block_receipts);
        }

        self.remove_receipts_from(from_transaction_num, block)?;

        Ok(ExecutionOutcome::new_init(
            state,
            reverts,
            Vec::new(),
            receipts,
            start_block_number,
            Vec::new(),
        ))
    }
}

impl<TX: DbTxMut + DbTx + 'static, N: NodeTypes> DatabaseProvider<TX, N> {
    fn write_account_trie_updates<A: TrieTableAdapter>(
        tx: &TX,
        trie_updates: &TrieUpdatesSorted,
        num_entries: &mut usize,
    ) -> ProviderResult<()>
    where
        TX: DbTxMut,
    {
        let mut account_trie_cursor = tx.cursor_write::<A::AccountTrieTable>()?;
        // Process sorted account nodes
        for (key, updated_node) in trie_updates.account_nodes_ref() {
            let nibbles = A::AccountKey::from(*key);
            match updated_node {
                Some(node) => {
                    if !key.is_empty() {
                        *num_entries += 1;
                        account_trie_cursor.upsert(nibbles, node)?;
                    }
                }
                None => {
                    *num_entries += 1;
                    if account_trie_cursor.seek_exact(nibbles)?.is_some() {
                        account_trie_cursor.delete_current()?;
                    }
                }
            }
        }
        Ok(())
    }

    fn write_storage_tries<A: TrieTableAdapter>(
        tx: &TX,
        storage_tries: Vec<(&B256, &StorageTrieUpdatesSorted)>,
        num_entries: &mut usize,
    ) -> ProviderResult<()>
    where
        TX: DbTxMut,
    {
        let mut cursor = tx.cursor_dup_write::<A::StorageTrieTable>()?;
        for (hashed_address, storage_trie_updates) in storage_tries {
            let mut db_storage_trie_cursor: DatabaseStorageTrieCursor<_, A> =
                DatabaseStorageTrieCursor::new(cursor, *hashed_address);
            *num_entries +=
                db_storage_trie_cursor.write_storage_trie_updates_sorted(storage_trie_updates)?;
            cursor = db_storage_trie_cursor.cursor;
        }
        Ok(())
    }
}

impl<TX: DbTxMut + DbTx + 'static, N: NodeTypes> TrieWriter for DatabaseProvider<TX, N> {
    /// Writes trie updates to the database with already sorted updates.
    ///
    /// Returns the number of entries modified.
    #[instrument(level = "debug", target = "providers::db", skip_all)]
    fn write_trie_updates_sorted(&self, trie_updates: &TrieUpdatesSorted) -> ProviderResult<usize> {
        if trie_updates.is_empty() {
            return Ok(0)
        }

        // Track the number of inserted entries.
        let mut num_entries = 0;

        reth_trie_db::with_adapter!(self, |A| {
            Self::write_account_trie_updates::<A>(self.tx_ref(), trie_updates, &mut num_entries)?;
        });

        num_entries +=
            self.write_storage_trie_updates_sorted(trie_updates.storage_tries_ref().iter())?;

        Ok(num_entries)
    }
}

impl<TX: DbTxMut + DbTx + 'static, N: NodeTypes> StorageTrieWriter for DatabaseProvider<TX, N> {
    /// Writes storage trie updates from the given storage trie map with already sorted updates.
    ///
    /// Expects the storage trie updates to already be sorted by the hashed address key.
    ///
    /// Returns the number of entries modified.
    fn write_storage_trie_updates_sorted<'a>(
        &self,
        storage_tries: impl Iterator<Item = (&'a B256, &'a StorageTrieUpdatesSorted)>,
    ) -> ProviderResult<usize> {
        let mut num_entries = 0;
        let mut storage_tries = storage_tries.collect::<Vec<_>>();
        storage_tries.sort_unstable_by(|a, b| a.0.cmp(b.0));
        reth_trie_db::with_adapter!(self, |A| {
            Self::write_storage_tries::<A>(self.tx_ref(), storage_tries, &mut num_entries)?;
        });
        Ok(num_entries)
    }
}

impl<TX: DbTxMut + DbTx + 'static, N: NodeTypes> HashingWriter for DatabaseProvider<TX, N> {
    #[allow(clippy::clone_on_copy)]
    fn unwind_account_hashing<'a>(
        &self,
        changesets: impl Iterator<Item = &'a (BlockNumber, AccountBeforeTx)>,
    ) -> ProviderResult<BTreeMap<B256, Option<Account>>> {
        // Aggregate all block changesets and make a list of accounts that have been changed.
        // Note that collecting and then reversing the order is necessary to ensure that the
        // changes are applied in the correct order.
        let hashed_accounts = changesets
            .into_iter()
            .map(|(_, e)| (keccak256(e.address), e.info.clone()))
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<BTreeMap<_, _>>();

        // Apply values to HashedState, and remove the account if it's None.
        let mut hashed_accounts_cursor = self.tx.cursor_write::<tables::HashedAccounts>()?;
        for (hashed_address, account) in &hashed_accounts {
            if let Some(account) = account {
                hashed_accounts_cursor.upsert(*hashed_address, account)?;
            } else if hashed_accounts_cursor.seek_exact(*hashed_address)?.is_some() {
                hashed_accounts_cursor.delete_current()?;
            }
        }

        Ok(hashed_accounts)
    }

    fn unwind_account_hashing_range(
        &self,
        range: impl RangeBounds<BlockNumber>,
    ) -> ProviderResult<BTreeMap<B256, Option<Account>>> {
        let changesets = self.account_changesets_range(range)?;
        self.unwind_account_hashing(changesets.iter())
    }

    fn insert_account_for_hashing(
        &self,
        changesets: impl IntoIterator<Item = (Address, Option<Account>)>,
    ) -> ProviderResult<BTreeMap<B256, Option<Account>>> {
        let mut hashed_accounts_cursor = self.tx.cursor_write::<tables::HashedAccounts>()?;
        let hashed_accounts =
            changesets.into_iter().map(|(ad, ac)| (keccak256(ad), ac)).collect::<BTreeMap<_, _>>();
        for (hashed_address, account) in &hashed_accounts {
            if let Some(account) = account {
                hashed_accounts_cursor.upsert(*hashed_address, account)?;
            } else if hashed_accounts_cursor.seek_exact(*hashed_address)?.is_some() {
                hashed_accounts_cursor.delete_current()?;
            }
        }
        Ok(hashed_accounts)
    }

    fn unwind_storage_hashing(
        &self,
        changesets: impl Iterator<Item = (BlockNumberAddress, StorageEntry)>,
    ) -> ProviderResult<B256Map<BTreeSet<B256>>> {
        // Aggregate all block changesets and make list of accounts that have been changed.
        let mut hashed_storages = changesets
            .into_iter()
            .map(|(BlockNumberAddress((_, address)), storage_entry)| {
                let hashed_key = keccak256(storage_entry.key);
                (keccak256(address), hashed_key, storage_entry.value)
            })
            .collect::<Vec<_>>();
        hashed_storages.sort_by_key(|(ha, hk, _)| (*ha, *hk));

        // Apply values to HashedState, and remove the account if it's None.
        let mut hashed_storage_keys: B256Map<BTreeSet<B256>> =
            B256Map::with_capacity_and_hasher(hashed_storages.len(), Default::default());
        let mut hashed_storage = self.tx.cursor_dup_write::<tables::HashedStorages>()?;
        for (hashed_address, key, value) in hashed_storages.into_iter().rev() {
            hashed_storage_keys.entry(hashed_address).or_default().insert(key);

            if hashed_storage
                .seek_by_key_subkey(hashed_address, key)?
                .is_some_and(|entry| entry.key == key)
            {
                hashed_storage.delete_current()?;
            }

            if !value.is_zero() {
                hashed_storage.upsert(hashed_address, &StorageEntry { key, value })?;
            }
        }
        Ok(hashed_storage_keys)
    }

    fn unwind_storage_hashing_range(
        &self,
        range: impl RangeBounds<BlockNumber>,
    ) -> ProviderResult<B256Map<BTreeSet<B256>>> {
        let changesets = self.storage_changesets_range(range)?;
        self.unwind_storage_hashing(changesets.into_iter())
    }

    fn insert_storage_for_hashing(
        &self,
        storages: impl IntoIterator<Item = (Address, impl IntoIterator<Item = StorageEntry>)>,
    ) -> ProviderResult<B256Map<BTreeSet<B256>>> {
        // hash values
        let hashed_storages =
            storages.into_iter().fold(BTreeMap::new(), |mut map, (address, storage)| {
                let storage = storage.into_iter().fold(BTreeMap::new(), |mut map, entry| {
                    map.insert(keccak256(entry.key), entry.value);
                    map
                });
                map.insert(keccak256(address), storage);
                map
            });

        let hashed_storage_keys = hashed_storages
            .iter()
            .map(|(hashed_address, entries)| (*hashed_address, entries.keys().copied().collect()))
            .collect();

        let mut hashed_storage_cursor = self.tx.cursor_dup_write::<tables::HashedStorages>()?;
        // Hash the address and key and apply them to HashedStorage (if Storage is None
        // just remove it);
        hashed_storages.into_iter().try_for_each(|(hashed_address, storage)| {
            storage.into_iter().try_for_each(|(key, value)| -> ProviderResult<()> {
                if hashed_storage_cursor
                    .seek_by_key_subkey(hashed_address, key)?
                    .is_some_and(|entry| entry.key == key)
                {
                    hashed_storage_cursor.delete_current()?;
                }

                if !value.is_zero() {
                    hashed_storage_cursor.upsert(hashed_address, &StorageEntry { key, value })?;
                }
                Ok(())
            })
        })?;

        Ok(hashed_storage_keys)
    }
}
