//! `RocksDB` storage history index pruner.

use crate::{
    segments::{PruneInput, Segment},
    PrunerError,
};
use alloy_primitives::BlockNumber;
use reth_provider::{
    DBProvider, EitherWriter, PruneShardOutcome, RocksDBProviderFactory, StaticFileProviderFactory,
    StorageChangeSetReader, StorageSettingsCache,
};
use reth_prune_types::{
    PruneMode, PrunePurpose, PruneSegment, SegmentOutput, SegmentOutputCheckpoint,
};
use reth_static_file_types::StaticFileSegment;
use rustc_hash::FxHashMap;
use std::ops::RangeInclusive;
use tracing::{instrument, trace};

#[derive(Debug)]
pub struct StoragesHistoryPruner {
    mode: PruneMode,
}

impl StoragesHistoryPruner {
    pub const fn new(mode: PruneMode) -> Self {
        Self { mode }
    }

    /// Prune when storage changesets are in static files.
    fn prune_static_files<Provider>(
        &self,
        provider: &Provider,
        range: RangeInclusive<BlockNumber>,
        range_end: BlockNumber,
        input: PruneInput,
    ) -> Result<SegmentOutput, PrunerError>
    where
        Provider: StorageChangeSetReader + RocksDBProviderFactory + StaticFileProviderFactory,
    {
        let mut limiter = input.limiter;
        if limiter.is_limit_reached() {
            return Ok(SegmentOutput::not_done(
                limiter.interrupt_reason(),
                input.previous_checkpoint.map(SegmentOutputCheckpoint::from_prune_checkpoint),
            ))
        }

        let mut highest_deleted_storages = FxHashMap::default();
        let mut last_changeset_pruned_block = None;
        let mut scanned_changesets = 0usize;
        let mut done = true;

        for block in range {
            let changes = provider.storage_block_changeset(block)?;
            let changes_count = changes.len();

            for change in changes {
                highest_deleted_storages.insert((change.address, change.key), block);
            }

            scanned_changesets += changes_count;
            limiter.increment_deleted_entries_count_by(changes_count);
            last_changeset_pruned_block = Some(block);

            if limiter.is_limit_reached() {
                done = false;
                break;
            }
        }
        trace!(target: "pruner", scanned = %scanned_changesets, %done, "Scanned storage changesets");

        let last_changeset_pruned_block = last_changeset_pruned_block.unwrap_or(range_end);

        let mut keys_deleted = 0usize;
        let mut keys_updated = 0usize;

        provider.with_rocksdb_batch(|mut batch| {
            for ((address, storage_key), highest_block) in &highest_deleted_storages {
                let prune_to = (*highest_block).min(last_changeset_pruned_block);
                match batch.prune_storage_history_to(*address, *storage_key, prune_to)? {
                    PruneShardOutcome::Deleted => keys_deleted += 1,
                    PruneShardOutcome::Updated => keys_updated += 1,
                    PruneShardOutcome::Unchanged => {}
                }
            }
            Ok(((), Some(batch.into_inner())))
        })?;

        trace!(target: "pruner", keys_deleted, keys_updated, %done, "Pruned storage history (RocksDB indices)");

        // Delete static file jars below the pruned block
        provider.static_file_provider().delete_segment_below_block(
            StaticFileSegment::StorageChangeSets,
            last_changeset_pruned_block + 1,
        )?;

        let progress = limiter.progress(done);

        Ok(SegmentOutput {
            progress,
            pruned: scanned_changesets + keys_deleted,
            checkpoint: Some(SegmentOutputCheckpoint {
                block_number: Some(last_changeset_pruned_block),
                tx_number: None,
            }),
        })
    }

