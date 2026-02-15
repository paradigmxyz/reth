use crate::{
    db_ext::DbTxPruneExt,
    segments::{
        user::history::{finalize_history_prune, HistoryPruneResult},
        PruneInput, Segment,
    },
    PrunerError,
};
use alloy_primitives::BlockNumber;
use reth_db_api::{models::ShardedKey, tables, transaction::DbTxMut};
use reth_provider::{
    changeset_walker::StaticFileAccountChangesetWalker, DBProvider, EitherWriter,
    RocksDBProviderFactory, StaticFileProviderFactory,
};
use reth_prune_types::{
    PruneMode, PrunePurpose, PruneSegment, SegmentOutput, SegmentOutputCheckpoint,
};
use reth_static_file_types::StaticFileSegment;
use reth_storage_api::{ChangeSetReader, StorageSettingsCache};
use rustc_hash::FxHashMap;
use tracing::{instrument, trace};

/// Number of account history tables to prune in one step.
///
/// Account History consists of two tables: [`tables::AccountChangeSets`] (either in database or
/// static files) and [`tables::AccountsHistory`]. We want to prune them to the same block number.
const ACCOUNT_HISTORY_TABLES_TO_PRUNE: usize = 2;

#[derive(Debug)]
pub struct AccountHistory {
    mode: PruneMode,
}

impl AccountHistory {
    pub const fn new(mode: PruneMode) -> Self {
        Self { mode }
    }
}

impl<Provider> Segment<Provider> for AccountHistory
where
    Provider: DBProvider<Tx: DbTxMut>
        + StaticFileProviderFactory
        + StorageSettingsCache
        + ChangeSetReader
        + RocksDBProviderFactory,
{
    fn segment(&self) -> PruneSegment {
        PruneSegment::AccountHistory
    }

    fn mode(&self) -> Option<PruneMode> {
        Some(self.mode)
    }

    fn purpose(&self) -> PrunePurpose {
        PrunePurpose::User
    }

    #[instrument(
        name = "AccountHistory::prune",
        target = "pruner",
        skip(self, provider),
        ret(level = "trace")
    )]
    fn prune(&self, provider: &Provider, input: PruneInput) -> Result<SegmentOutput, PrunerError> {
        let range = match input.get_next_block_range() {
            Some(range) => range,
            None => {
                trace!(target: "pruner", "No account history to prune");
                return Ok(SegmentOutput::done())
            }
        };
        let range_end = *range.end();

        // Check where account history indices are stored
        #[cfg(all(unix, feature = "rocksdb"))]
        if provider.cached_storage_settings().storage_v2 {
            return self.prune_rocksdb(provider, input, range, range_end);
        }

        // Check where account changesets are stored (MDBX path)
        if EitherWriter::account_changesets_destination(provider).is_static_file() {
            self.prune_static_files(provider, input, range, range_end)
        } else {
            self.prune_database(provider, input, range, range_end)
        }
    }
}

