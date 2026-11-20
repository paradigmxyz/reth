use crate::{
    segments::{
        user::history::{finalize_history_prune, prune_database_history, HistoryPruneResult},
        PruneInput, Segment,
    },
    PrunerError,
};
use alloy_primitives::{map::HashMap, Address, BlockNumber, B256};
use reth_db_api::{
    models::{storage_sharded_key::StorageShardedKey, BlockNumberAddress},
    tables,
    transaction::DbTxMut,
};
use reth_provider::{DBProvider, EitherWriter, RocksDBProviderFactory, StaticFileProviderFactory};
use reth_prune_types::{
    PruneMode, PrunePurpose, PruneSegment, SegmentOutput, SegmentOutputCheckpoint,
};
use reth_static_file_types::StaticFileSegment;
use reth_storage_api::{StorageChangeSetReader, StorageSettingsCache};
use tracing::{instrument, trace};

/// Number of storage history tables to prune in one step.
///
/// Storage History consists of two tables: [`tables::StorageChangeSets`] and
/// [`tables::StoragesHistory`]. We want to prune them to the same block number.
const STORAGE_HISTORY_TABLES_TO_PRUNE: usize = 2;

#[derive(Debug)]
pub struct StorageHistory {
    mode: PruneMode,
}

impl StorageHistory {
    pub const fn new(mode: PruneMode) -> Self {
        Self { mode }
    }
}

impl<Provider> Segment<Provider> for StorageHistory
where
    Provider: DBProvider<Tx: DbTxMut>
        + StaticFileProviderFactory
        + StorageChangeSetReader
        + StorageSettingsCache
        + RocksDBProviderFactory,
{
    fn segment(&self) -> PruneSegment {
        PruneSegment::StorageHistory
    }

    fn mode(&self) -> Option<PruneMode> {
        Some(self.mode)
    }

    fn purpose(&self) -> PrunePurpose {
        PrunePurpose::User
    }

    #[instrument(
        name = "StorageHistory::prune",
        target = "pruner",
        skip(self, provider),
        ret(level = "trace")
    )]
    fn prune(&self, provider: &Provider, input: PruneInput) -> Result<SegmentOutput, PrunerError> {
        if EitherWriter::storage_changesets_destination(provider).is_database() {
            return prune_database_history::<tables::StorageChangeSets, tables::StoragesHistory, _>(
                provider.tx_ref(),
                BlockNumberAddress((input.to_block, Address::repeat_byte(0xff))),
                input,
                |(BlockNumberAddress((block, address)), entry)| {
                    (block, StorageShardedKey::new(address, entry.key, 0))
                },
                |a, b| a.address == b.address && a.sharded_key.key == b.sharded_key.key,
            )
        }

        let range = match input.get_next_block_range() {
            Some(range) => range,
            None => {
                trace!(target: "pruner", "No storage history to prune");
                return Ok(SegmentOutput::done())
            }
        };
        let range_end = *range.end();

        // Check where storage history indices are stored
        if provider.cached_storage_settings().storage_v2 {
            return self.prune_rocksdb(provider, input, range, range_end);
        }

        self.prune_static_files(provider, input, range, range_end)
    }
}

impl StorageHistory {
    /// Prunes storage history when changesets are stored in static files.
    fn prune_static_files<Provider>(
        &self,
        provider: &Provider,
        input: PruneInput,
        range: std::ops::RangeInclusive<BlockNumber>,
        range_end: BlockNumber,
    ) -> Result<SegmentOutput, PrunerError>
    where
        Provider: DBProvider<Tx: DbTxMut> + StaticFileProviderFactory,
    {
        let mut limiter = if let Some(limit) = input.limiter.deleted_entries_limit() {
            input.limiter.set_deleted_entries_limit(limit / STORAGE_HISTORY_TABLES_TO_PRUNE)
        } else {
            input.limiter
        };

        // The limiter may already be exhausted from a previous segment in the same prune run.
        // Early exit avoids unnecessary iteration when no budget remains.
        if limiter.is_limit_reached() {
            return Ok(SegmentOutput::not_done(
                limiter.interrupt_reason(),
                input.previous_checkpoint.map(SegmentOutputCheckpoint::from_prune_checkpoint),
            ))
        }

        // The size of this map is limited by `prune_delete_limit * blocks_since_last_run /
        // STORAGE_HISTORY_TABLES_TO_PRUNE`, and with current defaults it's usually `3500 * 5
        // / 2`, so 8750 entries. Each entry is `160 bit + 256 bit + 64 bit`, so the total
        // size should be up to ~0.5MB + some hashmap overhead. `blocks_since_last_run` is
        // additionally limited by the `max_reorg_depth`, so no OOM is expected here.
        let mut highest_deleted_storages = HashMap::default();
        let mut last_changeset_pruned_block = None;
        let mut pruned_changesets = 0;
        let mut done = true;

        let walker = provider.static_file_provider().walk_storage_changeset_range(range);
        for result in walker {
            let (block_address, entry) = result?;
            let block_number = block_address.block_number();
            let address = block_address.address();
            // The walk itself deletes nothing, so an interrupted block cannot be resumed: giving
            // up the budget inside block N reports checkpoint N-1 and the next run rereads the
            // same entries, forever. Stop on block boundaries only, overshooting the budget by at
            // most the rest of one block.
            if limiter.is_limit_reached() &&
                last_changeset_pruned_block.is_some_and(|last| last != block_number)
            {
                done = false;
                break;
            }
            highest_deleted_storages.insert((address, entry.key), block_number);
            last_changeset_pruned_block = Some(block_number);
            pruned_changesets += 1;
            limiter.increment_deleted_entries_count();
        }

        // Delete static file jars only when fully processed
        if done && let Some(last_block) = last_changeset_pruned_block {
            provider
                .static_file_provider()
                .delete_segment_below_block(StaticFileSegment::StorageChangeSets, last_block + 1)?;
        }
        trace!(target: "pruner", pruned = %pruned_changesets, %done, "Pruned storage history (changesets from static files)");

        let result = HistoryPruneResult {
            highest_deleted: highest_deleted_storages,
            last_pruned_block: last_changeset_pruned_block,
            pruned_count: pruned_changesets,
            done,
        };
        finalize_history_prune::<_, tables::StoragesHistory, (Address, B256), _>(
            provider,
            result,
            range_end,
            &limiter,
            |(address, storage_key), block_number| {
                StorageShardedKey::new(address, storage_key, block_number)
            },
            |a, b| a.address == b.address && a.sharded_key.key == b.sharded_key.key,
        )
        .map_err(Into::into)
    }

