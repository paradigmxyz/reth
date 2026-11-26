//! Account and storage reads, changesets, and history indexes.

use super::DatabaseProvider;
use crate::{
    changeset_walker::changeset_block_range,
    providers::rocksdb::{OwnedRocksReadSnapshot, RocksReadSnapshot},
    AccountExtReader, AccountReader, BlockNumReader, ChangeSetReader, EitherReader, HistoryWriter,
    ProviderError, StorageReader,
};
use alloy_primitives::{keccak256, Address, BlockNumber, B256};
use itertools::Itertools;
use reth_db_api::{
    cursor::{DbCursorRO, DbCursorRW, DbDupCursorRO},
    models::{
        sharded_key, storage_sharded_key::StorageShardedKey, AccountBeforeTx, BlockNumberAddress,
        BlockNumberAddressRange, ShardedKey,
    },
    table::Table,
    tables,
    transaction::{DbTx, DbTxMut},
    BlockNumberList,
};
use reth_node_types::NodeTypes;
use reth_primitives_traits::{Account, StorageEntry};
use reth_static_file_types::StaticFileSegment;
use reth_storage_api::{HistoryInfo, HistoryReader, StorageChangeSetReader, StorageSettingsCache};
use reth_storage_errors::provider::ProviderResult;
use std::{
    collections::{BTreeMap, BTreeSet},
    ops::{RangeBounds, RangeInclusive},
};
use tracing::instrument;

impl<TX: DbTx, N: NodeTypes> AccountReader for DatabaseProvider<TX, N> {
    fn basic_account(&self, address: &Address) -> ProviderResult<Option<Account>> {
        if self.cached_storage_settings().use_hashed_state() {
            let hashed_address = keccak256(address);
            Ok(self.tx.get_by_encoded_key::<tables::HashedAccounts>(&hashed_address)?)
        } else {
            Ok(self.tx.get_by_encoded_key::<tables::PlainAccountState>(address)?)
        }
    }
}

