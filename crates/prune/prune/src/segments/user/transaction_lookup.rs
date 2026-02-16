use crate::{
    db_ext::DbTxPruneExt,
    segments::{PruneInput, Segment, SegmentOutput},
    PrunerError,
};
use alloy_eips::eip2718::Encodable2718;
use rayon::prelude::*;
use reth_db_api::{tables, transaction::DbTxMut};
use reth_provider::{
    BlockReader, DBProvider, PruneCheckpointReader, RocksDBProviderFactory,
    StaticFileProviderFactory,
};
use reth_prune_types::{
    PruneCheckpoint, PruneMode, PruneProgress, PrunePurpose, PruneSegment, SegmentOutputCheckpoint,
};
use reth_static_file_types::StaticFileSegment;
use reth_storage_api::StorageSettingsCache;
use tracing::{debug, instrument, trace};

#[derive(Debug)]
pub struct TransactionLookup {
    mode: PruneMode,
}

impl TransactionLookup {
    pub const fn new(mode: PruneMode) -> Self {
        Self { mode }
    }
}

impl<Provider> Segment<Provider> for TransactionLookup
where
    Provider: DBProvider<Tx: DbTxMut>
        + BlockReader<Transaction: Encodable2718>
        + PruneCheckpointReader
        + StaticFileProviderFactory
        + StorageSettingsCache
        + RocksDBProviderFactory,
{
    fn segment(&self) -> PruneSegment {
        PruneSegment::TransactionLookup
    }

    fn mode(&self) -> Option<PruneMode> {
        Some(self.mode)
    }

    fn purpose(&self) -> PrunePurpose {
        PrunePurpose::User
    }

    #[instrument(
        name = "TransactionLookup::prune",
        target = "pruner",
        skip(self, provider),
        ret(level = "trace")
    )]
    fn prune(
        &self,
        provider: &Provider,
        mut input: PruneInput,
    ) -> Result<SegmentOutput, PrunerError> {
        // It is not possible to prune TransactionLookup data for which we don't have transaction
        // data. If the TransactionLookup checkpoint is lagging behind (which can happen e.g. when
        // pre-merge history is dropped and then later tx lookup pruning is enabled) then we can
        // only prune from the lowest static file.
        if let Some(lowest_range) =
            provider.static_file_provider().get_lowest_range(StaticFileSegment::Transactions) &&
            input
                .previous_checkpoint
                .is_none_or(|checkpoint| checkpoint.block_number < Some(lowest_range.start()))
        {
            let new_checkpoint = lowest_range.start().saturating_sub(1);
            if let Some(body_indices) = provider.block_body_indices(new_checkpoint)? {
                input.previous_checkpoint = Some(PruneCheckpoint {
                    block_number: Some(new_checkpoint),
                    tx_number: Some(body_indices.last_tx_num()),
                    prune_mode: self.mode,
                });
                debug!(
                    target: "pruner",
                    static_file_checkpoint = ?input.previous_checkpoint,
                    "Using static file transaction checkpoint as TransactionLookup starting point"
                );
            }
        }

        let (start, end) = match input.get_next_tx_num_range(provider)? {
            Some(range) => range,
            None => {
                trace!(target: "pruner", "No transaction lookup entries to prune");
                return Ok(SegmentOutput::done())
            }
        }
        .into_inner();

        // Check where transaction hash numbers are stored
        #[cfg(all(unix, feature = "rocksdb"))]
        if provider.cached_storage_settings().storage_v2 {
            return self.prune_rocksdb(provider, input, start, end);
        }

        // For PruneMode::Full, clear the entire table in one operation
        if self.mode.is_full() {
            let pruned = provider.tx_ref().clear_table::<tables::TransactionHashNumbers>()?;
            trace!(target: "pruner", %pruned, "Cleared transaction lookup table");

            let last_pruned_block = provider
                .block_by_transaction_id(end)?
                .ok_or(PrunerError::InconsistentData("Block for transaction is not found"))?;

            return Ok(SegmentOutput {
                progress: PruneProgress::Finished,
                pruned,
                checkpoint: Some(SegmentOutputCheckpoint {
                    block_number: Some(last_pruned_block),
                    tx_number: Some(end),
                }),
            });
        }

        let tx_range = start..=
            Some(end)
                .min(
                    input
                        .limiter
                        .deleted_entries_limit_left()
                        // Use saturating addition here to avoid panicking on
                        // `deleted_entries_limit == usize::MAX`
                        .map(|left| start.saturating_add(left as u64) - 1),
                )
                .unwrap();
        let tx_range_end = *tx_range.end();

        // Retrieve transactions in the range and calculate their hashes in parallel
        let mut hashes = provider
            .transactions_by_tx_range(tx_range.clone())?
            .into_par_iter()
            .map(|transaction| transaction.trie_hash())
            .collect::<Vec<_>>();

        // Sort hashes to enable efficient cursor traversal through the TransactionHashNumbers
        // table, which is keyed by hash. Without sorting, each seek would be O(log n) random
        // access; with sorting, the cursor advances sequentially through the B+tree.
        hashes.sort_unstable();

        // Number of transactions retrieved from the database should match the tx range count
        let tx_count = tx_range.count();
        if hashes.len() != tx_count {
            return Err(PrunerError::InconsistentData(
                "Unexpected number of transaction hashes retrieved by transaction number range",
            ))
        }

        let mut limiter = input.limiter;

        let mut last_pruned_transaction = None;
        let (pruned, done) =
            provider.tx_ref().prune_table_with_iterator::<tables::TransactionHashNumbers>(
                hashes,
                &mut limiter,
                |row| {
                    last_pruned_transaction =
                        Some(last_pruned_transaction.unwrap_or(row.1).max(row.1))
                },
            )?;

        let done = done && tx_range_end == end;
        trace!(target: "pruner", %pruned, %done, "Pruned transaction lookup");

        let last_pruned_transaction = last_pruned_transaction.unwrap_or(tx_range_end);

        let last_pruned_block = provider
            .block_by_transaction_id(last_pruned_transaction)?
            .ok_or(PrunerError::InconsistentData("Block for transaction is not found"))?
            // If there's more transaction lookup entries to prune, set the checkpoint block number
            // to previous, so we could finish pruning its transaction lookup entries on the next
            // run.
            .checked_sub(if done { 0 } else { 1 });

        let progress = limiter.progress(done);

        Ok(SegmentOutput {
            progress,
            pruned,
            checkpoint: Some(SegmentOutputCheckpoint {
                block_number: last_pruned_block,
                tx_number: Some(last_pruned_transaction),
            }),
        })
    }
}