    /// Prune when storage changesets are in the database.
    fn prune_database<Provider>(
        &self,
        provider: &Provider,
        range: RangeInclusive<BlockNumber>,
        range_end: BlockNumber,
        input: PruneInput,
    ) -> Result<SegmentOutput, PrunerError>
    where
        Provider: StorageChangeSetReader + RocksDBProviderFactory,
    {
        let mut limiter = input.limiter;
        if limiter.is_limit_reached() {
            return Ok(SegmentOutput::not_done(
                limiter.interrupt_reason(),
                input.previous_checkpoint.map(SegmentOutputCheckpoint::from_prune_checkpoint),
            ))
        }

        let mut highest_deleted_storages = FxHashMap::default();
        let mut last_changeset_pruned_block = None;
        let mut scanned_changesets = 0usize;
        let mut done = true;

        for block in range {
            let changes = provider.storage_block_changeset(block)?;
            let changes_count = changes.len();

            for change in changes {
                highest_deleted_storages.insert((change.address, change.key), block);
            }

            scanned_changesets += changes_count;
            limiter.increment_deleted_entries_count_by(changes_count);
            last_changeset_pruned_block = Some(block);

            if limiter.is_limit_reached() {
                done = false;
                break;
            }
        }
        trace!(target: "pruner", scanned = %scanned_changesets, %done, "Scanned storage changesets");

        let last_changeset_pruned_block = last_changeset_pruned_block.unwrap_or(range_end);

        let mut keys_deleted = 0usize;
        let mut keys_updated = 0usize;

        provider.with_rocksdb_batch(|mut batch| {
            for ((address, storage_key), highest_block) in &highest_deleted_storages {
                let prune_to = (*highest_block).min(last_changeset_pruned_block);
                match batch.prune_storage_history_to(*address, *storage_key, prune_to)? {
                    PruneShardOutcome::Deleted => keys_deleted += 1,
                    PruneShardOutcome::Updated => keys_updated += 1,
                    PruneShardOutcome::Unchanged => {}
                }
            }
            Ok(((), Some(batch.into_inner())))
        })?;

        trace!(target: "pruner", keys_deleted, keys_updated, %done, "Pruned storage history (RocksDB indices)");

        let progress = limiter.progress(done);

        Ok(SegmentOutput {
            progress,
            pruned: scanned_changesets + keys_deleted,
            checkpoint: Some(SegmentOutputCheckpoint {
                block_number: Some(last_changeset_pruned_block),
                tx_number: None,
            }),
        })
    }
}