impl<TX: DbTx + 'static, N: NodeTypes> AccountExtReader for DatabaseProvider<TX, N> {
    fn changed_accounts_with_range(
        &self,
        range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<BTreeSet<Address>> {
        let mut reader = EitherReader::new_account_changesets(self)?;

        reader.changed_accounts_with_range(range)
    }

    fn basic_accounts(
        &self,
        iter: impl IntoIterator<Item = Address>,
    ) -> ProviderResult<Vec<(Address, Option<Account>)>> {
        if self.cached_storage_settings().use_hashed_state() {
            let mut hashed_accounts = self.tx.cursor_read::<tables::HashedAccounts>()?;
            Ok(iter
                .into_iter()
                .map(|address| {
                    let hashed_address = keccak256(address);
                    hashed_accounts.seek_exact(hashed_address).map(|a| (address, a.map(|(_, v)| v)))
                })
                .collect::<Result<Vec<_>, _>>()?)
        } else {
            let mut plain_accounts = self.tx.cursor_read::<tables::PlainAccountState>()?;
            Ok(iter
                .into_iter()
                .map(|address| {
                    plain_accounts.seek_exact(address).map(|a| (address, a.map(|(_, v)| v)))
                })
                .collect::<Result<Vec<_>, _>>()?)
        }
    }

    fn changed_accounts_and_blocks_with_range(
        &self,
        range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<BTreeMap<Address, Vec<u64>>> {
        let highest_static_block = self
            .static_file_provider
            .get_highest_static_file_block(StaticFileSegment::AccountChangeSets);

        if let Some(highest) = highest_static_block &&
            self.cached_storage_settings().storage_v2
        {
            let start = *range.start();
            let static_end = (*range.end()).min(highest);

            let mut changed_accounts_and_blocks: BTreeMap<_, Vec<u64>> = BTreeMap::default();
            if start <= static_end {
                for block in start..=static_end {
                    let block_changesets = self.account_block_changeset(block)?;
                    for changeset in block_changesets {
                        changed_accounts_and_blocks
                            .entry(changeset.address)
                            .or_default()
                            .push(block);
                    }
                }
            }

            Ok(changed_accounts_and_blocks)
        } else {
            let mut changeset_cursor = self.tx.cursor_read::<tables::AccountChangeSets>()?;

            let account_transitions = changeset_cursor.walk_range(range)?.try_fold(
                BTreeMap::new(),
                |mut accounts: BTreeMap<Address, Vec<u64>>, entry| -> ProviderResult<_> {
                    let (index, account) = entry?;
                    accounts.entry(account.address).or_default().push(index);
                    Ok(accounts)
                },
            )?;

            Ok(account_transitions)
        }
    }
}

impl<TX: DbTx, N: NodeTypes> StorageChangeSetReader for DatabaseProvider<TX, N> {
    fn storage_changeset(
        &self,
        block_number: BlockNumber,
    ) -> ProviderResult<Vec<(BlockNumberAddress, StorageEntry)>> {
        if self.cached_storage_settings().storage_v2 {
            self.static_file_provider.storage_changeset(block_number)
        } else {
            let range = block_number..=block_number;
            let storage_range = BlockNumberAddress::range(range);
            self.tx
                .cursor_dup_read::<tables::StorageChangeSets>()?
                .walk_range(storage_range)?
                .map(|r| {
                    let (bna, entry) = r?;
                    Ok((bna, entry))
                })
                .collect()
        }
    }

    fn get_storage_before_block(
        &self,
        block_number: BlockNumber,
        address: Address,
        storage_key: B256,
    ) -> ProviderResult<Option<StorageEntry>> {
        if self.cached_storage_settings().storage_v2 {
            self.static_file_provider.get_storage_before_block(block_number, address, storage_key)
        } else {
            Ok(self
                .tx
                .cursor_dup_read::<tables::StorageChangeSets>()?
                .seek_by_key_subkey(BlockNumberAddress((block_number, address)), storage_key)?
                .filter(|entry| entry.key == storage_key))
        }
    }

    fn storage_changesets_range(
        &self,
        range: impl RangeBounds<BlockNumber>,
    ) -> ProviderResult<Vec<(BlockNumberAddress, StorageEntry)>> {
        let mut changesets = Vec::new();
        self.for_each_storage_changeset(range, |block_address, entry| {
            changesets.push((block_address, entry))
        })?;
        Ok(changesets)
    }

    fn for_each_storage_changeset(
        &self,
        range: impl RangeBounds<BlockNumber>,
        mut f: impl FnMut(BlockNumberAddress, StorageEntry),
    ) -> ProviderResult<()> {
        if self.cached_storage_settings().storage_v2 {
            self.static_file_provider.for_each_storage_changeset(range, f)
        } else {
            let mut cursor = self.tx.cursor_dup_read::<tables::StorageChangeSets>()?;
            for entry in cursor.walk_range(BlockNumberAddressRange::from(range))? {
                let (block_address, storage_entry) = entry?;
                f(block_address, storage_entry);
            }
            Ok(())
        }
    }

    fn lowest_storage_changeset_block(&self) -> ProviderResult<Option<BlockNumber>> {
        // Only storage v2 keeps changesets in static files.
        if !self.cached_storage_settings().storage_v2 {
            return Ok(None)
        }
        Ok(self
            .static_file_provider
            .get_lowest_range_start(StaticFileSegment::StorageChangeSets)
            .filter(|&block| block > 0))
    }
}

impl<TX: DbTx, N: NodeTypes> ChangeSetReader for DatabaseProvider<TX, N> {
    fn account_block_changeset(
        &self,
        block_number: BlockNumber,
    ) -> ProviderResult<Vec<AccountBeforeTx>> {
        if self.cached_storage_settings().storage_v2 {
            let static_changesets =
                self.static_file_provider.account_block_changeset(block_number)?;
            Ok(static_changesets)
        } else {
            let range = block_number..=block_number;
            self.tx
                .cursor_read::<tables::AccountChangeSets>()?
                .walk_range(range)?
                .map(|result| -> ProviderResult<_> {
                    let (_, account_before) = result?;
                    Ok(account_before)
                })
                .collect()
        }
    }

    fn get_account_before_block(
        &self,
        block_number: BlockNumber,
        address: Address,
    ) -> ProviderResult<Option<AccountBeforeTx>> {
        if self.cached_storage_settings().storage_v2 {
            Ok(self.static_file_provider.get_account_before_block(block_number, address)?)
        } else {
            self.tx
                .cursor_dup_read::<tables::AccountChangeSets>()?
                .seek_by_key_subkey(block_number, address)?
                .filter(|acc| acc.address == address)
                .map(Ok)
                .transpose()
        }
    }

    fn account_changesets_range(
        &self,
        range: impl core::ops::RangeBounds<BlockNumber>,
    ) -> ProviderResult<Vec<(BlockNumber, AccountBeforeTx)>> {
        let mut changesets = Vec::new();
        self.for_each_account_changeset(range, |block_number, changeset| {
            changesets.push((block_number, changeset))
        })?;
        Ok(changesets)
    }

    fn for_each_account_changeset(
        &self,
        range: impl RangeBounds<BlockNumber>,
        mut f: impl FnMut(BlockNumber, AccountBeforeTx),
    ) -> ProviderResult<()> {
        if self.cached_storage_settings().storage_v2 {
            self.static_file_provider.for_each_account_changeset(range, f)
        } else {
            let range = changeset_block_range(range, BlockNumber::MAX);
            if range.is_empty() {
                return Ok(())
            }
            let mut cursor = self.tx.cursor_read::<tables::AccountChangeSets>()?;
            for entry in cursor.walk_range(range)? {
                let (block_number, changeset) = entry?;
                f(block_number, changeset);
            }
            Ok(())
        }
    }

    fn lowest_account_changeset_block(&self) -> ProviderResult<Option<BlockNumber>> {
        // Only storage v2 keeps changesets in static files.
        if !self.cached_storage_settings().storage_v2 {
            return Ok(None)
        }
        Ok(self
            .static_file_provider
            .get_lowest_range_start(StaticFileSegment::AccountChangeSets)
            .filter(|&block| block > 0))
    }
}

impl<TX: DbTx + 'static, N: NodeTypes> DatabaseProvider<TX, N> {
    /// Returns the `RocksDB` snapshot used for history lookups, creating it on first use.
    ///
    /// A reading provider serves a single request, so one snapshot is created for all of its
    /// history lookups instead of one per lookup. The snapshot also caches the raw iterator of
    /// each history column family, which is what makes repeated lookups cheap. The storage
    /// settings are read once here as well, so a lookup does not take the settings lock.
    ///
    /// The snapshot and its iterators pin the `RocksDB` snapshot and the SST files and memtables
    /// they were created on until the provider is dropped, the same way the provider's MDBX read
    /// transaction pins its pages. Provider lifetimes are request or job scoped, and a provider
    /// whose MDBX transaction hits the read-transaction timeout errors on every later read and is
    /// dropped by its caller, so that timeout bounds the `RocksDB` retention as well.
    fn history_rocksdb_snapshot(&self) -> Option<&RocksReadSnapshot<'_>> {
        self.rocksdb_history_snapshot
            .get_or_init(|| {
                self.cached_storage_settings()
                    .storage_v2
                    .then(|| self.rocksdb_provider.owned_snapshot())
            })
            .as_ref()
            .map(OwnedRocksReadSnapshot::as_snapshot)
    }
}