    /// Prunes storage history when indices are stored in `RocksDB`.
    ///
    /// Reads storage changesets from static files and prunes the corresponding
    /// `RocksDB` history shards.
    fn prune_rocksdb<Provider>(
        &self,
        provider: &Provider,
        input: PruneInput,
        range: std::ops::RangeInclusive<BlockNumber>,
        range_end: BlockNumber,
    ) -> Result<SegmentOutput, PrunerError>
    where
        Provider: DBProvider + StaticFileProviderFactory + RocksDBProviderFactory,
    {
        let mut limiter = input.limiter;

        if limiter.is_limit_reached() {
            return Ok(SegmentOutput::not_done(
                limiter.interrupt_reason(),
                input.previous_checkpoint.map(SegmentOutputCheckpoint::from_prune_checkpoint),
            ))
        }

        let mut highest_deleted_storages = HashMap::<_, _>::default();
        let mut last_changeset_pruned_block = None;
        let mut changesets_processed = 0usize;
        let mut done = true;

        // Walk storage changesets from static files using a streaming iterator.
        // For each changeset, track the highest block number seen for each (address, storage_key)
        // pair to determine which history shard entries need pruning.
        let walker = provider.static_file_provider().walk_storage_changeset_range(range);
        for result in walker {
            let (block_address, entry) = result?;
            let block_number = block_address.block_number();
            let address = block_address.address();
            // Static file changesets are not deleted here, so an interrupted block cannot be
            // resumed: giving up the budget inside block N reports checkpoint N-1 and the next run
            // rereads the same entries, forever. Stop on block boundaries only, overshooting the
            // budget by at most the rest of one block.
            if limiter.is_limit_reached() &&
                last_changeset_pruned_block.is_some_and(|last| last != block_number)
            {
                done = false;
                break;
            }
            highest_deleted_storages.insert((address, entry.key), block_number);
            last_changeset_pruned_block = Some(block_number);
            changesets_processed += 1;
            limiter.increment_deleted_entries_count();
        }

        trace!(target: "pruner", processed = %changesets_processed, %done, "Scanned storage changesets from static files");

        let last_changeset_pruned_block = last_changeset_pruned_block.unwrap_or(range_end);

        // Prune RocksDB history shards for affected storage slots
        let mut deleted_shards = 0usize;
        let mut updated_shards = 0usize;

        // Sort by (address, storage_key) for better RocksDB cache locality
        let mut sorted_storages: Vec<_> = highest_deleted_storages.into_iter().collect();
        sorted_storages.sort_unstable_by_key(|((addr, key), _)| (*addr, *key));

        provider.with_rocksdb_batch(|mut batch| {
            let targets: Vec<_> = sorted_storages
                .iter()
                .map(|((addr, key), highest)| {
                    ((*addr, *key), (*highest).min(last_changeset_pruned_block))
                })
                .collect();

            let outcomes = batch.prune_storage_history_batch(&targets)?;
            deleted_shards = outcomes.deleted;
            updated_shards = outcomes.updated;

            Ok(((), Some(batch.into_inner())))
        })?;

        trace!(target: "pruner", deleted = deleted_shards, updated = updated_shards, %done, "Pruned storage history (RocksDB indices)");

        // Delete static file jars only when fully processed. During provider.commit(), RocksDB
        // batch is committed before the MDBX checkpoint. If crash occurs after RocksDB commit
        // but before MDBX commit, on restart the pruner checkpoint indicates data needs
        // re-pruning, but the RocksDB shards are already pruned - this is safe because pruning
        // is idempotent (re-pruning already-pruned shards is a no-op).
        if done {
            provider.static_file_provider().delete_segment_below_block(
                StaticFileSegment::StorageChangeSets,
                last_changeset_pruned_block + 1,
            )?;
        }

        let progress = limiter.progress(done);

        Ok(SegmentOutput {
            progress,
            pruned: changesets_processed + deleted_shards + updated_shards,
            checkpoint: Some(SegmentOutputCheckpoint {
                block_number: Some(last_changeset_pruned_block),
                tx_number: None,
            }),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::segments::PruneLimiter;
    use alloy_primitives::{Address, BlockNumber, B256, U256};
    use assert_matches::assert_matches;
    use reth_db_api::{
        models::{
            sharded_key::NUM_OF_INDICES_IN_SHARD, storage_sharded_key::StorageShardedKey,
            StorageSettings,
        },
        tables, BlockNumberList,
    };
    use reth_primitives_traits::{Account, StorageEntry};
    use reth_provider::{
        providers::BlockchainProvider, DBProvider, DatabaseProviderFactory, ProviderError,
        PruneCheckpointReader, RocksDBProviderFactory,
    };
    use reth_prune_types::{
        PruneCheckpoint, PruneInterruptReason, PruneMode, PruneProgress, PruneSegment,
    };
    use reth_stages::test_utils::{StorageKind, TestStageDB};
    use reth_stages_types::{StageCheckpoint, StageId};
    use reth_storage_api::{StageCheckpointWriter, StateProviderFactory, StorageSettingsCache};
    use reth_testing_utils::generators::{
        self, random_block_range, random_changeset_range, random_eoa_accounts, BlockRangeParams,
        ChangeSet,
    };
    use std::{collections::BTreeMap, ops::AddAssign};

    #[test]
    fn prune_legacy() {
        let db = TestStageDB::default();
        let mut rng = generators::rng();

        let blocks = random_block_range(
            &mut rng,
            0..=5000,
            BlockRangeParams { parent: Some(B256::ZERO), tx_count: 0..1, ..Default::default() },
        );
        db.insert_blocks(blocks.iter(), StorageKind::Database(None)).expect("insert blocks");

        let accounts = random_eoa_accounts(&mut rng, 2).into_iter().collect::<BTreeMap<_, _>>();

        let (changesets, _) = random_changeset_range(
            &mut rng,
            blocks.iter(),
            accounts.into_iter().map(|(addr, acc)| (addr, (acc, Vec::new()))),
            1..2,
            1..2,
        );
        db.insert_changesets(changesets.clone(), None).expect("insert changesets");
        db.insert_history(changesets.clone(), None).expect("insert history");

        let storage_occurrences = db.table::<tables::StoragesHistory>().unwrap().into_iter().fold(
            BTreeMap::<_, usize>::new(),
            |mut map, (key, _)| {
                map.entry((key.address, key.sharded_key.key)).or_default().add_assign(1);
                map
            },
        );
        assert!(storage_occurrences.into_iter().any(|(_, occurrences)| occurrences > 1));

        assert_eq!(
            db.table::<tables::StorageChangeSets>().unwrap().len(),
            changesets.iter().flatten().flat_map(|(_, _, entries)| entries).count()
        );

        let original_shards = db.table::<tables::StoragesHistory>().unwrap();

        let test_prune = |to_block: BlockNumber,
                          last_pruned_block_number: BlockNumber,
                          expected_result: (PruneProgress, usize)| {
            let prune_mode = PruneMode::Before(to_block);
            let deleted_entries_limit = 500;
            let limiter = PruneLimiter::default().set_deleted_entries_limit(deleted_entries_limit);
            let input = PruneInput {
                previous_checkpoint: db
                    .factory
                    .provider()
                    .unwrap()
                    .get_prune_checkpoint(PruneSegment::StorageHistory)
                    .unwrap(),
                to_block,
                limiter,
            };
            let segment = StorageHistory::new(prune_mode);

            let provider = db.factory.database_provider_rw().unwrap();
            provider.set_storage_settings_cache(StorageSettings::v1());
            let result = segment.prune(&provider, input).unwrap();

            assert_matches!(
                result,
                SegmentOutput {progress, pruned, checkpoint: Some(_)}
                    if (progress, pruned) == expected_result
            );

            segment
                .save_checkpoint(
                    &provider,
                    result.checkpoint.unwrap().as_prune_checkpoint(prune_mode),
                )
                .unwrap();
            provider.commit().expect("commit");

            assert_eq!(
                db.table::<tables::StorageChangeSets>().unwrap().len(),
                changesets
                    .iter()
                    .skip(last_pruned_block_number as usize + 1)
                    .flatten()
                    .flat_map(|(_, _, entries)| entries)
                    .count()
            );

            let actual_shards = db.table::<tables::StoragesHistory>().unwrap();

            let expected_shards = original_shards
                .iter()
                .filter(|(key, _)| key.sharded_key.highest_block_number > last_pruned_block_number)
                .map(|(key, blocks)| {
                    let new_blocks =
                        blocks.iter().skip_while(|block| *block <= last_pruned_block_number);
                    (key.clone(), BlockNumberList::new_pre_sorted(new_blocks))
                })
                .collect::<Vec<_>>();

            assert_eq!(actual_shards, expected_shards);

            assert_eq!(
                db.factory
                    .provider()
                    .unwrap()
                    .get_prune_checkpoint(PruneSegment::StorageHistory)
                    .unwrap(),
                Some(PruneCheckpoint {
                    block_number: Some(last_pruned_block_number),
                    tx_number: None,
                    prune_mode
                })
            );
        };

        test_prune(
            998,
            499,
            (PruneProgress::HasMoreData(PruneInterruptReason::DeletedEntriesLimitReached), 500),
        );
        test_prune(998, 998, (PruneProgress::Finished, 499));
        test_prune(1200, 1200, (PruneProgress::Finished, 202));
    }

    #[test]
    fn prune_across_mdbx_history_shards() -> Result<(), PrunerError> {
        let address = Address::ZERO;
        let slot = B256::ZERO;
        let next_slot = B256::with_last_byte(1);
        let first_end = NUM_OF_INDICES_IN_SHARD as u64;
        let second_end = 2 * first_end;
        let changesets = (1..=second_end + 3).map(|block| {
            let key = if block == second_end + 3 { next_slot } else { slot };
            storage_changes(&[key])
        });
        // The adjacent slot still has an old index whose changeset was already pruned.
        let db = mdbx_db(std::iter::once(storage_changes(&[next_slot])).chain(changesets), 1);

        let shard = |highest, blocks| {
            (
                StorageShardedKey::new(address, slot, highest),
                BlockNumberList::new_pre_sorted(blocks),
            )
        };
        let neighbor = (
            StorageShardedKey::last(address, next_slot),
            BlockNumberList::new_pre_sorted([0, second_end + 3]),
        );
        for (to_block, mut expected) in [
            (
                first_end - 1,
                vec![
                    shard(first_end, first_end..=first_end),
                    shard(second_end, first_end + 1..=second_end),
                    shard(u64::MAX, second_end + 1..=second_end + 2),
                ],
            ),
            (second_end + 1, vec![shard(u64::MAX, second_end + 2..=second_end + 2)]),
            (second_end + 2, vec![]),
        ] {
            let result = prune_mdbx_batch(&db, to_block, None);
            assert_eq!(result.progress, PruneProgress::Finished);
            assert_eq!(result.checkpoint.unwrap().block_number, Some(to_block));
            expected.push(neighbor.clone());
            assert_eq!(db.table::<tables::StoragesHistory>()?, expected);
        }
        Ok(())
    }

    /// An interrupted block is unavailable to readers and its remaining changesets are pruned on
    /// the next run.
    #[test]
    #[allow(clippy::clone_on_copy)]
    fn prune_partial_progress_mid_block() {
        let db = TestStageDB::default();
        let mut rng = generators::rng();

        // Create blocks 0..=10
        let blocks = random_block_range(
            &mut rng,
            0..=10,
            BlockRangeParams { parent: Some(B256::ZERO), tx_count: 0..1, ..Default::default() },
        );
        db.insert_blocks(blocks.iter(), StorageKind::Database(None)).expect("insert blocks");

        // Create specific changesets where block 5 has 4 storage changes
        let addr1 = Address::with_last_byte(1);
        let addr2 = Address::with_last_byte(2);

        let account = Account { nonce: 1, balance: U256::from(100), ..Default::default() };

        // Create storage entries
        let storage_entry = |key: u8| reth_primitives_traits::StorageEntry {
            key: B256::with_last_byte(key),
            value: U256::from(100),
        };

        // Build changesets: blocks 0-4 have 1 storage change each, block 5 has 4 changes, block 6
        // has 1. Entries within each account must be sorted by key.
        let changesets: Vec<ChangeSet> = vec![
            vec![(addr1, account.clone(), vec![storage_entry(1)])], // block 0
            vec![(addr1, account.clone(), vec![storage_entry(1)])], // block 1
            vec![(addr1, account.clone(), vec![storage_entry(1)])], // block 2
            vec![(addr1, account.clone(), vec![storage_entry(1)])], // block 3
            vec![(addr1, account.clone(), vec![storage_entry(1)])], // block 4
            // block 5: 4 different storage changes (2 addresses, each with 2 storage slots)
            // Sorted by address, then by storage key within each address
            vec![
                (addr1, account.clone(), vec![storage_entry(1), storage_entry(2)]),
                (addr2, account.clone(), vec![storage_entry(1), storage_entry(2)]),
            ],
            vec![(addr1, account, vec![storage_entry(3)])], // block 6
        ];

        db.insert_changesets(changesets.clone(), None).expect("insert changesets");
        db.insert_history(changesets.clone(), None).expect("insert history");

        // Total storage changesets
        let total_storage_entries: usize =
            changesets.iter().flat_map(|c| c.iter()).map(|(_, _, entries)| entries.len()).sum();
        assert_eq!(db.table::<tables::StorageChangeSets>().unwrap().len(), total_storage_entries);

        let prune_mode = PruneMode::Before(10);

        // Seven changesets and two empty shards exhaust the budget halfway through block 5.
        let limiter = PruneLimiter::default().set_deleted_entries_limit(9);

        let input = PruneInput { previous_checkpoint: None, to_block: 10, limiter };
        let segment = StorageHistory::new(prune_mode);

        let provider = db.factory.database_provider_rw().unwrap();
        provider.set_storage_settings_cache(StorageSettings::v1());
        let result = segment.prune(&provider, input).unwrap();

        // Should report that there's more data
        assert!(!result.progress.is_finished(), "Expected HasMoreData since we stopped mid-block");

        // Save checkpoint and commit
        segment
            .save_checkpoint(&provider, result.checkpoint.unwrap().as_prune_checkpoint(prune_mode))
            .unwrap();
        provider.commit().expect("commit");

        // Block 5 is already incomplete for readers, even though its remaining rows must be pruned.
        let checkpoint = db
            .factory
            .provider()
            .unwrap()
            .get_prune_checkpoint(PruneSegment::StorageHistory)
            .unwrap()
            .expect("checkpoint should exist");

        assert_eq!(
            checkpoint.block_number,
            Some(5),
            "incomplete block 5 must be unavailable to readers"
        );

        let remaining_changesets = db.table::<tables::StorageChangeSets>().unwrap();
        assert_eq!(
            remaining_changesets,
            [
                ((5, addr2).into(), storage_entry(1)),
                ((5, addr2).into(), storage_entry(2)),
                ((6, addr1).into(), storage_entry(3)),
            ]
        );

        let history = db.table::<tables::StoragesHistory>().unwrap();
        let shard = |address, slot, block| {
            (
                StorageShardedKey::last(address, B256::with_last_byte(slot)),
                BlockNumberList::new_pre_sorted([block]),
            )
        };
        assert_eq!(history, [shard(addr1, 3, 6), shard(addr2, 1, 5), shard(addr2, 2, 5)]);

        // Run prune again to complete - should finish processing block 5 and 6
        let input2 = PruneInput {
            previous_checkpoint: Some(checkpoint),
            to_block: 10,
            limiter: PruneLimiter::default().set_deleted_entries_limit(100), // high limit
        };

        let provider2 = db.factory.database_provider_rw().unwrap();
        provider2.set_storage_settings_cache(StorageSettings::v1());
        let result2 = segment.prune(&provider2, input2).unwrap();

        assert!(result2.progress.is_finished(), "Second run should complete");

        segment
            .save_checkpoint(
                &provider2,
                result2.checkpoint.unwrap().as_prune_checkpoint(prune_mode),
            )
            .unwrap();
        provider2.commit().expect("commit");

        // Verify final checkpoint
        let final_checkpoint = db
            .factory
            .provider()
            .unwrap()
            .get_prune_checkpoint(PruneSegment::StorageHistory)
            .unwrap()
            .expect("checkpoint should exist");

        // Should now be at block 6 (the last block with changesets)
        assert_eq!(final_checkpoint.block_number, Some(6), "Final checkpoint should be at block 6");

        // All changesets should be pruned
        let final_changesets = db.table::<tables::StorageChangeSets>().unwrap();
        assert!(final_changesets.is_empty(), "All changesets up to block 10 should be pruned");
    }

    #[test]
    fn prune_rocksdb() {
        use reth_storage_api::StorageSettings;

        let db = TestStageDB::default();
        let mut rng = generators::rng();

        let blocks = random_block_range(
            &mut rng,
            0..=100,
            BlockRangeParams { parent: Some(B256::ZERO), tx_count: 0..1, ..Default::default() },
        );
        db.insert_blocks(blocks.iter(), StorageKind::Database(None)).expect("insert blocks");

        let accounts = random_eoa_accounts(&mut rng, 2).into_iter().collect::<BTreeMap<_, _>>();

        let (changesets, _) = random_changeset_range(
            &mut rng,
            blocks.iter(),
            accounts.into_iter().map(|(addr, acc)| (addr, (acc, Vec::new()))),
            1..2,
            1..2,
        );

        db.insert_changesets_to_static_files(changesets.clone(), None)
            .expect("insert changesets to static files");

        let mut storage_indices: BTreeMap<(alloy_primitives::Address, B256), Vec<u64>> =
            BTreeMap::new();
        for (block, changeset) in changesets.iter().enumerate() {
            for (address, _, storage_entries) in changeset {
                for entry in storage_entries {
                    storage_indices.entry((*address, entry.key)).or_default().push(block as u64);
                }
            }
        }

        {
            let rocksdb = db.factory.rocksdb_provider();
            let mut batch = rocksdb.batch();
            for ((address, storage_key), block_numbers) in &storage_indices {
                let shard = BlockNumberList::new_pre_sorted(block_numbers.clone());
                batch
                    .put::<tables::StoragesHistory>(
                        StorageShardedKey::last(*address, *storage_key),
                        &shard,
                    )
                    .expect("insert storage history shard");
            }
            batch.commit().expect("commit rocksdb batch");
        }

        {
            let rocksdb = db.factory.rocksdb_provider();
            for (address, storage_key) in storage_indices.keys() {
                let shards = rocksdb.storage_history_shards(*address, *storage_key).unwrap();
                assert!(!shards.is_empty(), "RocksDB should contain storage history before prune");
            }
        }

        let to_block = 50u64;
        let prune_mode = PruneMode::Before(to_block);
        let input =
            PruneInput { previous_checkpoint: None, to_block, limiter: PruneLimiter::default() };
        let segment = StorageHistory::new(prune_mode);

        let provider = db.factory.database_provider_rw().unwrap();
        provider.set_storage_settings_cache(StorageSettings::v2());
        let result = segment.prune(&provider, input).unwrap();
        provider.commit().expect("commit");

        assert_matches!(
            result,
            SegmentOutput { progress: PruneProgress::Finished, checkpoint: Some(_), .. }
        );

        {
            let rocksdb = db.factory.rocksdb_provider();
            for ((address, storage_key), block_numbers) in &storage_indices {
                let shards = rocksdb.storage_history_shards(*address, *storage_key).unwrap();

                let remaining_blocks: Vec<u64> =
                    block_numbers.iter().copied().filter(|&b| b > to_block).collect();

                if remaining_blocks.is_empty() {
                    assert!(
                        shards.is_empty(),
                        "Shard for {:?}/{:?} should be deleted when all blocks pruned",
                        address,
                        storage_key
                    );
                } else {
                    assert!(!shards.is_empty(), "Shard should exist with remaining blocks");
                    let actual_blocks: Vec<u64> =
                        shards.iter().flat_map(|(_, list)| list.iter()).collect();
                    assert_eq!(
                        actual_blocks, remaining_blocks,
                        "RocksDB shard should only contain blocks > {}",
                        to_block
                    );
                }
            }
        }
    }

    /// A block holding at least a whole run's budget of changesets must not stall pruning: the
    /// walk deletes no changesets, so a checkpoint rewound below such a block would make every
    /// later run reread it and never advance.
    #[test]
    #[allow(clippy::clone_on_copy)]
    fn dense_block_advances_rocksdb_checkpoint() {
        use reth_storage_api::StorageSettings;

        let db = TestStageDB::default();
        let mut rng = generators::rng();

        let blocks = random_block_range(
            &mut rng,
            0..=20,
            BlockRangeParams { parent: Some(B256::ZERO), tx_count: 0..1, ..Default::default() },
        );
        db.insert_blocks(blocks.iter(), StorageKind::Database(None)).expect("insert blocks");

        // Two storage changesets per block, so a budget of two makes every block "dense".
        const ENTRIES_PER_BLOCK: usize = 2;
        let (address, account) = random_eoa_accounts(&mut rng, 1).into_iter().next().unwrap();
        let keys = [B256::with_last_byte(1), B256::with_last_byte(2)];
        let changesets = (0..=20)
            .map(|_| {
                vec![(
                    address,
                    account.clone(),
                    keys.iter().map(|key| StorageEntry { key: *key, value: U256::ONE }).collect(),
                )]
            })
            .collect::<Vec<_>>();
        db.insert_changesets_to_static_files(changesets, None)
            .expect("insert changesets to static files");

        {
            let rocksdb = db.factory.rocksdb_provider();
            let mut batch = rocksdb.batch();
            for key in keys {
                batch
                    .put::<tables::StoragesHistory>(
                        StorageShardedKey::last(address, key),
                        &BlockNumberList::new_pre_sorted(0..=20),
                    )
                    .expect("insert storage history shard");
            }
            batch.commit().expect("commit rocksdb batch");
        }

        let to_block = 15u64;
        let prune_mode = PruneMode::Before(to_block);
        let segment = StorageHistory::new(prune_mode);

        // Start from a checkpoint in the middle so a rewind can't be masked by block 0.
        let mut checkpoint = PruneCheckpoint { block_number: Some(4), tx_number: None, prune_mode };

        let run_prune = |checkpoint: PruneCheckpoint, limit: usize| {
            let input = PruneInput {
                previous_checkpoint: Some(checkpoint),
                to_block,
                limiter: PruneLimiter::default().set_deleted_entries_limit(limit),
            };

            let provider = db.factory.database_provider_rw().unwrap();
            provider.set_storage_settings_cache(StorageSettings::v2());
            let result = segment.prune(&provider, input).unwrap();
            segment
                .save_checkpoint(
                    &provider,
                    result.checkpoint.unwrap().as_prune_checkpoint(prune_mode),
                )
                .unwrap();
            provider.commit().expect("commit");

            let checkpoint = db
                .factory
                .provider()
                .unwrap()
                .get_prune_checkpoint(PruneSegment::StorageHistory)
                .unwrap()
                .unwrap();
            (result, checkpoint)
        };

        // The RocksDB path does not halve the limit, so this budget is exactly one dense block.
        for _ in 0..3 {
            let previous = checkpoint.block_number;
            let (result, next) = run_prune(checkpoint, ENTRIES_PER_BLOCK);
            checkpoint = next;

            assert!(
                !result.progress.is_finished(),
                "the range is longer than one run's budget allows"
            );
            assert!(
                checkpoint.block_number > previous,
                "checkpoint must advance past the dense block, got {:?} after {previous:?}",
                checkpoint.block_number
            );
        }
        assert_eq!(checkpoint.block_number, Some(7), "one dense block cleared per run");

        // With enough budget the remainder of the range completes in one run.
        let (result, checkpoint) = run_prune(checkpoint, 1000);
        assert!(result.progress.is_finished());
        assert_eq!(checkpoint.block_number, Some(to_block));
    }

    /// Keeps the full shard history even when older changesets have already been pruned.
    fn mdbx_db(
        history: impl IntoIterator<Item = ChangeSet> + Clone,
        first_changeset: usize,
    ) -> TestStageDB {
        let db = TestStageDB::default();
        db.factory.set_storage_settings_cache(StorageSettings::v1());
        db.insert_changesets(
            history.clone().into_iter().skip(first_changeset),
            Some(first_changeset as u64),
        )
        .unwrap();
        db.insert_history(history, None).unwrap();
        db
    }

    fn storage_changes(slots: &[B256]) -> ChangeSet {
        vec![(
            Address::ZERO,
            Account::default(),
            slots.iter().map(|&slot| StorageEntry::new(slot, U256::ONE)).collect(),
        )]
    }

    fn prune_mdbx_batch(
        db: &TestStageDB,
        to_block: BlockNumber,
        limit: Option<usize>,
    ) -> SegmentOutput {
        let rows = || {
            db.count_entries::<tables::StorageChangeSets>().unwrap() +
                db.count_entries::<tables::StoragesHistory>().unwrap()
        };
        let before = rows();
        let provider = db.factory.database_provider_rw().unwrap();
        let segment = StorageHistory::new(PruneMode::Full);
        let limiter = limit.map_or_else(PruneLimiter::default, |limit| {
            PruneLimiter::default().set_deleted_entries_limit(limit)
        });
        let input = PruneInput {
            previous_checkpoint: provider
                .get_prune_checkpoint(PruneSegment::StorageHistory)
                .unwrap(),
            to_block,
            limiter,
        };
        let result = segment.prune(&provider, input.clone()).unwrap();
        if let Some(checkpoint) = result.checkpoint {
            let checkpoint = checkpoint.as_prune_checkpoint(PruneMode::Full);
            segment.save_checkpoint(&provider, checkpoint).unwrap();
            if result.progress.is_finished() && checkpoint.block_number == Some(to_block) {
                let repeat = PruneInput { previous_checkpoint: Some(checkpoint), ..input };
                assert_eq!(segment.prune(&provider, repeat).unwrap(), SegmentOutput::done());
            }
        }
        provider.commit().unwrap();
        assert_eq!(before - rows(), result.pruned);
        assert!(limit.is_none_or(|limit| result.pruned <= limit));
        result
    }

    fn save_mdbx_checkpoint(db: &TestStageDB, block: BlockNumber) {
        db.commit_with_provider(|provider| {
            let checkpoint = PruneCheckpoint { block_number: Some(block), ..Default::default() };
            StorageHistory::new(PruneMode::Full).save_checkpoint(provider, checkpoint)
        })
        .unwrap();
    }

    fn assert_only_storage_change(db: &TestStageDB, slot: B256, block: BlockNumber) {
        assert_eq!(
            db.table::<tables::StorageChangeSets>().unwrap(),
            [((block, Address::ZERO).into(), StorageEntry::new(slot, U256::ONE))]
        );
        assert_eq!(
            db.table::<tables::StoragesHistory>().unwrap(),
            [(
                StorageShardedKey::last(Address::ZERO, slot),
                BlockNumberList::new_pre_sorted([block])
            )]
        );
    }

    #[test]
    fn prune_mdbx_legacy_shards_within_budget() {
        let slot = B256::ZERO;
        let end = 3 * NUM_OF_INDICES_IN_SHARD as u64;
        // Legacy pruning removed the old changesets but left three expired finite shards.
        let history = (1..=end + 2).map(|_| storage_changes(&[slot]));
        let db = mdbx_db(std::iter::once(vec![]).chain(history), end as usize + 1);
        save_mdbx_checkpoint(&db, end);

        for limit in [2, 1] {
            let result = prune_mdbx_batch(&db, end + 1, Some(limit));
            assert!(!result.progress.is_finished());
            assert_eq!(result.checkpoint.unwrap().block_number, Some(end));
        }
        let result = prune_mdbx_batch(&db, end + 1, Some(1));
        assert_eq!(result.progress, PruneProgress::Finished);
        assert_eq!(result.checkpoint.unwrap().block_number, Some(end + 1));
        assert_only_storage_change(&db, slot, end + 2);
    }

    #[test]
    fn prune_mdbx_resumes_at_shard_boundary() {
        let slot = B256::ZERO;
        let neighbor = B256::with_last_byte(1);
        let end = NUM_OF_INDICES_IN_SHARD as u64;
        let slots = [slot, neighbor];
        let changes = (1..=end + 1)
            .map(|block| storage_changes(if block == end { &slots } else { &slots[..1] }));
        let db = mdbx_db(std::iter::once(vec![]).chain(changes), 1);
        assert!(prune_mdbx_batch(&db, end - 1, None).progress.is_finished());

        let interrupted = prune_mdbx_batch(&db, end, Some(2));
        assert!(!interrupted.progress.is_finished());
        assert_eq!(interrupted.checkpoint.unwrap().block_number, Some(end));
        let resumed = prune_mdbx_batch(&db, end, None);
        assert!(resumed.progress.is_finished());
        assert_eq!(resumed.checkpoint.unwrap().block_number, Some(end));
        assert_only_storage_change(&db, slot, end + 1);
    }

    #[test]
    fn prune_mdbx_resumes_within_genesis() {
        let slots = [B256::ZERO, B256::with_last_byte(1), B256::with_last_byte(2)];
        let db = mdbx_db([storage_changes(&slots), storage_changes(&slots[..1])], 0);

        let interrupted = prune_mdbx_batch(&db, 1, Some(2));
        assert!(!interrupted.progress.is_finished());
        assert_eq!(interrupted.checkpoint.unwrap().block_number, None);
        let resumed = prune_mdbx_batch(&db, 1, None);
        assert!(resumed.progress.is_finished());
        assert_eq!(resumed.checkpoint.unwrap().block_number, Some(1));
        assert!(db.table_is_empty::<tables::StorageChangeSets>().unwrap());
        assert!(db.table_is_empty::<tables::StoragesHistory>().unwrap());
    }

    #[test]
    fn prune_mdbx_genesis_below_legacy_checkpoint() -> Result<(), ProviderError> {
        let future_slot = B256::with_last_byte(2);
        let mut changes = vec![vec![]; 7];
        changes[0] = storage_changes(&[B256::ZERO, B256::with_last_byte(1)]);
        changes[6] = storage_changes(&[future_slot]);
        let db = mdbx_db(changes, 0);
        save_mdbx_checkpoint(&db, 5);

        // Two genesis changesets and two shards were stranded below the old checkpoint.
        for _ in 0..4 {
            assert_eq!(prune_mdbx_batch(&db, 5, Some(1)).pruned, 1);
            let checkpoint =
                db.factory.provider()?.get_prune_checkpoint(PruneSegment::StorageHistory)?;
            assert_eq!(checkpoint.unwrap().block_number, Some(5));
        }
        assert_eq!(prune_mdbx_batch(&db, 5, Some(1)), SegmentOutput::done());
        assert_only_storage_change(&db, future_slot, 6);
        Ok(())
    }

    #[test]
    fn prune_mdbx_partial_block_read_boundary() {
        let blocks = random_block_range(
            &mut generators::rng(),
            0..=3,
            BlockRangeParams { parent: Some(B256::ZERO), tx_count: 0..1, ..Default::default() },
        );
        let slot = B256::ZERO;
        let db = mdbx_db([vec![], vec![], storage_changes(&[slot, B256::with_last_byte(1)])], 0);
        db.insert_blocks(blocks.iter(), StorageKind::Static).unwrap();
        db.commit_with_provider(|provider| {
            provider.save_stage_checkpoint(StageId::Finish, StageCheckpoint::new(3))?;
            provider.tx_ref().put::<tables::PlainStorageState>(
                Address::ZERO,
                StorageEntry::new(slot, U256::from(20)),
            )?;
            Ok(())
        })
        .unwrap();
        save_mdbx_checkpoint(&db, 1);
        let chain = BlockchainProvider::new(db.factory.clone()).unwrap();
        let storage_at = |block| chain.history_by_block_number(block)?.storage(Address::ZERO, slot);
        assert_eq!(storage_at(1).unwrap(), Some(U256::ONE));
        assert_eq!(chain.storage_changeset(2).unwrap().len(), 2);

        // The first deletion removes the index; the second removes the changeset.
        for _ in 0..2 {
            let batch = prune_mdbx_batch(&db, 2, Some(1));
            assert!(!batch.progress.is_finished());
            assert_matches!(
                storage_at(1),
                Err(ProviderError::InsufficientChangesets { requested: 1, available })
                    if available == (2..=3)
            );
            assert_matches!(chain.storage_changeset(2), Err(ProviderError::StateAtBlockPruned(2)));
            assert_eq!(storage_at(2).unwrap(), Some(U256::from(20)));
        }
    }
}