impl<Provider> Segment<Provider> for StoragesHistoryPruner
where
    Provider: DBProvider
        + StorageChangeSetReader
        + RocksDBProviderFactory
        + StaticFileProviderFactory
        + StorageSettingsCache,
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

    #[instrument(target = "pruner", skip(self, provider), ret(level = "trace"))]
    fn prune(&self, provider: &Provider, input: PruneInput) -> Result<SegmentOutput, PrunerError> {
        let range = match input.get_next_block_range() {
            Some(range) => range,
            None => {
                trace!(target: "pruner", "No storage history to prune");
                return Ok(SegmentOutput::done())
            }
        };
        let range_end = *range.end();

        if EitherWriter::storage_changesets_destination(provider).is_static_file() {
            self.prune_static_files(provider, range, range_end, input)
        } else {
            self.prune_database(provider, range, range_end, input)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::segments::{PruneInput, PruneLimiter, Segment};
    use alloy_primitives::{Address, B256, U256};
    use reth_db_api::{
        models::{storage_sharded_key::StorageShardedKey, BlockNumberAddress, StorageBeforeTx},
        tables,
        transaction::DbTxMut,
        BlockNumberList,
    };
    use reth_primitives_traits::StorageEntry;
    use reth_provider::{
        DatabaseProviderFactory, RocksDBProviderFactory, StaticFileProviderFactory,
        StaticFileWriter,
    };
    use reth_prune_types::{PruneMode, PruneProgress};
    use reth_stages::test_utils::{StorageKind, TestStageDB};
    use reth_static_file_types::StaticFileSegment;
    use reth_storage_api::{DBProvider, StorageSettings, StorageSettingsCache};
    use reth_testing_utils::generators::{self, random_block_range, BlockRangeParams};

    fn setup_rocksdb_test_db() -> TestStageDB {
        let db = TestStageDB::default();
        db.factory.set_storage_settings_cache(
            StorageSettings::legacy().with_storages_history_in_rocksdb(true),
        );
        db
    }

    /// Helper to write storage changesets to static files for testing.
    fn write_storage_changesets_to_static_files(
        db: &TestStageDB,
        changesets: &[(u64, Address, B256, U256)],
    ) {
        let static_file_provider = db.factory.static_file_provider();
        let mut writer =
            static_file_provider.latest_writer(StaticFileSegment::StorageChangeSets).unwrap();

        let mut current_block: Option<u64> = None;
        let mut block_entries = Vec::new();

        for (block_number, address, key, value) in changesets {
            if current_block != Some(*block_number) {
                if !block_entries.is_empty() {
                    writer
                        .append_storage_changeset(
                            std::mem::take(&mut block_entries),
                            current_block.unwrap(),
                        )
                        .unwrap();
                }
                current_block = Some(*block_number);
            }
            block_entries.push(StorageBeforeTx { address: *address, key: *key, value: *value });
        }

        if !block_entries.is_empty() {
            writer.append_storage_changeset(block_entries, current_block.unwrap()).unwrap();
        }

        writer.commit().unwrap();
        static_file_provider.initialize_index().unwrap();
    }

    /// Helper to write storage history to `RocksDB`.
    fn write_storage_history_to_rocksdb(db: &TestStageDB, history: &[(Address, B256, Vec<u64>)]) {
        let provider = db.factory.database_provider_rw().unwrap();
        provider
            .with_rocksdb_batch(|mut batch| {
                for (address, key, blocks) in history {
                    let shard = BlockNumberList::new_pre_sorted(blocks.iter().copied());
                    batch
                        .put::<tables::StoragesHistory>(
                            StorageShardedKey::last(*address, *key),
                            &shard,
                        )
                        .unwrap();
                }
                Ok(((), Some(batch.into_inner())))
            })
            .unwrap();
        provider.commit().unwrap();
    }

    /// Helper to read storage history from `RocksDB`.
    fn read_storage_history_from_rocksdb(
        db: &TestStageDB,
        address: Address,
        key: B256,
    ) -> Vec<u64> {
        let provider = db.factory.database_provider_rw().unwrap();
        let rocksdb = provider.rocksdb_provider();
        let shards = rocksdb.storage_history_shards(address, key).unwrap();
        shards.into_iter().flat_map(|(_, blocks)| blocks.iter().collect::<Vec<_>>()).collect()
    }

    #[test]
    fn test_prune_storage_history_static_files_full_range() {
        let db = setup_rocksdb_test_db();
        let mut rng = generators::rng();

        let blocks = random_block_range(
            &mut rng,
            0..=10,
            BlockRangeParams { parent: Some(B256::ZERO), tx_count: 0..1, ..Default::default() },
        );
        db.insert_blocks(blocks.iter(), StorageKind::Database(None)).expect("insert blocks");

        let addr1 = Address::from([0x11; 20]);
        let addr2 = Address::from([0x22; 20]);
        let key1 = B256::from([0x01; 32]);
        let key2 = B256::from([0x02; 32]);

        let changesets = vec![
            (0, addr1, key1, U256::from(50)),
            (1, addr1, key1, U256::from(100)),
            (1, addr1, key2, U256::from(200)),
            (2, addr1, key1, U256::from(110)),
            (2, addr2, key1, U256::from(300)),
            (3, addr1, key1, U256::from(120)),
            (3, addr2, key2, U256::from(400)),
        ];
        write_storage_changesets_to_static_files(&db, &changesets);

        write_storage_history_to_rocksdb(
            &db,
            &[
                (addr1, key1, vec![0, 1, 2, 3, 10, 20]),
                (addr1, key2, vec![1, 15]),
                (addr2, key1, vec![2, 25]),
                (addr2, key2, vec![3, 30]),
            ],
        );

        let segment = StoragesHistoryPruner::new(PruneMode::Before(3));
        let provider = db.factory.database_provider_rw().unwrap();

        let input =
            PruneInput { previous_checkpoint: None, to_block: 3, limiter: PruneLimiter::default() };

        let result = segment.prune(&provider, input).unwrap();
        provider.commit().unwrap();

        assert_eq!(result.progress, PruneProgress::Finished);
        assert!(result.pruned > 0);

        let history1 = read_storage_history_from_rocksdb(&db, addr1, key1);
        assert!(!history1.contains(&0));
        assert!(!history1.contains(&1));
        assert!(!history1.contains(&2));
        assert!(!history1.contains(&3));
        assert!(history1.contains(&10) || history1.contains(&20));

        let history2 = read_storage_history_from_rocksdb(&db, addr1, key2);
        assert!(!history2.contains(&1));

        let history3 = read_storage_history_from_rocksdb(&db, addr2, key1);
        assert!(!history3.contains(&2));

        let history4 = read_storage_history_from_rocksdb(&db, addr2, key2);
        assert!(!history4.contains(&3));
    }

    #[test]
    fn test_prune_storage_history_mdbx_fallback() {
        let db = setup_rocksdb_test_db();
        let mut rng = generators::rng();

        let blocks = random_block_range(
            &mut rng,
            1..=10,
            BlockRangeParams { parent: Some(B256::ZERO), tx_count: 0..1, ..Default::default() },
        );
        db.insert_blocks(blocks.iter(), StorageKind::Database(None)).expect("insert blocks");

        let addr1 = Address::from([0x11; 20]);
        let key1 = B256::from([0x01; 32]);

        db.commit(|tx| {
            for block in 1..=5u64 {
                tx.put::<tables::StorageChangeSets>(
                    BlockNumberAddress((block, addr1)),
                    StorageEntry { key: key1, value: U256::from(block * 100) },
                )?;
            }
            Ok(())
        })
        .unwrap();

        write_storage_history_to_rocksdb(&db, &[(addr1, key1, vec![1, 2, 3, 4, 5])]);

        let segment = StoragesHistoryPruner::new(PruneMode::Before(3));
        let provider = db.factory.database_provider_rw().unwrap();

        let input =
            PruneInput { previous_checkpoint: None, to_block: 3, limiter: PruneLimiter::default() };

        let result = segment.prune(&provider, input).unwrap();
        provider.commit().unwrap();

        assert_eq!(result.progress, PruneProgress::Finished);
        assert!(result.pruned > 0);
    }

    #[test]
    fn test_prune_storage_history_limiter_block_boundary() {
        let db = setup_rocksdb_test_db();
        let mut rng = generators::rng();

        let blocks = random_block_range(
            &mut rng,
            0..=20,
            BlockRangeParams { parent: Some(B256::ZERO), tx_count: 0..1, ..Default::default() },
        );
        db.insert_blocks(blocks.iter(), StorageKind::Database(None)).expect("insert blocks");

        let addr1 = Address::from([0x11; 20]);
        let key1 = B256::from([0x01; 32]);

        let mut changesets = Vec::new();
        for block in 0..=10u64 {
            for i in 0..10 {
                changesets.push((block, addr1, B256::from([i as u8; 32]), U256::from(block * 100)));
            }
        }
        write_storage_changesets_to_static_files(&db, &changesets);

        let all_blocks: Vec<u64> = (0..=10).collect();
        write_storage_history_to_rocksdb(&db, &[(addr1, key1, all_blocks)]);

        let segment = StoragesHistoryPruner::new(PruneMode::Before(10));
        let provider = db.factory.database_provider_rw().unwrap();

        let limiter = PruneLimiter::default().set_deleted_entries_limit(15);
        let input = PruneInput { previous_checkpoint: None, to_block: 10, limiter };

        let result = segment.prune(&provider, input).unwrap();
        provider.commit().unwrap();

        assert_eq!(
            result.progress,
            PruneProgress::HasMoreData(
                reth_prune_types::PruneInterruptReason::DeletedEntriesLimitReached
            ),
            "Should stop due to limit"
        );

        let checkpoint = result.checkpoint.expect("should have checkpoint");
        let pruned_block = checkpoint.block_number.expect("should have block number");
        assert!(pruned_block < 10, "Should have stopped before target block");
        assert!(pruned_block > 0, "Should have pruned at least some blocks");
    }

    #[test]
    fn test_prune_storage_history_empty_range() {
        let db = setup_rocksdb_test_db();
        let mut rng = generators::rng();

        let blocks = random_block_range(
            &mut rng,
            1..=5,
            BlockRangeParams { parent: Some(B256::ZERO), tx_count: 0..1, ..Default::default() },
        );
        db.insert_blocks(blocks.iter(), StorageKind::Database(None)).expect("insert blocks");

        let segment = StoragesHistoryPruner::new(PruneMode::Before(5));
        let provider = db.factory.database_provider_rw().unwrap();

        let input = PruneInput {
            previous_checkpoint: Some(reth_prune_types::PruneCheckpoint {
                block_number: Some(5),
                tx_number: None,
                prune_mode: PruneMode::Before(5),
            }),
            to_block: 5,
            limiter: PruneLimiter::default(),
        };

        let result = segment.prune(&provider, input).unwrap();

        assert_eq!(result.progress, PruneProgress::Finished);
        assert_eq!(result.pruned, 0, "Nothing to prune");
    }

    #[test]
    fn test_prune_storage_history_mixed_static_and_mdbx() {
        let db = setup_rocksdb_test_db();
        let mut rng = generators::rng();

        let blocks = random_block_range(
            &mut rng,
            0..=20,
            BlockRangeParams { parent: Some(B256::ZERO), tx_count: 0..1, ..Default::default() },
        );
        db.insert_blocks(blocks.iter(), StorageKind::Database(None)).expect("insert blocks");

        let addr1 = Address::from([0x11; 20]);
        let key1 = B256::from([0x01; 32]);

        let mut changesets = Vec::new();
        for block in 0..=10u64 {
            changesets.push((block, addr1, key1, U256::from(block * 100)));
        }
        write_storage_changesets_to_static_files(&db, &changesets);

        db.commit(|tx| {
            for block in 11..=20u64 {
                tx.put::<tables::StorageChangeSets>(
                    BlockNumberAddress((block, addr1)),
                    StorageEntry { key: key1, value: U256::from(block * 100) },
                )?;
            }
            Ok(())
        })
        .unwrap();

        write_storage_history_to_rocksdb(&db, &[(addr1, key1, (0..=20).collect())]);

        let segment = StoragesHistoryPruner::new(PruneMode::Before(15));
        let provider = db.factory.database_provider_rw().unwrap();

        let input = PruneInput {
            previous_checkpoint: None,
            to_block: 15,
            limiter: PruneLimiter::default(),
        };

        let result = segment.prune(&provider, input).unwrap();
        provider.commit().unwrap();

        assert_eq!(result.progress, PruneProgress::Finished);
        assert!(result.pruned > 0, "Should have pruned entries from both static and MDBX");

        let checkpoint = result.checkpoint.expect("should have checkpoint");
        assert_eq!(checkpoint.block_number, Some(15));
    }
}