impl<TX: DbTx + 'static, N: NodeTypes> HistoryReader for DatabaseProvider<TX, N> {
    fn account_history_info(
        &self,
        address: Address,
        block_number: BlockNumber,
        lowest_available_block_number: Option<BlockNumber>,
    ) -> ProviderResult<HistoryInfo> {
        let visible_tip = self.best_block_number()?;
        let mut reader = EitherReader::new_accounts_history(self, self.history_rocksdb_snapshot())?;
        reader
            .account_history_info(address, block_number, lowest_available_block_number, visible_tip)
            .map(Into::into)
    }

    fn storage_history_info(
        &self,
        address: Address,
        storage_key: B256,
        block_number: BlockNumber,
        lowest_available_block_number: Option<BlockNumber>,
    ) -> ProviderResult<HistoryInfo> {
        let visible_tip = self.best_block_number()?;
        let mut reader = EitherReader::new_storages_history(self, self.history_rocksdb_snapshot())?;
        reader
            .storage_history_info(
                address,
                storage_key,
                block_number,
                lowest_available_block_number,
                visible_tip,
            )
            .map(Into::into)
    }
}

impl<TX: DbTx + 'static, N: NodeTypes> StorageReader for DatabaseProvider<TX, N> {
    fn plain_state_storages(
        &self,
        addresses_with_keys: impl IntoIterator<Item = (Address, impl IntoIterator<Item = B256>)>,
    ) -> ProviderResult<Vec<(Address, Vec<StorageEntry>)>> {
        if self.cached_storage_settings().use_hashed_state() {
            let mut hashed_storage = self.tx.cursor_dup_read::<tables::HashedStorages>()?;

            addresses_with_keys
                .into_iter()
                .map(|(address, storage)| {
                    let hashed_address = keccak256(address);
                    storage
                        .into_iter()
                        .map(|key| -> ProviderResult<_> {
                            let hashed_key = keccak256(key);
                            let value = hashed_storage
                                .seek_by_key_subkey(hashed_address, hashed_key)?
                                .filter(|v| v.key == hashed_key)
                                .map(|v| v.value)
                                .unwrap_or_default();
                            Ok(StorageEntry { key, value })
                        })
                        .collect::<ProviderResult<Vec<_>>>()
                        .map(|storage| (address, storage))
                })
                .collect::<ProviderResult<Vec<(_, _)>>>()
        } else {
            let mut plain_storage = self.tx.cursor_dup_read::<tables::PlainStorageState>()?;

            addresses_with_keys
                .into_iter()
                .map(|(address, storage)| {
                    storage
                        .into_iter()
                        .map(|key| -> ProviderResult<_> {
                            Ok(plain_storage
                                .seek_by_key_subkey(address, key)?
                                .filter(|v| v.key == key)
                                .unwrap_or_else(|| StorageEntry { key, value: Default::default() }))
                        })
                        .collect::<ProviderResult<Vec<_>>>()
                        .map(|storage| (address, storage))
                })
                .collect::<ProviderResult<Vec<(_, _)>>>()
        }
    }

    fn changed_storages_with_range(
        &self,
        range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<BTreeMap<Address, BTreeSet<B256>>> {
        if self.cached_storage_settings().storage_v2 {
            self.storage_changesets_range(range)?.into_iter().try_fold(
                BTreeMap::new(),
                |mut accounts: BTreeMap<Address, BTreeSet<B256>>, entry| {
                    let (BlockNumberAddress((_, address)), storage_entry) = entry;
                    accounts.entry(address).or_default().insert(storage_entry.key);
                    Ok(accounts)
                },
            )
        } else {
            self.tx
                .cursor_read::<tables::StorageChangeSets>()?
                .walk_range(BlockNumberAddress::range(range))?
                // fold all storages and save its old state so we can remove it from HashedStorage
                // it is needed as it is dup table.
                .try_fold(
                    BTreeMap::new(),
                    |mut accounts: BTreeMap<Address, BTreeSet<B256>>, entry| {
                        let (BlockNumberAddress((_, address)), storage_entry) = entry?;
                        accounts.entry(address).or_default().insert(storage_entry.key);
                        Ok(accounts)
                    },
                )
        }
    }

    fn changed_storages_and_blocks_with_range(
        &self,
        range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<BTreeMap<(Address, B256), Vec<u64>>> {
        if self.cached_storage_settings().storage_v2 {
            self.storage_changesets_range(range)?.into_iter().try_fold(
                BTreeMap::new(),
                |mut storages: BTreeMap<(Address, B256), Vec<u64>>, (index, storage)| {
                    storages
                        .entry((index.address(), storage.key))
                        .or_default()
                        .push(index.block_number());
                    Ok(storages)
                },
            )
        } else {
            let mut changeset_cursor = self.tx.cursor_read::<tables::StorageChangeSets>()?;

            let storage_changeset_lists =
                changeset_cursor.walk_range(BlockNumberAddress::range(range))?.try_fold(
                    BTreeMap::new(),
                    |mut storages: BTreeMap<(Address, B256), Vec<u64>>,
                     entry|
                     -> ProviderResult<_> {
                        let (index, storage) = entry?;
                        storages
                            .entry((index.address(), storage.key))
                            .or_default()
                            .push(index.block_number());
                        Ok(storages)
                    },
                )?;

            Ok(storage_changeset_lists)
        }
    }
}

impl<TX: DbTxMut + DbTx + 'static, N: NodeTypes> HistoryWriter for DatabaseProvider<TX, N> {
    fn unwind_account_history_indices<'a>(
        &self,
        changesets: impl Iterator<Item = &'a (BlockNumber, AccountBeforeTx)>,
    ) -> ProviderResult<usize> {
        let mut last_indices = changesets
            .into_iter()
            .map(|(index, account)| (account.address, *index))
            .collect::<Vec<_>>();
        last_indices.sort_unstable_by_key(|(a, _)| *a);

        if self.cached_storage_settings().storage_v2 {
            let batch = self.rocksdb_provider.unwind_account_history_indices(&last_indices)?;
            self.pending_rocksdb_batches.lock().push(batch);
        } else {
            // Unwind the account history index in MDBX.
            let mut cursor = self.tx.cursor_write::<tables::AccountsHistory>()?;
            for &(address, rem_index) in &last_indices {
                let partial_shard = unwind_history_shards::<_, tables::AccountsHistory, _>(
                    &mut cursor,
                    ShardedKey::last(address),
                    rem_index,
                    |sharded_key| sharded_key.key == address,
                )?;

                // Check the last returned partial shard.
                // If it's not empty, the shard needs to be reinserted.
                if !partial_shard.is_empty() {
                    cursor.insert(
                        ShardedKey::last(address),
                        &BlockNumberList::new_pre_sorted(partial_shard),
                    )?;
                }
            }
        }

        let changesets = last_indices.len();
        Ok(changesets)
    }

    fn unwind_account_history_indices_range(
        &self,
        range: impl RangeBounds<BlockNumber>,
    ) -> ProviderResult<usize> {
        let changesets = self.account_changesets_range(range)?;
        self.unwind_account_history_indices(changesets.iter())
    }

    fn insert_account_history_index(
        &self,
        account_transitions: impl IntoIterator<Item = (Address, impl IntoIterator<Item = u64>)>,
    ) -> ProviderResult<()> {
        self.append_history_index::<_, tables::AccountsHistory>(
            account_transitions,
            ShardedKey::new,
        )
    }

    fn unwind_storage_history_indices(
        &self,
        changesets: impl Iterator<Item = (BlockNumberAddress, StorageEntry)>,
    ) -> ProviderResult<usize> {
        let mut storage_changesets = changesets
            .into_iter()
            .map(|(BlockNumberAddress((bn, address)), storage)| (address, storage.key, bn))
            .collect::<Vec<_>>();
        storage_changesets.sort_unstable_by_key(|(address, key, _)| (*address, *key));

        if self.cached_storage_settings().storage_v2 {
            let batch =
                self.rocksdb_provider.unwind_storage_history_indices(&storage_changesets)?;
            self.pending_rocksdb_batches.lock().push(batch);
        } else {
            // Unwind the storage history index in MDBX.
            let mut cursor = self.tx.cursor_write::<tables::StoragesHistory>()?;
            for &(address, storage_key, rem_index) in &storage_changesets {
                let partial_shard = unwind_history_shards::<_, tables::StoragesHistory, _>(
                    &mut cursor,
                    StorageShardedKey::last(address, storage_key),
                    rem_index,
                    |storage_sharded_key| {
                        storage_sharded_key.address == address &&
                            storage_sharded_key.sharded_key.key == storage_key
                    },
                )?;

                // Check the last returned partial shard.
                // If it's not empty, the shard needs to be reinserted.
                if !partial_shard.is_empty() {
                    cursor.insert(
                        StorageShardedKey::last(address, storage_key),
                        &BlockNumberList::new_pre_sorted(partial_shard),
                    )?;
                }
            }
        }

        let changesets = storage_changesets.len();
        Ok(changesets)
    }

    fn unwind_storage_history_indices_range(
        &self,
        range: impl RangeBounds<BlockNumber>,
    ) -> ProviderResult<usize> {
        let changesets = self.storage_changesets_range(range)?;
        self.unwind_storage_history_indices(changesets.into_iter())
    }

    fn insert_storage_history_index(
        &self,
        storage_transitions: impl IntoIterator<Item = ((Address, B256), impl IntoIterator<Item = u64>)>,
    ) -> ProviderResult<()> {
        self.append_history_index::<_, tables::StoragesHistory>(
            storage_transitions,
            |(address, storage_key), highest_block_number| {
                StorageShardedKey::new(address, storage_key, highest_block_number)
            },
        )
    }

    #[instrument(level = "debug", target = "providers::db", skip_all)]
    fn update_history_indices(&self, range: RangeInclusive<BlockNumber>) -> ProviderResult<()> {
        let storage_settings = self.cached_storage_settings();
        if !storage_settings.storage_v2 {
            let indices = self.changed_accounts_and_blocks_with_range(range.clone())?;
            self.insert_account_history_index(indices)?;
        }

        if !storage_settings.storage_v2 {
            let indices = self.changed_storages_and_blocks_with_range(range)?;
            self.insert_storage_history_index(indices)?;
        }

        Ok(())
    }
}