impl TransactionLookup {
    /// Prunes transaction lookup when indices are stored in `RocksDB`.
    ///
    /// Reads transactions from static files and deletes corresponding entries
    /// from the `RocksDB` `TransactionHashNumbers` table.
    #[cfg(all(unix, feature = "rocksdb"))]
    fn prune_rocksdb<Provider>(
        &self,
        provider: &Provider,
        input: PruneInput,
        start: alloy_primitives::TxNumber,
        end: alloy_primitives::TxNumber,
    ) -> Result<SegmentOutput, PrunerError>
    where
        Provider: DBProvider
            + BlockReader<Transaction: Encodable2718>
            + StaticFileProviderFactory
            + RocksDBProviderFactory,
    {
        // For PruneMode::Full, clear the entire RocksDB table in one operation
        if self.mode.is_full() {
            let rocksdb = provider.rocksdb_provider();
            rocksdb.clear::<tables::TransactionHashNumbers>()?;
            trace!(target: "pruner", "Cleared transaction lookup table (RocksDB)");

            let last_pruned_block = provider
                .block_by_transaction_id(end)?
                .ok_or(PrunerError::InconsistentData("Block for transaction is not found"))?;

            return Ok(SegmentOutput {
                progress: PruneProgress::Finished,
                pruned: 0, // RocksDB clear doesn't return count
                checkpoint: Some(SegmentOutputCheckpoint {
                    block_number: Some(last_pruned_block),
                    tx_number: Some(end),
                }),
            });
        }

        let tx_range_end = input
            .limiter
            .deleted_entries_limit_left()
            .map(|left| start.saturating_add(left as u64).saturating_sub(1))
            .map_or(end, |limited| limited.min(end));
        let tx_range = start..=tx_range_end;

        // Retrieve transactions in the range and calculate their hashes in parallel
        let hashes: Vec<_> = provider
            .transactions_by_tx_range(tx_range.clone())?
            .into_par_iter()
            .map(|transaction| transaction.trie_hash())
            .collect();

        // Number of transactions retrieved from the database should match the tx range count
        let tx_count = tx_range.count();
        if hashes.len() != tx_count {
            return Err(PrunerError::InconsistentData(
                "Unexpected number of transaction hashes retrieved by transaction number range",
            ))
        }

        let mut limiter = input.limiter;

        // Delete transaction hash -> number mappings from RocksDB
        let mut deleted = 0usize;
        provider.with_rocksdb_batch(|mut batch| {
            for hash in &hashes {
                if limiter.is_limit_reached() {
                    break;
                }
                batch.delete::<tables::TransactionHashNumbers>(*hash)?;
                limiter.increment_deleted_entries_count();
                deleted += 1;
            }
            Ok(((), Some(batch.into_inner())))
        })?;

        let done = deleted == hashes.len() && tx_range_end == end;
        trace!(target: "pruner", %deleted, %done, "Pruned transaction lookup (RocksDB)");

        let last_pruned_transaction =
            if deleted > 0 { start + deleted as u64 - 1 } else { tx_range_end };

        let last_pruned_block = provider
            .block_by_transaction_id(last_pruned_transaction)?
            .ok_or(PrunerError::InconsistentData("Block for transaction is not found"))?
            .checked_sub(if done { 0 } else { 1 });

        let progress = limiter.progress(done);

        Ok(SegmentOutput {
            progress,
            pruned: deleted,
            checkpoint: Some(SegmentOutputCheckpoint {
                block_number: last_pruned_block,
                tx_number: Some(last_pruned_transaction),
            }),
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::segments::{PruneInput, PruneLimiter, Segment, SegmentOutput, TransactionLookup};
    use alloy_primitives::{BlockNumber, TxNumber, B256};
    use assert_matches::assert_matches;
    use itertools::{
        FoldWhile::{Continue, Done},
        Itertools,
    };
    use reth_db_api::tables;
    use reth_provider::{DBProvider, DatabaseProviderFactory, PruneCheckpointReader};
    use reth_prune_types::{
        PruneCheckpoint, PruneInterruptReason, PruneMode, PruneProgress, PruneSegment,
    };
    use reth_stages::test_utils::{StorageKind, TestStageDB};
    use reth_testing_utils::generators::{self, random_block_range, BlockRangeParams};
    use std::ops::Sub;

    #[test]
    fn prune() {
        let db = TestStageDB::default();
        let mut rng = generators::rng();

        let blocks = random_block_range(
            &mut rng,
            1..=10,
            BlockRangeParams { parent: Some(B256::ZERO), tx_count: 2..3, ..Default::default() },
        );
        db.insert_blocks(blocks.iter(), StorageKind::Static).expect("insert blocks");

        let mut tx_hash_numbers = Vec::new();
        for block in &blocks {
            tx_hash_numbers.reserve_exact(block.transaction_count());
            for transaction in &block.body().transactions {
                tx_hash_numbers.push((*transaction.tx_hash(), tx_hash_numbers.len() as u64));
            }
        }
        let tx_hash_numbers_len = tx_hash_numbers.len();
        db.insert_tx_hash_numbers(tx_hash_numbers).expect("insert tx hash numbers");

        assert_eq!(
            db.count_entries::<tables::Transactions>().unwrap(),
            blocks.iter().map(|block| block.transaction_count()).sum::<usize>()
        );
        assert_eq!(
            db.count_entries::<tables::Transactions>().unwrap(),
            db.table::<tables::TransactionHashNumbers>().unwrap().len()
        );

        let test_prune = |to_block: BlockNumber, expected_result: (PruneProgress, usize)| {
            let prune_mode = PruneMode::Before(to_block);
            let segment = TransactionLookup::new(prune_mode);
            let mut limiter = PruneLimiter::default().set_deleted_entries_limit(10);
            let input = PruneInput {
                previous_checkpoint: db
                    .factory
                    .provider()
                    .unwrap()
                    .get_prune_checkpoint(PruneSegment::TransactionLookup)
                    .unwrap(),
                to_block,
                limiter: limiter.clone(),
            };

            let next_tx_number_to_prune = db
                .factory
                .provider()
                .unwrap()
                .get_prune_checkpoint(PruneSegment::TransactionLookup)
                .unwrap()
                .and_then(|checkpoint| checkpoint.tx_number)
                .map(|tx_number| tx_number + 1)
                .unwrap_or_default();

            let last_pruned_tx_number = blocks
                .iter()
                .take(to_block as usize)
                .map(|block| block.transaction_count())
                .sum::<usize>()
                .min(
                    next_tx_number_to_prune as usize +
                        input.limiter.deleted_entries_limit().unwrap(),
                )
                .sub(1);

            let last_pruned_block_number = blocks
                .iter()
                .fold_while((0, 0), |(_, mut tx_count), block| {
                    tx_count += block.transaction_count();

                    if tx_count > last_pruned_tx_number {
                        Done((block.number, tx_count))
                    } else {
                        Continue((block.number, tx_count))
                    }
                })
                .into_inner()
                .0;

            let provider = db.factory.database_provider_rw().unwrap();
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

            let last_pruned_block_number = last_pruned_block_number
                .checked_sub(if result.progress.is_finished() { 0 } else { 1 });

            assert_eq!(
                db.table::<tables::TransactionHashNumbers>().unwrap().len(),
                tx_hash_numbers_len - (last_pruned_tx_number + 1)
            );
            assert_eq!(
                db.factory
                    .provider()
                    .unwrap()
                    .get_prune_checkpoint(PruneSegment::TransactionLookup)
                    .unwrap(),
                Some(PruneCheckpoint {
                    block_number: last_pruned_block_number,
                    tx_number: Some(last_pruned_tx_number as TxNumber),
                    prune_mode
                })
            );
        };

        test_prune(
            6,
            (PruneProgress::HasMoreData(PruneInterruptReason::DeletedEntriesLimitReached), 10),
        );
        test_prune(6, (PruneProgress::Finished, 2));
        test_prune(10, (PruneProgress::Finished, 8));
    }

    #[cfg(all(unix, feature = "rocksdb"))]
    #[test]
    fn prune_rocksdb() {
        use reth_db_api::models::StorageSettings;
        use reth_provider::RocksDBProviderFactory;
        use reth_storage_api::StorageSettingsCache;

        let db = TestStageDB::default();
        let mut rng = generators::rng();

        let blocks = random_block_range(
            &mut rng,
            1..=10,
            BlockRangeParams { parent: Some(B256::ZERO), tx_count: 2..3, ..Default::default() },
        );
        db.insert_blocks(blocks.iter(), StorageKind::Static).expect("insert blocks");

        // Collect transaction hashes and their tx numbers
        let mut tx_hash_numbers = Vec::new();
        for block in &blocks {
            tx_hash_numbers.reserve_exact(block.transaction_count());
            for transaction in &block.body().transactions {
                tx_hash_numbers.push((*transaction.tx_hash(), tx_hash_numbers.len() as u64));
            }
        }
        let tx_hash_numbers_len = tx_hash_numbers.len();

        // Insert into RocksDB instead of MDBX
        {
            let rocksdb = db.factory.rocksdb_provider();
            let mut batch = rocksdb.batch();
            for (hash, tx_num) in &tx_hash_numbers {
                batch.put::<tables::TransactionHashNumbers>(*hash, tx_num).unwrap();
            }
            batch.commit().expect("commit rocksdb batch");
        }

        // Verify RocksDB has all entries
        {
            let rocksdb = db.factory.rocksdb_provider();
            for (hash, expected_tx_num) in &tx_hash_numbers {
                let actual = rocksdb.get::<tables::TransactionHashNumbers>(*hash).unwrap();
                assert_eq!(actual, Some(*expected_tx_num));
            }
        }

        let to_block: BlockNumber = 6;
        let prune_mode = PruneMode::Before(to_block);
        let input =
            PruneInput { previous_checkpoint: None, to_block, limiter: PruneLimiter::default() };
        let segment = TransactionLookup::new(prune_mode);

        // Enable RocksDB storage for transaction hash numbers
        db.factory.set_storage_settings_cache(StorageSettings::v2());

        let provider = db.factory.database_provider_rw().unwrap();
        let result = segment.prune(&provider, input).unwrap();
        provider.commit().expect("commit");

        assert_matches!(
            result,
            SegmentOutput { progress: PruneProgress::Finished, pruned, checkpoint: Some(_) }
                if pruned > 0
        );

        // Calculate expected: blocks 1-6 should have their tx hashes pruned
        let txs_up_to_block_6: usize = blocks.iter().take(6).map(|b| b.transaction_count()).sum();

        // Verify RocksDB entries: first `txs_up_to_block_6` should be gone
        {
            let rocksdb = db.factory.rocksdb_provider();
            for (i, (hash, _)) in tx_hash_numbers.iter().enumerate() {
                let entry = rocksdb.get::<tables::TransactionHashNumbers>(*hash).unwrap();
                if i < txs_up_to_block_6 {
                    assert!(entry.is_none(), "Entry {} (hash {:?}) should be pruned", i, hash);
                } else {
                    assert!(entry.is_some(), "Entry {} (hash {:?}) should still exist", i, hash);
                }
            }
        }

        // Verify remaining count
        {
            let rocksdb = db.factory.rocksdb_provider();
            let remaining: Vec<_> =
                rocksdb.iter::<tables::TransactionHashNumbers>().unwrap().collect();
            assert_eq!(
                remaining.len(),
                tx_hash_numbers_len - txs_up_to_block_6,
                "Remaining RocksDB entries should match expected"
            );
        }
    }

    /// Tests that when `RocksDB` prune deletes nothing (limit exhausted), checkpoint doesn't
    /// advance.
    ///
    /// This test simulates a scenario where:
    /// 1. Some transactions have already been pruned (checkpoint at tx 5)
    /// 2. The deleted entries limit is exhausted before any new deletions
    /// 3. The checkpoint should NOT advance to the next start position
    #[cfg(all(unix, feature = "rocksdb"))]
    #[test]
    fn prune_rocksdb_zero_deleted_checkpoint() {
        use reth_db_api::models::StorageSettings;
        use reth_provider::RocksDBProviderFactory;
        use reth_storage_api::StorageSettingsCache;

        let db = TestStageDB::default();
        let mut rng = generators::rng();

        let blocks = random_block_range(
            &mut rng,
            1..=10,
            BlockRangeParams { parent: Some(B256::ZERO), tx_count: 2..3, ..Default::default() },
        );
        db.insert_blocks(blocks.iter(), StorageKind::Static).expect("insert blocks");

        // Collect transaction hashes and their tx numbers
        let mut tx_hash_numbers = Vec::new();
        for block in &blocks {
            tx_hash_numbers.reserve_exact(block.transaction_count());
            for transaction in &block.body().transactions {
                tx_hash_numbers.push((*transaction.tx_hash(), tx_hash_numbers.len() as u64));
            }
        }

        // Insert into RocksDB
        {
            let rocksdb = db.factory.rocksdb_provider();
            let mut batch = rocksdb.batch();
            for (hash, tx_num) in &tx_hash_numbers {
                batch.put::<tables::TransactionHashNumbers>(*hash, tx_num).unwrap();
            }
            batch.commit().expect("commit rocksdb batch");
        }

        // Enable RocksDB storage for transaction hash numbers
        db.factory.set_storage_settings_cache(StorageSettings::v2());

        let to_block: BlockNumber = 6;
        let prune_mode = PruneMode::Before(to_block);

        // Simulate that we've already pruned up to tx 5, so start will be tx 6
        let previous_checkpoint =
            Some(PruneCheckpoint { block_number: Some(2), tx_number: Some(5), prune_mode });

        // Create a limiter with limit of 1, but exhaust it before pruning
        // This means deleted_entries_limit_left() = Some(0)
        let mut limiter = PruneLimiter::default().set_deleted_entries_limit(1);
        limiter.increment_deleted_entries_count(); // Exhaust the limit

        let input = PruneInput { previous_checkpoint, to_block, limiter };
        let segment = TransactionLookup::new(prune_mode);

        let provider = db.factory.database_provider_rw().unwrap();
        let result = segment.prune(&provider, input).unwrap();
        provider.commit().expect("commit");

        // With an exhausted limit, nothing should be deleted
        assert_eq!(result.pruned, 0, "Nothing should be pruned with exhausted limit");

        // The checkpoint tx_number should NOT advance to 6 (start)
        // With the bug: checkpoint.tx_number = start = 6 (WRONG - claims tx 6 was pruned)
        // With the fix: checkpoint.tx_number = tx_range_end = 5 (correct - no advancement)
        if let Some(checkpoint) = &result.checkpoint {
            assert_eq!(
                checkpoint.tx_number,
                Some(5),
                "Checkpoint should stay at 5 (previous), not advance to 6 (start)"
            );
        }

        // All RocksDB entries should still exist (nothing was actually deleted)
        {
            let rocksdb = db.factory.rocksdb_provider();
            let remaining: Vec<_> =
                rocksdb.iter::<tables::TransactionHashNumbers>().unwrap().collect();
            assert_eq!(
                remaining.len(),
                tx_hash_numbers.len(),
                "All RocksDB entries should still exist"
            );
        }
    }
}