impl AccountHistory {
    /// Prunes account history when changesets are stored in static files.
    fn prune_static_files<Provider>(
        &self,
        provider: &Provider,
        input: PruneInput,
        range: std::ops::RangeInclusive<BlockNumber>,
        range_end: BlockNumber,
    ) -> Result<SegmentOutput, PrunerError>
    where
        Provider: DBProvider<Tx: DbTxMut> + StaticFileProviderFactory + ChangeSetReader,
    {
        let mut limiter = if let Some(limit) = input.limiter.deleted_entries_limit() {
            input.limiter.set_deleted_entries_limit(limit / ACCOUNT_HISTORY_TABLES_TO_PRUNE)
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

        // Deleted account changeset keys (account addresses) with the highest block number deleted
        // for that key.
        //
        // The size of this map is limited by `prune_delete_limit * blocks_since_last_run /
        // ACCOUNT_HISTORY_TABLES_TO_PRUNE`, and with current default it's usually `3500 * 5
        // / 2`, so 8750 entries. Each entry is `160 bit + 64 bit`, so the total
        // size should be up to ~0.25MB + some hashmap overhead. `blocks_since_last_run` is
        // additionally limited by the `max_reorg_depth`, so no OOM is expected here.
        let mut highest_deleted_accounts = FxHashMap::default();
        let mut last_changeset_pruned_block = None;
        let mut pruned_changesets = 0;
        let mut done = true;

        let walker = StaticFileAccountChangesetWalker::new(provider, range);
        for result in walker {
            if limiter.is_limit_reached() {
                done = false;
                break;
            }
            let (block_number, changeset) = result?;
            highest_deleted_accounts.insert(changeset.address, block_number);
            last_changeset_pruned_block = Some(block_number);
            pruned_changesets += 1;
            limiter.increment_deleted_entries_count();
        }

        // Delete static file jars only when fully processed
        if done && let Some(last_block) = last_changeset_pruned_block {
            provider
                .static_file_provider()
                .delete_segment_below_block(StaticFileSegment::AccountChangeSets, last_block + 1)?;
        }
        trace!(target: "pruner", pruned = %pruned_changesets, %done, "Pruned account history (changesets from static files)");

        let result = HistoryPruneResult {
            highest_deleted: highest_deleted_accounts,
            last_pruned_block: last_changeset_pruned_block,
            pruned_count: pruned_changesets,
            done,
        };
        finalize_history_prune::<_, tables::AccountsHistory, _, _>(
            provider,
            result,
            range_end,
            &limiter,
            ShardedKey::new,
            |a, b| a.key == b.key,
        )
        .map_err(Into::into)
    }

    fn prune_database<Provider>(
        &self,
        provider: &Provider,
        input: PruneInput,
        range: std::ops::RangeInclusive<BlockNumber>,
        range_end: BlockNumber,
    ) -> Result<SegmentOutput, PrunerError>
    where
        Provider: DBProvider<Tx: DbTxMut>,
    {
        let mut limiter = if let Some(limit) = input.limiter.deleted_entries_limit() {
            input.limiter.set_deleted_entries_limit(limit / ACCOUNT_HISTORY_TABLES_TO_PRUNE)
        } else {
            input.limiter
        };

        if limiter.is_limit_reached() {
            return Ok(SegmentOutput::not_done(
                limiter.interrupt_reason(),
                input.previous_checkpoint.map(SegmentOutputCheckpoint::from_prune_checkpoint),
            ))
        }

        // Deleted account changeset keys (account addresses) with the highest block number deleted
        // for that key.
        //
        // The size of this map is limited by `prune_delete_limit * blocks_since_last_run /
        // ACCOUNT_HISTORY_TABLES_TO_PRUNE`, and with the current defaults it's usually `3500 * 5 /
        // 2`, so 8750 entries. Each entry is `160 bit + 64 bit`, so the total size should be up to
        // ~0.25MB + some hashmap overhead. `blocks_since_last_run` is additionally limited by the
        // `max_reorg_depth`, so no OOM is expected here.
        let mut last_changeset_pruned_block = None;
        let mut highest_deleted_accounts = FxHashMap::default();
        let (pruned_changesets, done) =
            provider.tx_ref().prune_table_with_range::<tables::AccountChangeSets>(
                range,
                &mut limiter,
                |_| false,
                |(block_number, account)| {
                    highest_deleted_accounts.insert(account.address, block_number);
                    last_changeset_pruned_block = Some(block_number);
                },
            )?;
        trace!(target: "pruner", pruned = %pruned_changesets, %done, "Pruned account history (changesets from database)");

        let result = HistoryPruneResult {
            highest_deleted: highest_deleted_accounts,
            last_pruned_block: last_changeset_pruned_block,
            pruned_count: pruned_changesets,
            done,
        };
        finalize_history_prune::<_, tables::AccountsHistory, _, _>(
            provider,
            result,
            range_end,
            &limiter,
            ShardedKey::new,
            |a, b| a.key == b.key,
        )
        .map_err(Into::into)
    }

    /// Prunes account history when indices are stored in `RocksDB`.
    ///
    /// Reads account changesets from static files and prunes the corresponding
    /// `RocksDB` history shards.
    #[cfg(all(unix, feature = "rocksdb"))]
    fn prune_rocksdb<Provider>(
        &self,
        provider: &Provider,
        input: PruneInput,
        range: std::ops::RangeInclusive<BlockNumber>,
        range_end: BlockNumber,
    ) -> Result<SegmentOutput, PrunerError>
    where
        Provider: DBProvider + StaticFileProviderFactory + ChangeSetReader + RocksDBProviderFactory,
    {
        // Unlike MDBX path, we don't divide the limit by 2 because RocksDB path only prunes
        // history shards (no separate changeset table to delete from). The changesets are in
        // static files which are deleted separately.
        let mut limiter = input.limiter;

        if limiter.is_limit_reached() {
            return Ok(SegmentOutput::not_done(
                limiter.interrupt_reason(),
                input.previous_checkpoint.map(SegmentOutputCheckpoint::from_prune_checkpoint),
            ))
        }

        let mut highest_deleted_accounts = FxHashMap::default();
        let mut last_changeset_pruned_block = None;
        let mut changesets_processed = 0usize;
        let mut done = true;

        // Walk account changesets from static files using a streaming iterator.
        // For each changeset, track the highest block number seen for each address
        // to determine which history shard entries need pruning.
        let walker = StaticFileAccountChangesetWalker::new(provider, range);
        for result in walker {
            if limiter.is_limit_reached() {
                done = false;
                break;
            }
            let (block_number, changeset) = result?;
            highest_deleted_accounts.insert(changeset.address, block_number);
            last_changeset_pruned_block = Some(block_number);
            changesets_processed += 1;
            limiter.increment_deleted_entries_count();
        }
        trace!(target: "pruner", processed = %changesets_processed, %done, "Scanned account changesets from static files");

        let last_changeset_pruned_block = last_changeset_pruned_block
            .map(|block_number| if done { block_number } else { block_number.saturating_sub(1) })
            .unwrap_or(range_end);

        // Prune RocksDB history shards for affected accounts
        let mut deleted_shards = 0usize;
        let mut updated_shards = 0usize;

        // Sort by address for better RocksDB cache locality
        let mut sorted_accounts: Vec<_> = highest_deleted_accounts.into_iter().collect();
        sorted_accounts.sort_unstable_by_key(|(addr, _)| *addr);

        provider.with_rocksdb_batch(|mut batch| {
            let targets: Vec<_> = sorted_accounts
                .iter()
                .map(|(addr, highest)| (*addr, (*highest).min(last_changeset_pruned_block)))
                .collect();

            let outcomes = batch.prune_account_history_batch(&targets)?;
            deleted_shards = outcomes.deleted;
            updated_shards = outcomes.updated;

            Ok(((), Some(batch.into_inner())))
        })?;
        trace!(target: "pruner", deleted = deleted_shards, updated = updated_shards, %done, "Pruned account history (RocksDB indices)");

        // Delete static file jars only when fully processed. During provider.commit(), RocksDB
        // batch is committed before the MDBX checkpoint. If crash occurs after RocksDB commit
        // but before MDBX commit, on restart the pruner checkpoint indicates data needs
        // re-pruning, but the RocksDB shards are already pruned - this is safe because pruning
        // is idempotent (re-pruning already-pruned shards is a no-op).
        if done {
            provider.static_file_provider().delete_segment_below_block(
                StaticFileSegment::AccountChangeSets,
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
    use super::ACCOUNT_HISTORY_TABLES_TO_PRUNE;
    use crate::segments::{AccountHistory, PruneInput, PruneLimiter, Segment, SegmentOutput};
    use alloy_primitives::{BlockNumber, B256};
    use assert_matches::assert_matches;
    use reth_db_api::{models::StorageSettings, tables, BlockNumberList};
    use reth_provider::{DBProvider, DatabaseProviderFactory, PruneCheckpointReader};
    use reth_prune_types::{
        PruneCheckpoint, PruneInterruptReason, PruneMode, PruneProgress, PruneSegment,
    };
    use reth_stages::test_utils::{StorageKind, TestStageDB};
    use reth_storage_api::StorageSettingsCache;
    use reth_testing_utils::generators::{
        self, random_block_range, random_changeset_range, random_eoa_accounts, BlockRangeParams,
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
            0..0,
            0..0,
        );
        db.insert_changesets(changesets.clone(), None).expect("insert changesets");
        db.insert_history(changesets.clone(), None).expect("insert history");

        let account_occurrences = db.table::<tables::AccountsHistory>().unwrap().into_iter().fold(
            BTreeMap::<_, usize>::new(),
            |mut map, (key, _)| {
                map.entry(key.key).or_default().add_assign(1);
                map
            },
        );
        assert!(account_occurrences.into_iter().any(|(_, occurrences)| occurrences > 1));

        assert_eq!(
            db.table::<tables::AccountChangeSets>().unwrap().len(),
            changesets.iter().flatten().count()
        );

        let original_shards = db.table::<tables::AccountsHistory>().unwrap();

        let test_prune =
            |to_block: BlockNumber, run: usize, expected_result: (PruneProgress, usize)| {
                let prune_mode = PruneMode::Before(to_block);
                let deleted_entries_limit = 2000;
                let mut limiter =
                    PruneLimiter::default().set_deleted_entries_limit(deleted_entries_limit);
                let input = PruneInput {
                    previous_checkpoint: db
                        .factory
                        .provider()
                        .unwrap()
                        .get_prune_checkpoint(PruneSegment::AccountHistory)
                        .unwrap(),
                    to_block,
                    limiter: limiter.clone(),
                };
                let segment = AccountHistory::new(prune_mode);

                let provider = db.factory.database_provider_rw().unwrap();
                provider.set_storage_settings_cache(StorageSettings::v1());
                let result = segment.prune(&provider, input).unwrap();
                limiter.increment_deleted_entries_count_by(result.pruned);

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

                let changesets = changesets
                    .iter()
                    .enumerate()
                    .flat_map(|(block_number, changeset)| {
                        changeset.iter().map(move |change| (block_number, change))
                    })
                    .collect::<Vec<_>>();

                #[expect(clippy::skip_while_next)]
                let pruned = changesets
                    .iter()
                    .enumerate()
                    .skip_while(|(i, (block_number, _))| {
                        *i < deleted_entries_limit / ACCOUNT_HISTORY_TABLES_TO_PRUNE * run &&
                            *block_number <= to_block as usize
                    })
                    .next()
                    .map(|(i, _)| i)
                    .unwrap_or_default();

                // Skip what we've pruned so far, subtracting one to get last pruned block number
                // further down
                let mut pruned_changesets = changesets.iter().skip(pruned.saturating_sub(1));

                let last_pruned_block_number = pruned_changesets
                    .next()
                    .map(|(block_number, _)| if result.progress.is_finished() {
                        *block_number
                    } else {
                        block_number.saturating_sub(1)
                    } as BlockNumber)
                    .unwrap_or(to_block);

                let pruned_changesets = pruned_changesets.fold(
                    BTreeMap::<_, Vec<_>>::new(),
                    |mut acc, (block_number, change)| {
                        acc.entry(block_number).or_default().push(change);
                        acc
                    },
                );

                assert_eq!(
                    db.table::<tables::AccountChangeSets>().unwrap().len(),
                    pruned_changesets.values().flatten().count()
                );

                let actual_shards = db.table::<tables::AccountsHistory>().unwrap();

                let expected_shards = original_shards
                    .iter()
                    .filter(|(key, _)| key.highest_block_number > last_pruned_block_number)
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
                        .get_prune_checkpoint(PruneSegment::AccountHistory)
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
            1,
            (PruneProgress::HasMoreData(PruneInterruptReason::DeletedEntriesLimitReached), 1000),
        );
        test_prune(998, 2, (PruneProgress::Finished, 998));
        test_prune(1400, 3, (PruneProgress::Finished, 804));
    }

    /// Tests the `prune_static_files` code path. On unix with rocksdb feature, v2 storage
    /// routes to `prune_rocksdb` instead, so this test only runs without rocksdb (the
    /// `prune_rocksdb_path` test covers that configuration).
    #[test]
    #[cfg(not(all(unix, feature = "rocksdb")))]
    fn prune_static_file() {
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
            0..0,
            0..0,
        );

        db.insert_changesets_to_static_files(changesets.clone(), None)
            .expect("insert changesets to static files");
        db.insert_history(changesets.clone(), None).expect("insert history");

        let account_occurrences = db.table::<tables::AccountsHistory>().unwrap().into_iter().fold(
            BTreeMap::<_, usize>::new(),
            |mut map, (key, _)| {
                map.entry(key.key).or_default().add_assign(1);
                map
            },
        );
        assert!(account_occurrences.into_iter().any(|(_, occurrences)| occurrences > 1));

        let original_shards = db.table::<tables::AccountsHistory>().unwrap();

        let test_prune =
            |to_block: BlockNumber, run: usize, expected_result: (PruneProgress, usize)| {
                let prune_mode = PruneMode::Before(to_block);
                let deleted_entries_limit = 2000;
                let mut limiter =
                    PruneLimiter::default().set_deleted_entries_limit(deleted_entries_limit);
                let input = PruneInput {
                    previous_checkpoint: db
                        .factory
                        .provider()
                        .unwrap()
                        .get_prune_checkpoint(PruneSegment::AccountHistory)
                        .unwrap(),
                    to_block,
                    limiter: limiter.clone(),
                };
                let segment = AccountHistory::new(prune_mode);

                let provider = db.factory.database_provider_rw().unwrap();
                provider.set_storage_settings_cache(StorageSettings::v2());
                let result = segment.prune(&provider, input).unwrap();
                limiter.increment_deleted_entries_count_by(result.pruned);

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

                let changesets = changesets
                    .iter()
                    .enumerate()
                    .flat_map(|(block_number, changeset)| {
                        changeset.iter().map(move |change| (block_number, change))
                    })
                    .collect::<Vec<_>>();

                #[expect(clippy::skip_while_next)]
                let pruned = changesets
                    .iter()
                    .enumerate()
                    .skip_while(|(i, (block_number, _))| {
                        *i < deleted_entries_limit / ACCOUNT_HISTORY_TABLES_TO_PRUNE * run &&
                            *block_number <= to_block as usize
                    })
                    .next()
                    .map(|(i, _)| i)
                    .unwrap_or_default();

                // Skip what we've pruned so far, subtracting one to get last pruned block number
                // further down
                let mut pruned_changesets = changesets.iter().skip(pruned.saturating_sub(1));

                let last_pruned_block_number = pruned_changesets
                    .next()
                    .map(|(block_number, _)| {
                        (if result.progress.is_finished() {
                            *block_number
                        } else {
                            block_number.saturating_sub(1)
                        }) as BlockNumber
                    })
                    .unwrap_or(to_block);

                let actual_shards = db.table::<tables::AccountsHistory>().unwrap();

                let expected_shards = original_shards
                    .iter()
                    .filter(|(key, _)| key.highest_block_number > last_pruned_block_number)
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
                        .get_prune_checkpoint(PruneSegment::AccountHistory)
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
            1,
            (PruneProgress::HasMoreData(PruneInterruptReason::DeletedEntriesLimitReached), 1000),
        );
        test_prune(998, 2, (PruneProgress::Finished, 1000));
        test_prune(1400, 3, (PruneProgress::Finished, 804));
    }

    #[cfg(all(unix, feature = "rocksdb"))]
    #[test]
    fn prune_rocksdb_path() {
        use reth_db_api::models::ShardedKey;
        use reth_provider::{RocksDBProviderFactory, StaticFileProviderFactory};

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
            0..0,
            0..0,
        );

        db.insert_changesets_to_static_files(changesets.clone(), None)
            .expect("insert changesets to static files");

        let mut account_blocks: BTreeMap<_, Vec<u64>> = BTreeMap::new();
        for (block, changeset) in changesets.iter().enumerate() {
            for (address, _, _) in changeset {
                account_blocks.entry(*address).or_default().push(block as u64);
            }
        }

        let rocksdb = db.factory.rocksdb_provider();
        let mut batch = rocksdb.batch();
        for (address, block_numbers) in &account_blocks {
            let shard = BlockNumberList::new_pre_sorted(block_numbers.iter().copied());
            batch
                .put::<tables::AccountsHistory>(ShardedKey::new(*address, u64::MAX), &shard)
                .unwrap();
        }
        batch.commit().unwrap();

        for (address, expected_blocks) in &account_blocks {
            let shards = rocksdb.account_history_shards(*address).unwrap();
            assert_eq!(shards.len(), 1);
            assert_eq!(shards[0].1.iter().collect::<Vec<_>>(), *expected_blocks);
        }

        let to_block: BlockNumber = 50;
        let prune_mode = PruneMode::Before(to_block);
        let input =
            PruneInput { previous_checkpoint: None, to_block, limiter: PruneLimiter::default() };
        let segment = AccountHistory::new(prune_mode);

        db.factory.set_storage_settings_cache(StorageSettings::v2());

        let provider = db.factory.database_provider_rw().unwrap();
        let result = segment.prune(&provider, input).unwrap();
        provider.commit().expect("commit");

        assert_matches!(
            result,
            SegmentOutput { progress: PruneProgress::Finished, pruned, checkpoint: Some(_) }
                if pruned > 0
        );

        for (address, original_blocks) in &account_blocks {
            let shards = rocksdb.account_history_shards(*address).unwrap();

            let expected_blocks: Vec<u64> =
                original_blocks.iter().copied().filter(|b| *b > to_block).collect();

            if expected_blocks.is_empty() {
                assert!(
                    shards.is_empty(),
                    "Expected no shards for address {address:?} after pruning"
                );
            } else {
                assert_eq!(shards.len(), 1, "Expected 1 shard for address {address:?}");
                assert_eq!(
                    shards[0].1.iter().collect::<Vec<_>>(),
                    expected_blocks,
                    "Shard blocks mismatch for address {address:?}"
                );
            }
        }

        let static_file_provider = db.factory.static_file_provider();
        let highest_block = static_file_provider.get_highest_static_file_block(
            reth_static_file_types::StaticFileSegment::AccountChangeSets,
        );
        if let Some(block) = highest_block {
            assert!(
                block > to_block,
                "Static files should only contain blocks above to_block ({to_block}), got {block}"
            );
        }
    }

    /// Tests that when a limiter stops mid-block (with multiple changes for the same block),
    /// the checkpoint is set to `block_number - 1` to avoid dangling index entries.
    #[test]
    fn prune_partial_progress_mid_block() {
        use alloy_primitives::{Address, U256};
        use reth_primitives_traits::Account;
        use reth_testing_utils::generators::ChangeSet;

        let db = TestStageDB::default();
        let mut rng = generators::rng();

        // Create blocks 0..=10
        let blocks = random_block_range(
            &mut rng,
            0..=10,
            BlockRangeParams { parent: Some(B256::ZERO), tx_count: 0..1, ..Default::default() },
        );
        db.insert_blocks(blocks.iter(), StorageKind::Database(None)).expect("insert blocks");

        // Create specific changesets where block 5 has 4 account changes
        let addr1 = Address::with_last_byte(1);
        let addr2 = Address::with_last_byte(2);
        let addr3 = Address::with_last_byte(3);
        let addr4 = Address::with_last_byte(4);
        let addr5 = Address::with_last_byte(5);

        let account = Account { nonce: 1, balance: U256::from(100), bytecode_hash: None };

        // Build changesets: blocks 0-4 have 1 change each, block 5 has 4 changes, block 6 has 1
        let changesets: Vec<ChangeSet> = vec![
            vec![(addr1, account, vec![])], // block 0
            vec![(addr1, account, vec![])], // block 1
            vec![(addr1, account, vec![])], // block 2
            vec![(addr1, account, vec![])], // block 3
            vec![(addr1, account, vec![])], // block 4
            // block 5: 4 different account changes (sorted by address for consistency)
            vec![
                (addr1, account, vec![]),
                (addr2, account, vec![]),
                (addr3, account, vec![]),
                (addr4, account, vec![]),
            ],
            vec![(addr5, account, vec![])], // block 6
        ];

        db.insert_changesets(changesets.clone(), None).expect("insert changesets");
        db.insert_history(changesets.clone(), None).expect("insert history");

        // Total changesets: 5 (blocks 0-4) + 4 (block 5) + 1 (block 6) = 10
        assert_eq!(
            db.table::<tables::AccountChangeSets>().unwrap().len(),
            changesets.iter().flatten().count()
        );

        let prune_mode = PruneMode::Before(10);

        // Set limiter to stop after 7 entries (mid-block 5: 5 from blocks 0-4, then 2 of 4 from
        // block 5). Due to ACCOUNT_HISTORY_TABLES_TO_PRUNE=2, actual limit is 7/2=3
        // changesets. So we'll process blocks 0, 1, 2 (3 changesets), stopping before block
        // 3. Actually, let's use a higher limit to reach block 5. With limit=14, we get 7
        // changeset slots. Blocks 0-4 use 5 slots, leaving 2 for block 5 (which has 4), so
        // we stop mid-block 5.
        let deleted_entries_limit = 14; // 14/2 = 7 changeset entries before limit
        let limiter = PruneLimiter::default().set_deleted_entries_limit(deleted_entries_limit);

        let input = PruneInput { previous_checkpoint: None, to_block: 10, limiter };
        let segment = AccountHistory::new(prune_mode);

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

        // Verify checkpoint is set to block 4 (not 5), since block 5 is incomplete
        let checkpoint = db
            .factory
            .provider()
            .unwrap()
            .get_prune_checkpoint(PruneSegment::AccountHistory)
            .unwrap()
            .expect("checkpoint should exist");

        assert_eq!(
            checkpoint.block_number,
            Some(4),
            "Checkpoint should be block 4 (block before incomplete block 5)"
        );

        // Verify remaining changesets (block 5 and 6 should still have entries)
        let remaining_changesets = db.table::<tables::AccountChangeSets>().unwrap();
        // After pruning blocks 0-4, remaining should be block 5 (4 entries) + block 6 (1 entry) = 5
        // But since we stopped mid-block 5, some of block 5 might be pruned
        // However, checkpoint is 4, so on re-run we should re-process from block 5
        assert!(
            !remaining_changesets.is_empty(),
            "Should have remaining changesets for blocks 5-6"
        );

        // Verify no dangling history indices for blocks that weren't fully pruned
        // The indices for block 5 should still reference blocks <= 5 appropriately
        let history = db.table::<tables::AccountsHistory>().unwrap();
        for (key, _blocks) in &history {
            // All blocks in the history should be > checkpoint block number
            // OR the shard's highest_block_number should be > checkpoint
            assert!(
                key.highest_block_number > 4,
                "Found stale history shard with highest_block_number {} <= checkpoint 4",
                key.highest_block_number
            );
        }

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
            .get_prune_checkpoint(PruneSegment::AccountHistory)
            .unwrap()
            .expect("checkpoint should exist");

        // Should now be at block 6 (the last block with changesets)
        assert_eq!(final_checkpoint.block_number, Some(6), "Final checkpoint should be at block 6");

        // All changesets should be pruned
        let final_changesets = db.table::<tables::AccountChangeSets>().unwrap();
        assert!(final_changesets.is_empty(), "All changesets up to block 10 should be pruned");
    }

    /// Tests that when rocksdb feature is enabled but `account_history_in_rocksdb = false`,
    /// the pruner uses MDBX path instead of `RocksDB` path.
    ///
    /// This ensures storage settings are respected even when rocksdb feature is available.
    #[cfg(all(unix, feature = "rocksdb"))]
    #[test]
    fn prune_respects_storage_settings_mdbx_path() {
        use reth_db_api::models::ShardedKey;
        use reth_provider::RocksDBProviderFactory;

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
            0..0,
            0..0,
        );

        // Insert changesets and history into MDBX (not static files, not RocksDB)
        db.insert_changesets(changesets.clone(), None).expect("insert changesets");
        db.insert_history(changesets.clone(), None).expect("insert history");

        // Also insert some data into RocksDB to verify it's NOT touched
        let mut account_blocks: BTreeMap<_, Vec<u64>> = BTreeMap::new();
        for (block, changeset) in changesets.iter().enumerate() {
            for (address, _, _) in changeset {
                account_blocks.entry(*address).or_default().push(block as u64);
            }
        }

        let rocksdb = db.factory.rocksdb_provider();
        let mut batch = rocksdb.batch();
        for (address, block_numbers) in &account_blocks {
            let shard = BlockNumberList::new_pre_sorted(block_numbers.iter().copied());
            batch
                .put::<tables::AccountsHistory>(ShardedKey::new(*address, u64::MAX), &shard)
                .unwrap();
        }
        batch.commit().unwrap();

        // Record RocksDB state before pruning
        let rocksdb_shards_before: Vec<_> = account_blocks
            .keys()
            .map(|addr| (*addr, rocksdb.account_history_shards(*addr).unwrap()))
            .collect();

        // Record MDBX state before pruning
        let mdbx_history_before = db.table::<tables::AccountsHistory>().unwrap();
        assert!(!mdbx_history_before.is_empty(), "MDBX should have history data");

        let to_block: BlockNumber = 50;
        let prune_mode = PruneMode::Before(to_block);
        let input =
            PruneInput { previous_checkpoint: None, to_block, limiter: PruneLimiter::default() };
        let segment = AccountHistory::new(prune_mode);

        // Key: Set account_history_in_rocksdb = false (use MDBX path)
        db.factory.set_storage_settings_cache(
            StorageSettings::default()
                .with_account_changesets_in_static_files(false)
                .with_account_history_in_rocksdb(false),
        );

        let provider = db.factory.database_provider_rw().unwrap();
        let result = segment.prune(&provider, input).unwrap();
        provider.commit().expect("commit");

        assert_matches!(
            result,
            SegmentOutput { progress: PruneProgress::Finished, pruned, checkpoint: Some(_) }
                if pruned > 0
        );

        // Verify MDBX history was pruned (shards should be reduced or modified)
        let mdbx_history_after = db.table::<tables::AccountsHistory>().unwrap();
        assert!(
            mdbx_history_after.len() <= mdbx_history_before.len(),
            "MDBX history should be pruned or reduced"
        );

        // Verify RocksDB data was NOT touched
        let rocksdb = db.factory.rocksdb_provider();
        for (addr, shards_before) in &rocksdb_shards_before {
            let shards_after = rocksdb.account_history_shards(*addr).unwrap();
            assert_eq!(
                shards_before.len(),
                shards_after.len(),
                "RocksDB shards for {addr:?} should NOT be modified when account_history_in_rocksdb=false"
            );
            for ((key_before, blocks_before), (key_after, blocks_after)) in
                shards_before.iter().zip(shards_after.iter())
            {
                assert_eq!(key_before, key_after, "RocksDB shard key should be unchanged");
                assert_eq!(
                    blocks_before.iter().collect::<Vec<_>>(),
                    blocks_after.iter().collect::<Vec<_>>(),
                    "RocksDB shard blocks should be unchanged"
                );
            }
        }
    }
}