impl<TX: DbTxMut + DbTx + 'static, N: NodeTypes> DatabaseProvider<TX, N> {
    /// Insert history index to the database.
    ///
    /// For each updated partial key, this function retrieves the last shard from the database
    /// (if any), appends the new indices to it, chunks the resulting list if needed, and upserts
    /// the shards back into the database.
    ///
    /// This function is used by history indexing stages.
    fn append_history_index<P, T>(
        &self,
        index_updates: impl IntoIterator<Item = (P, impl IntoIterator<Item = u64>)>,
        mut sharded_key_factory: impl FnMut(P, BlockNumber) -> T::Key,
    ) -> ProviderResult<()>
    where
        P: Copy,
        T: Table<Value = BlockNumberList>,
    {
        // This function cannot be used with DUPSORT tables because `upsert` on DUPSORT tables
        // will append duplicate entries instead of updating existing ones, causing data corruption.
        assert!(!T::DUPSORT, "append_history_index cannot be used with DUPSORT tables");

        let mut cursor = self.tx.cursor_write::<T>()?;

        for (partial_key, indices) in index_updates {
            let last_key = sharded_key_factory(partial_key, u64::MAX);
            let mut last_shard = cursor
                .seek_exact(last_key.clone())?
                .map(|(_, list)| list)
                .unwrap_or_else(BlockNumberList::empty);

            last_shard.append(indices).map_err(ProviderError::other)?;

            // fast path: all indices fit in one shard
            if last_shard.len() <= sharded_key::NUM_OF_INDICES_IN_SHARD as u64 {
                cursor.upsert(last_key, &last_shard)?;
                continue;
            }

            // slow path: rechunk into multiple shards
            let chunks = last_shard.iter().chunks(sharded_key::NUM_OF_INDICES_IN_SHARD);
            let mut chunks_peekable = chunks.into_iter().peekable();

            while let Some(chunk) = chunks_peekable.next() {
                let shard = BlockNumberList::new_pre_sorted(chunk);
                let highest_block_number = if chunks_peekable.peek().is_some() {
                    shard.iter().next_back().expect("`chunks` does not return empty list")
                } else {
                    // Insert last list with `u64::MAX`.
                    u64::MAX
                };

                cursor.upsert(sharded_key_factory(partial_key, highest_block_number), &shard)?;
            }
        }

        Ok(())
    }
}

