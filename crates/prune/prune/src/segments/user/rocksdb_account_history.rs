//! `RocksDB` account history index pruner.

use crate::{
    segments::{PruneInput, Segment},
    PrunerError,
};
use reth_provider::{
    ChangeSetReader, DBProvider, EitherWriter, PruneShardOutcome, RocksDBProviderFactory,
    StaticFileProviderFactory, StorageSettingsCache,
};
use reth_prune_types::{
    PruneMode, PrunePurpose, PruneSegment, SegmentOutput, SegmentOutputCheckpoint,
};
use reth_static_file_types::StaticFileSegment;
use rustc_hash::FxHashMap;
use std::ops::RangeInclusive;
use tracing::{instrument, trace};

#[derive(Debug)]
pub struct AccountsHistoryPruner {
    mode: PruneMode,
}

impl AccountsHistoryPruner {
    pub const fn new(mode: PruneMode) -> Self {
        Self { mode }
    }
}

impl AccountsHistoryPruner {
    fn prune_static_files<Provider>(
        &self,
        provider: &Provider,
        range: RangeInclusive<u64>,
        range_end: u64,
        input: PruneInput,
    ) -> Result<SegmentOutput, PrunerError>
    where
        Provider: ChangeSetReader + RocksDBProviderFactory + StaticFileProviderFactory,
    {
        let mut limiter = input.limiter;
        if limiter.is_limit_reached() {
            return Ok(SegmentOutput::not_done(
                limiter.interrupt_reason(),
                input.previous_checkpoint.map(SegmentOutputCheckpoint::from_prune_checkpoint),
            ))
        }

        let mut highest_deleted_accounts = FxHashMap::default();
        let mut last_changeset_pruned_block = None;
        let mut scanned_changesets = 0usize;
        let mut done = true;

        for block in range {
            let changes = provider.account_block_changeset(block)?;
            let changes_count = changes.len();

            for change in changes {
                highest_deleted_accounts.insert(change.address, block);
            }

            scanned_changesets += changes_count;
            limiter.increment_deleted_entries_count_by(changes_count);
            last_changeset_pruned_block = Some(block);

            if limiter.is_limit_reached() {
                done = false;
                break;
            }
        }
        trace!(target: "pruner", scanned = %scanned_changesets, %done, "Scanned account changesets");

        let last_changeset_pruned_block = last_changeset_pruned_block.unwrap_or(range_end);

        let mut keys_deleted = 0usize;
        let mut keys_updated = 0usize;

        provider.with_rocksdb_batch(|mut batch| {
            for (address, highest_block) in &highest_deleted_accounts {
                let prune_to = (*highest_block).min(last_changeset_pruned_block);
                match batch.prune_account_history_to(*address, prune_to)? {
                    PruneShardOutcome::Deleted => keys_deleted += 1,
                    PruneShardOutcome::Updated => keys_updated += 1,
                    PruneShardOutcome::Unchanged => {}
                }
            }
            Ok(((), Some(batch.into_inner())))
        })?;

        trace!(target: "pruner", keys_deleted, keys_updated, %done, "Pruned account history (RocksDB indices)");

        if done {
            provider.static_file_provider().delete_segment_below_block(
                StaticFileSegment::AccountChangeSets,
                last_changeset_pruned_block + 1,
            )?;
        }

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

    fn prune_database<Provider>(
        &self,
        provider: &Provider,
        range: RangeInclusive<u64>,
        range_end: u64,
        input: PruneInput,
    ) -> Result<SegmentOutput, PrunerError>
    where
        Provider: ChangeSetReader + RocksDBProviderFactory,
    {
        let mut limiter = input.limiter;
        if limiter.is_limit_reached() {
            return Ok(SegmentOutput::not_done(
                limiter.interrupt_reason(),
                input.previous_checkpoint.map(SegmentOutputCheckpoint::from_prune_checkpoint),
            ))
        }

        let mut highest_deleted_accounts = FxHashMap::default();
        let mut last_changeset_pruned_block = None;
        let mut scanned_changesets = 0usize;
        let mut done = true;

        for block in range {
            let changes = provider.account_block_changeset(block)?;
            let changes_count = changes.len();

            for change in changes {
                highest_deleted_accounts.insert(change.address, block);
            }

            scanned_changesets += changes_count;
            limiter.increment_deleted_entries_count_by(changes_count);
            last_changeset_pruned_block = Some(block);

            if limiter.is_limit_reached() {
                done = false;
                break;
            }
        }
        trace!(target: "pruner", scanned = %scanned_changesets, %done, "Scanned account changesets");

        let last_changeset_pruned_block = last_changeset_pruned_block.unwrap_or(range_end);

        let mut keys_deleted = 0usize;
        let mut keys_updated = 0usize;

        provider.with_rocksdb_batch(|mut batch| {
            for (address, highest_block) in &highest_deleted_accounts {
                let prune_to = (*highest_block).min(last_changeset_pruned_block);
                match batch.prune_account_history_to(*address, prune_to)? {
                    PruneShardOutcome::Deleted => keys_deleted += 1,
                    PruneShardOutcome::Updated => keys_updated += 1,
                    PruneShardOutcome::Unchanged => {}
                }
            }
            Ok(((), Some(batch.into_inner())))
        })?;

        trace!(target: "pruner", keys_deleted, keys_updated, %done, "Pruned account history (RocksDB indices)");

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

impl<Provider> Segment<Provider> for AccountsHistoryPruner
where
    Provider: ChangeSetReader
        + RocksDBProviderFactory
        + StaticFileProviderFactory
        + StorageSettingsCache
        + DBProvider,
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

    #[instrument(target = "pruner", skip(self, provider), ret(level = "trace"))]
    fn prune(&self, provider: &Provider, input: PruneInput) -> Result<SegmentOutput, PrunerError> {
        let range = match input.get_next_block_range() {
            Some(range) => range,
            None => {
                trace!(target: "pruner", "No account history to prune");
                return Ok(SegmentOutput::done())
            }
        };
        let range_end = *range.end();

        if EitherWriter::account_changesets_destination(provider).is_static_file() {
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
    use alloy_primitives::{Address, B256};
    use reth_db_api::{
        models::{AccountBeforeTx, ShardedKey},
        tables,
        transaction::DbTxMut,
        BlockNumberList,
    };
    use reth_provider::{
        DatabaseProviderFactory, RocksDBProviderFactory, StaticFileProviderFactory,
        StaticFileWriter,
    };
    use reth_prune_types::{PruneMode, PruneProgress};
    use reth_stages::test_utils::{StorageKind, TestStageDB};
    use reth_static_file_types::StaticFileSegment;
    use reth_storage_api::{DBProvider, StorageSettings, StorageSettingsCache};
    use reth_testing_utils::generators::{
        self, random_block_range, random_changeset_range, random_eoa_accounts, BlockRangeParams,
    };
    use std::collections::BTreeMap;

    fn setup_rocksdb_test_db() -> TestStageDB {
        let db = TestStageDB::default();
        db.factory.set_storage_settings_cache(
            StorageSettings::legacy().with_account_history_in_rocksdb(true),
        );
        db
    }

    fn generate_test_changeset(_block: u64, addresses: &[Address]) -> Vec<AccountBeforeTx> {
        addresses.iter().map(|&address| AccountBeforeTx { address, info: None }).collect()
    }

    /// Helper to write account history to RocksDB.
    fn write_account_history_to_rocksdb(db: &TestStageDB, history: &[(Address, Vec<u64>)]) {
        let provider = db.factory.database_provider_rw().unwrap();
        provider
            .with_rocksdb_batch(|mut batch| {
                for (address, blocks) in history {
                    let shard = BlockNumberList::new_pre_sorted(blocks.iter().copied());
                    batch
                        .put::<tables::AccountsHistory>(ShardedKey::last(*address), &shard)
                        .unwrap();
                }
                Ok(((), Some(batch.into_inner())))
            })
            .unwrap();
        provider.commit().unwrap();
    }

    #[test]
    fn test_prune_account_history_static_files_full_range() {
        let db = setup_rocksdb_test_db();
        let mut rng = generators::rng();

        let blocks = random_block_range(
            &mut rng,
            0..=10,
            BlockRangeParams { parent: Some(B256::ZERO), tx_count: 0..1, ..Default::default() },
        );
        db.insert_blocks(blocks.iter(), StorageKind::Database(None)).expect("insert blocks");

        let addresses: Vec<Address> = (0..3)
            .map(|i| {
                let mut addr = Address::ZERO;
                addr.0[0] = i as u8;
                addr
            })
            .collect();

        let static_file_provider = db.factory.static_file_provider();
        {
            let mut writer = static_file_provider
                .latest_writer(StaticFileSegment::AccountChangeSets)
                .expect("get writer");

            for block_num in 0..=10 {
                let changeset = generate_test_changeset(block_num, &addresses);
                writer.append_account_changeset(changeset, block_num).expect("append changeset");
            }
            writer.commit().expect("commit static file");
        }

        let highest = static_file_provider
            .get_highest_static_file_block(StaticFileSegment::AccountChangeSets);
        assert_eq!(highest, Some(10), "Static file should cover blocks 0-10");

        // Write account history to RocksDB (not MDBX)
        let history: Vec<_> = addresses.iter().map(|&addr| (addr, (0..=10).collect())).collect();
        write_account_history_to_rocksdb(&db, &history);

        let prune_mode = PruneMode::Before(5);
        let input =
            PruneInput { previous_checkpoint: None, to_block: 5, limiter: PruneLimiter::default() };
        let segment = AccountsHistoryPruner::new(prune_mode);

        let provider = db.factory.database_provider_rw().unwrap();
        let result = segment.prune(&provider, input).expect("prune should succeed");
        provider.commit().expect("commit after prune");

        assert_eq!(result.progress, PruneProgress::Finished);
        assert!(result.pruned > 0, "Should have pruned some entries");

        let checkpoint = result.checkpoint.expect("should have checkpoint");
        assert_eq!(checkpoint.block_number, Some(5));
    }

    #[test]
    fn test_prune_account_history_mdbx_fallback() {
        let db = setup_rocksdb_test_db();
        let mut rng = generators::rng();

        let blocks = random_block_range(
            &mut rng,
            1..=10,
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
        db.insert_history(changesets, None).expect("insert history");

        let mdbx_count_before = db.table::<tables::AccountChangeSets>().unwrap().len();
        assert!(mdbx_count_before > 0, "Should have MDBX changesets");

        let static_file_provider = db.factory.static_file_provider();
        let highest = static_file_provider
            .get_highest_static_file_block(StaticFileSegment::AccountChangeSets);
        assert!(highest.is_none(), "No static file coverage expected");

        let prune_mode = PruneMode::Before(5);
        let input =
            PruneInput { previous_checkpoint: None, to_block: 5, limiter: PruneLimiter::default() };
        let segment = AccountsHistoryPruner::new(prune_mode);

        let provider = db.factory.database_provider_rw().unwrap();
        let result = segment.prune(&provider, input).expect("prune should succeed");
        provider.commit().expect("commit after prune");

        assert_eq!(result.progress, PruneProgress::Finished);
        assert!(result.pruned > 0, "Should have pruned some entries");
    }

    #[test]
    fn test_prune_account_history_limiter_block_boundary() {
        let db = setup_rocksdb_test_db();
        let mut rng = generators::rng();

        let blocks = random_block_range(
            &mut rng,
            0..=20,
            BlockRangeParams { parent: Some(B256::ZERO), tx_count: 0..1, ..Default::default() },
        );
        db.insert_blocks(blocks.iter(), StorageKind::Database(None)).expect("insert blocks");

        let addresses: Vec<Address> = (0..10)
            .map(|i| {
                let mut addr = Address::ZERO;
                addr.0[0] = i as u8;
                addr
            })
            .collect();

        let static_file_provider = db.factory.static_file_provider();
        {
            let mut writer = static_file_provider
                .latest_writer(StaticFileSegment::AccountChangeSets)
                .expect("get writer");

            for block_num in 0..=20 {
                let changeset = generate_test_changeset(block_num, &addresses);
                writer.append_account_changeset(changeset, block_num).expect("append changeset");
            }
            writer.commit().expect("commit static file");
        }

        let provider = db.factory.database_provider_rw().unwrap();
        for block_num in 0..=20u64 {
            for addr in &addresses {
                provider
                    .tx_ref()
                    .put::<tables::AccountsHistory>(
                        reth_db_api::models::ShardedKey::new(*addr, block_num),
                        reth_db_api::BlockNumberList::new([block_num]).unwrap(),
                    )
                    .expect("insert history");
            }
        }
        provider.commit().expect("commit provider");

        let prune_mode = PruneMode::Before(15);
        let limiter = PruneLimiter::default().set_deleted_entries_limit(25);
        let input = PruneInput { previous_checkpoint: None, to_block: 15, limiter };
        let segment = AccountsHistoryPruner::new(prune_mode);

        let provider = db.factory.database_provider_rw().unwrap();
        let result = segment.prune(&provider, input).expect("prune should succeed");
        provider.commit().expect("commit after prune");

        assert_eq!(
            result.progress,
            PruneProgress::HasMoreData(
                reth_prune_types::PruneInterruptReason::DeletedEntriesLimitReached
            ),
            "Should stop due to limit"
        );

        let checkpoint = result.checkpoint.expect("should have checkpoint");
        let pruned_block = checkpoint.block_number.expect("should have block number");
        assert!(pruned_block < 15, "Should have stopped before target block");
        assert!(pruned_block >= 0, "Should have pruned at least one block");
    }

    #[test]
    fn test_prune_account_history_empty_range() {
        let db = setup_rocksdb_test_db();
        let mut rng = generators::rng();

        let blocks = random_block_range(
            &mut rng,
            1..=5,
            BlockRangeParams { parent: Some(B256::ZERO), tx_count: 0..1, ..Default::default() },
        );
        db.insert_blocks(blocks.iter(), StorageKind::Database(None)).expect("insert blocks");

        let prune_mode = PruneMode::Before(5);
        let input = PruneInput {
            previous_checkpoint: Some(reth_prune_types::PruneCheckpoint {
                block_number: Some(5),
                tx_number: None,
                prune_mode,
            }),
            to_block: 5,
            limiter: PruneLimiter::default(),
        };
        let segment = AccountsHistoryPruner::new(prune_mode);

        let provider = db.factory.database_provider_rw().unwrap();
        let result = segment.prune(&provider, input).expect("prune should succeed");

        assert_eq!(result.progress, PruneProgress::Finished);
        assert_eq!(result.pruned, 0, "Nothing to prune");
    }

    #[test]
    fn test_prune_account_history_mixed_static_and_mdbx() {
        let db = setup_rocksdb_test_db();
        let mut rng = generators::rng();

        let blocks = random_block_range(
            &mut rng,
            0..=20,
            BlockRangeParams { parent: Some(B256::ZERO), tx_count: 0..1, ..Default::default() },
        );
        db.insert_blocks(blocks.iter(), StorageKind::Database(None)).expect("insert blocks");

        let addresses: Vec<Address> = (0..3)
            .map(|i| {
                let mut addr = Address::ZERO;
                addr.0[0] = i as u8;
                addr
            })
            .collect();

        let static_file_provider = db.factory.static_file_provider();
        {
            let mut writer = static_file_provider
                .latest_writer(StaticFileSegment::AccountChangeSets)
                .expect("get writer");

            for block_num in 0..=10 {
                let changeset = generate_test_changeset(block_num, &addresses);
                writer.append_account_changeset(changeset, block_num).expect("append changeset");
            }
            writer.commit().expect("commit static file");
        }

        db.commit(|tx| {
            for block_num in 11..=20u64 {
                for addr in &addresses {
                    tx.put::<tables::AccountChangeSets>(
                        block_num,
                        AccountBeforeTx { address: *addr, info: None },
                    )?;
                }
            }
            Ok(())
        })
        .expect("insert MDBX changesets");

        let provider = db.factory.database_provider_rw().unwrap();
        for block_num in 0..=20u64 {
            for addr in &addresses {
                provider
                    .tx_ref()
                    .put::<tables::AccountsHistory>(
                        reth_db_api::models::ShardedKey::new(*addr, block_num),
                        reth_db_api::BlockNumberList::new([block_num]).unwrap(),
                    )
                    .expect("insert history");
            }
        }
        provider.commit().expect("commit provider");

        let prune_mode = PruneMode::Before(15);
        let input = PruneInput {
            previous_checkpoint: None,
            to_block: 15,
            limiter: PruneLimiter::default(),
        };
        let segment = AccountsHistoryPruner::new(prune_mode);

        let provider = db.factory.database_provider_rw().unwrap();
        let result = segment.prune(&provider, input).expect("prune should succeed");
        provider.commit().expect("commit after prune");

        assert_eq!(result.progress, PruneProgress::Finished);
        assert!(result.pruned > 0, "Should have pruned entries from both static and MDBX");

        let checkpoint = result.checkpoint.expect("should have checkpoint");
        assert_eq!(checkpoint.block_number, Some(15));
    }
}