/// For a given key, unwind all history shards that contain block numbers at or above the given
/// block number.
///
/// S - Sharded key subtype.
/// T - Table to walk over.
/// C - Cursor implementation.
///
/// This function walks the entries from the given start key and deletes all shards that belong to
/// the key and contain block numbers at or above the given block number. Shards entirely below
/// the block number are preserved.
///
/// The boundary shard (the shard that spans across the block number) is removed from the database.
/// Any indices that are below the block number are filtered out and returned for reinsertion.
/// The boundary shard is returned for reinsertion (if it's not empty).
fn unwind_history_shards<S, T, C>(
    cursor: &mut C,
    start_key: T::Key,
    block_number: BlockNumber,
    mut shard_belongs_to_key: impl FnMut(&T::Key) -> bool,
) -> ProviderResult<Vec<u64>>
where
    T: Table<Value = BlockNumberList>,
    T::Key: AsRef<ShardedKey<S>>,
    C: DbCursorRO<T> + DbCursorRW<T>,
{
    // Start from the given key and iterate through shards
    let mut item = cursor.seek_exact(start_key)?;
    while let Some((sharded_key, list)) = item {
        // If the shard does not belong to the key, break.
        if !shard_belongs_to_key(&sharded_key) {
            break
        }

        // Always delete the current shard from the database first
        // We'll decide later what (if anything) to reinsert
        cursor.delete_current()?;

        // Get the first (lowest) block number in this shard
        // All block numbers in a shard are sorted in ascending order
        let first = list.iter().next().expect("List can't be empty");

        // Case 1: Entire shard is at or above the unwinding point
        // Keep it deleted (don't return anything for reinsertion)
        if first >= block_number {
            item = cursor.prev()?;
            continue
        }
        // Case 2: This is a boundary shard (spans across the unwinding point)
        // The shard contains some blocks below and some at/above the unwinding point
        else if block_number <= sharded_key.as_ref().highest_block_number {
            // Return only the block numbers that are below the unwinding point
            // These will be reinserted to preserve the historical data
            return Ok(list.iter().take_while(|i| *i < block_number).collect::<Vec<_>>())
        }
        // Case 3: Entire shard is below the unwinding point
        // Return all block numbers for reinsertion (preserve entire shard)
        return Ok(list.iter().collect::<Vec<_>>())
    }

    // No shards found or all processed
    Ok(Vec::new())
}
