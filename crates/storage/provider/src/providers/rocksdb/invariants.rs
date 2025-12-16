//! Invariant checking for `RocksDB` tables.
//!
//! This module provides consistency checks for tables stored in `RocksDB`, similar to the
//! consistency checks for static files. The goal is to detect and potentially heal
//! inconsistencies between `RocksDB` data and MDBX checkpoints.

use super::RocksDBProvider;
use alloy_eips::eip2718::Encodable2718;
use alloy_primitives::BlockNumber;
use rayon::prelude::*;
use reth_db::cursor::DbCursorRO;
use reth_db_api::{tables, transaction::DbTx};
use reth_stages_types::StageId;
use reth_storage_api::{
    DBProvider, StageCheckpointReader, StorageSettingsCache, TransactionsProvider,
};
use reth_storage_errors::provider::ProviderResult;

impl RocksDBProvider {
    /// Checks consistency of `RocksDB` tables against MDBX stage checkpoints.
    ///
    /// Returns an unwind target block number if the pipeline needs to unwind to rebuild
    /// `RocksDB` data. Returns `None` if all invariants pass or if inconsistencies were healed.
    ///
    /// # Invariants checked
    ///
    /// For `TransactionHashNumbers`:
    /// - The maximum `TxNumber` value should not exceed what the `TransactionLookup` stage
    ///   checkpoint indicates has been processed.
    /// - If `RocksDB` is ahead, excess entries are pruned (healed).
    /// - If `RocksDB` is behind, an unwind is required.
    ///
    /// For `StoragesHistory`:
    /// - The maximum block number in shards should not exceed the `IndexStorageHistory` stage
    ///   checkpoint.
    /// - Similar healing/unwind logic applies.
    ///
    /// # Requirements
    ///
    /// For pruning `TransactionHashNumbers`, the provider must be able to supply transaction
    /// data (typically from static files) so that transaction hashes can be computed. This
    /// implies that static files should be ahead of or in sync with `RocksDB`.
    pub fn check_consistency<Provider>(
        &self,
        provider: &Provider,
    ) -> ProviderResult<Option<BlockNumber>>
    where
        Provider: DBProvider
            + StageCheckpointReader
            + StorageSettingsCache
            + TransactionsProvider<Transaction: Encodable2718>,
    {
        let mut unwind_target: Option<BlockNumber> = None;

        // Check TransactionHashNumbers if stored in RocksDB
        if provider.cached_storage_settings().transaction_hash_numbers_in_rocksdb &&
            let Some(target) = self.check_transaction_hash_numbers(provider)?
        {
            unwind_target = Some(unwind_target.map_or(target, |t| t.min(target)));
        }

        // Check StoragesHistory if stored in RocksDB
        if provider.cached_storage_settings().storages_history_in_rocksdb &&
            let Some(target) = self.check_storages_history(provider)?
        {
            unwind_target = Some(unwind_target.map_or(target, |t| t.min(target)));
        }

        Ok(unwind_target)
    }

    /// Checks invariants for the `TransactionHashNumbers` table.
    ///
    /// Returns a block number to unwind to if `RocksDB` is behind the checkpoint.
    /// If `RocksDB` is ahead of the checkpoint, excess entries are pruned (healed).
    fn check_transaction_hash_numbers<Provider>(
        &self,
        provider: &Provider,
    ) -> ProviderResult<Option<BlockNumber>>
    where
        Provider:
            DBProvider + StageCheckpointReader + TransactionsProvider<Transaction: Encodable2718>,
    {
        // Get the TransactionLookup stage checkpoint
        let checkpoint = provider
            .get_stage_checkpoint(StageId::TransactionLookup)?
            .map(|cp| cp.block_number)
            .unwrap_or(0);

        // Find the max tx_number in RocksDB by iterating all entries.
        // Note: We can't use `last()` because that returns the entry with the largest
        // hash key, not the entry with the largest tx_number value.
        let max_tx = self
            .iter::<tables::TransactionHashNumbers>()?
            .map(|r| r.map(|(_, tx_num)| tx_num))
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .max();

        match max_tx {
            Some(tx_number) => {
                // If checkpoint is 0 but we have data, that's inconsistent
                if checkpoint == 0 && tx_number > 0 {
                    // RocksDB has data but stage hasn't run - need to clear all.
                    // Prune all transactions in range [0, tx_number].
                    self.prune_transaction_hash_numbers_in_range(provider, 0..=tx_number)?;
                    return Ok(None);
                }

                // Map tx_number to block_number using TransactionBlocks table
                // TransactionBlocks key is the highest tx ID in each block
                let mut cursor = provider.tx_ref().cursor_read::<tables::TransactionBlocks>()?;

                // Seek to the first entry where key >= tx_number
                if let Some((_, rocks_block)) = cursor.seek(tx_number)? {
                    // rocks_block is the block containing this transaction
                    if checkpoint > rocks_block {
                        // RocksDB is behind checkpoint - this indicates something went wrong
                        // since MDBX checkpoint is committed last when appending.
                        tracing::warn!(
                            target: "reth::providers::rocksdb",
                            rocks_max_tx = tx_number,
                            rocks_block,
                            checkpoint,
                            "TransactionHashNumbers behind checkpoint, unwind needed"
                        );
                        return Ok(Some(rocks_block));
                    } else if checkpoint < rocks_block {
                        // RocksDB is ahead of checkpoint - this is the common case during
                        // recovery from a crash during unwinding. MDBX checkpoint was
                        // committed first, so we need to prune excess RocksDB data.
                        //
                        // Find the max tx_number for the checkpoint block by iterating
                        // TransactionBlocks to find the entry where block_number == checkpoint
                        let mut max_tx_for_checkpoint = 0u64;
                        let mut cursor_walk =
                            provider.tx_ref().cursor_read::<tables::TransactionBlocks>()?;
                        let mut walker = cursor_walk.walk(Some(0))?;
                        while let Some((tx_num, block_num)) = walker.next().transpose()? {
                            if block_num == checkpoint {
                                max_tx_for_checkpoint = tx_num;
                                break;
                            } else if block_num > checkpoint {
                                // We've passed the checkpoint block, use the previous tx as max
                                break;
                            }
                            max_tx_for_checkpoint = tx_num;
                        }

                        if max_tx_for_checkpoint < tx_number {
                            tracing::info!(
                                target: "reth::providers::rocksdb",
                                rocks_max_tx = tx_number,
                                rocks_block,
                                checkpoint,
                                max_tx_for_checkpoint,
                                "TransactionHashNumbers ahead of checkpoint, pruning excess data"
                            );
                            // Prune transactions in range (max_tx_for_checkpoint, tx_number]
                            self.prune_transaction_hash_numbers_in_range(
                                provider,
                                (max_tx_for_checkpoint + 1)..=tx_number,
                            )?;
                        }
                    }
                }

                Ok(None)
            }
            None => {
                // Empty RocksDB table
                // If checkpoint says we should have data, that's an inconsistency
                if checkpoint > 0 {
                    // Need to rebuild from scratch
                    return Ok(Some(0));
                }
                Ok(None)
            }
        }
    }

    /// Prunes `TransactionHashNumbers` entries for transactions in the given range.
    ///
    /// This fetches transactions from the provider, computes their hashes in parallel,
    /// and deletes the corresponding entries from `RocksDB` by key. This approach is more
    /// scalable than iterating all rows because it only processes the transactions that
    /// need to be pruned.
    ///
    /// # Requirements
    ///
    /// The provider must be able to supply transaction data (typically from static files)
    /// so that transaction hashes can be computed. This implies that static files should
    /// be ahead of or in sync with `RocksDB`.
    pub fn prune_transaction_hash_numbers_in_range<Provider>(
        &self,
        provider: &Provider,
        tx_range: std::ops::RangeInclusive<u64>,
    ) -> ProviderResult<()>
    where
        Provider: TransactionsProvider<Transaction: Encodable2718>,
    {
        if tx_range.is_empty() {
            return Ok(());
        }

        // Fetch transactions in the range and compute their hashes in parallel
        let hashes: Vec<_> = provider
            .transactions_by_tx_range(tx_range.clone())?
            .into_par_iter()
            .map(|tx| tx.trie_hash())
            .collect();

        let deleted_count = hashes.len();
        if deleted_count > 0 {
            tracing::info!(
                target: "reth::providers::rocksdb",
                deleted_count,
                tx_range_start = *tx_range.start(),
                tx_range_end = *tx_range.end(),
                "Pruning TransactionHashNumbers entries by tx range"
            );

            self.write_batch(|batch| {
                for hash in hashes {
                    batch.delete::<tables::TransactionHashNumbers>(hash)?;
                }
                Ok(())
            })?;
        }

        Ok(())
    }

    /// Checks invariants for the `StoragesHistory` table.
    ///
    /// Returns a block number to unwind to if `RocksDB` is behind the checkpoint.
    /// If `RocksDB` is ahead of the checkpoint, excess entries are pruned (healed).
    fn check_storages_history<Provider>(
        &self,
        provider: &Provider,
    ) -> ProviderResult<Option<BlockNumber>>
    where
        Provider: DBProvider + StageCheckpointReader,
    {
        // Get the IndexStorageHistory stage checkpoint
        let checkpoint = provider
            .get_stage_checkpoint(StageId::IndexStorageHistory)?
            .map(|cp| cp.block_number)
            .unwrap_or(0);

        // Check if RocksDB has any data
        let rocks_first = self.first::<tables::StoragesHistory>()?;

        match rocks_first {
            Some(_) => {
                // If checkpoint is 0 but we have data, clear everything
                if checkpoint == 0 {
                    tracing::info!(
                        target: "reth::providers::rocksdb",
                        "StoragesHistory has data but checkpoint is 0, clearing all"
                    );
                    self.prune_storages_history_above(0)?;
                    return Ok(None);
                }

                // Find the max highest_block_number (excluding u64::MAX sentinel) across all
                // entries
                let mut max_highest_block = 0u64;
                for result in self.iter::<tables::StoragesHistory>()? {
                    let (key, _) = result?;
                    let highest = key.sharded_key.highest_block_number;
                    if highest != u64::MAX && highest > max_highest_block {
                        max_highest_block = highest;
                    }
                }

                // If any entry has highest_block > checkpoint, prune excess
                if max_highest_block > checkpoint {
                    tracing::info!(
                        target: "reth::providers::rocksdb",
                        rocks_highest = max_highest_block,
                        checkpoint,
                        "StoragesHistory ahead of checkpoint, pruning excess data"
                    );
                    self.prune_storages_history_above(checkpoint)?;
                }

                Ok(None)
            }
            None => {
                // Empty RocksDB table
                if checkpoint > 0 {
                    // Stage says we should have data but we don't
                    return Ok(Some(0));
                }
                Ok(None)
            }
        }
    }

    /// Prunes `StoragesHistory` entries where `highest_block_number` > `max_block`.
    ///
    /// For `StoragesHistory`, the key contains `highest_block_number`, so we can iterate
    /// and delete entries where `key.sharded_key.highest_block_number > max_block`.
    ///
    /// TODO(<https://github.com/paradigmxyz/reth/issues/20417>): use changeset-based pruning.
    fn prune_storages_history_above(&self, max_block: BlockNumber) -> ProviderResult<()> {
        use reth_db_api::models::storage_sharded_key::StorageShardedKey;

        let mut to_delete: Vec<StorageShardedKey> = Vec::new();

        // Iterate all entries and collect those to delete
        for result in self.iter::<tables::StoragesHistory>()? {
            let (key, _value) = result?;

            let highest_block = key.sharded_key.highest_block_number;

            // Delete entries where highest_block > max_block (excluding u64::MAX sentinel)
            // Also delete if max_block is 0 (clearing all)
            if max_block == 0 || (highest_block != u64::MAX && highest_block > max_block) {
                to_delete.push(key);
            }
        }

        let deleted_count = to_delete.len();
        if deleted_count > 0 {
            tracing::info!(
                target: "reth::providers::rocksdb",
                deleted_count,
                max_block,
                "Pruning StoragesHistory entries"
            );

            self.write_batch(|batch| {
                for key in to_delete {
                    batch.delete::<tables::StoragesHistory>(key)?;
                }
                Ok(())
            })?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        providers::rocksdb::RocksDBBuilder, test_utils::create_test_provider_factory, BlockWriter,
        DatabaseProviderFactory, StageCheckpointWriter, TransactionsProvider,
    };
    use alloy_primitives::{Address, B256};
    use reth_db::cursor::DbCursorRW;
    use reth_db_api::{
        models::{storage_sharded_key::StorageShardedKey, StorageSettings},
        tables::{self, BlockNumberList},
        transaction::DbTxMut,
    };
    use reth_stages_types::StageCheckpoint;
    use reth_testing_utils::generators::{self, BlockRangeParams};
    use tempfile::TempDir;

    #[test]
    fn test_first_last_empty_rocksdb() {
        let temp_dir = TempDir::new().unwrap();
        let provider = RocksDBBuilder::new(temp_dir.path())
            .with_table::<tables::TransactionHashNumbers>()
            .with_table::<tables::StoragesHistory>()
            .build()
            .unwrap();

        // Empty RocksDB, no checkpoints - should be consistent
        let first = provider.first::<tables::TransactionHashNumbers>().unwrap();
        let last = provider.last::<tables::TransactionHashNumbers>().unwrap();

        assert!(first.is_none());
        assert!(last.is_none());
    }

    #[test]
    fn test_first_last_with_data() {
        let temp_dir = TempDir::new().unwrap();
        let provider = RocksDBBuilder::new(temp_dir.path())
            .with_table::<tables::TransactionHashNumbers>()
            .build()
            .unwrap();

        // Insert some data
        let tx_hash = B256::from([1u8; 32]);
        provider.put::<tables::TransactionHashNumbers>(tx_hash, &100).unwrap();

        // RocksDB has data
        let last = provider.last::<tables::TransactionHashNumbers>().unwrap();
        assert!(last.is_some());
        assert_eq!(last.unwrap().1, 100);
    }

    #[test]
    fn test_check_consistency_empty_rocksdb_no_checkpoint_is_ok() {
        let temp_dir = TempDir::new().unwrap();
        let rocksdb = RocksDBBuilder::new(temp_dir.path())
            .with_table::<tables::TransactionHashNumbers>()
            .with_table::<tables::StoragesHistory>()
            .build()
            .unwrap();

        // Create a test provider factory for MDBX
        let factory = create_test_provider_factory();
        factory.set_storage_settings_cache(
            StorageSettings::legacy()
                .with_transaction_hash_numbers_in_rocksdb(true)
                .with_storages_history_in_rocksdb(true),
        );

        let provider = factory.database_provider_ro().unwrap();

        // Empty RocksDB and no checkpoints - should be consistent (None = no unwind needed)
        let result = rocksdb.check_consistency(&provider).unwrap();
        assert_eq!(result, None);
    }

    #[test]
    fn test_check_consistency_empty_rocksdb_with_checkpoint_needs_unwind() {
        let temp_dir = TempDir::new().unwrap();
        let rocksdb = RocksDBBuilder::new(temp_dir.path())
            .with_table::<tables::TransactionHashNumbers>()
            .build()
            .unwrap();

        // Create a test provider factory for MDBX
        let factory = create_test_provider_factory();
        factory.set_storage_settings_cache(
            StorageSettings::legacy().with_transaction_hash_numbers_in_rocksdb(true),
        );

        // Set a checkpoint indicating we should have processed up to block 100
        {
            let provider = factory.database_provider_rw().unwrap();
            provider
                .save_stage_checkpoint(StageId::TransactionLookup, StageCheckpoint::new(100))
                .unwrap();
            provider.commit().unwrap();
        }

        let provider = factory.database_provider_ro().unwrap();

        // RocksDB is empty but checkpoint says block 100 was processed
        // This means RocksDB is missing data and we need to unwind to rebuild
        let result = rocksdb.check_consistency(&provider).unwrap();
        assert_eq!(result, Some(0), "Should require unwind to block 0 to rebuild RocksDB");
    }

    #[test]
    fn test_check_consistency_rocksdb_has_data_no_checkpoint_prunes_data() {
        let temp_dir = TempDir::new().unwrap();
        let rocksdb = RocksDBBuilder::new(temp_dir.path())
            .with_table::<tables::TransactionHashNumbers>()
            .build()
            .unwrap();

        // Create a test provider factory for MDBX with NO checkpoint
        let factory = create_test_provider_factory();
        factory.set_storage_settings_cache(
            StorageSettings::legacy().with_transaction_hash_numbers_in_rocksdb(true),
        );

        // Generate blocks with real transactions and insert them
        let mut rng = generators::rng();
        let blocks = generators::random_block_range(
            &mut rng,
            0..=2,
            BlockRangeParams { parent: Some(B256::ZERO), tx_count: 2..3, ..Default::default() },
        );

        let mut tx_count = 0u64;
        {
            let provider = factory.database_provider_rw().unwrap();
            for block in &blocks {
                provider.insert_block(block.clone().try_recover().expect("recover block")).unwrap();
                for tx in &block.body().transactions {
                    let hash = tx.trie_hash();
                    rocksdb.put::<tables::TransactionHashNumbers>(hash, &tx_count).unwrap();
                    tx_count += 1;
                }
            }
            // No checkpoint set (checkpoint = 0)
            provider.commit().unwrap();
        }

        // Verify data exists
        assert!(rocksdb.last::<tables::TransactionHashNumbers>().unwrap().is_some());

        let provider = factory.database_provider_ro().unwrap();

        // RocksDB has data but checkpoint is 0
        // This means RocksDB has stale data that should be pruned (healed)
        let result = rocksdb.check_consistency(&provider).unwrap();
        assert_eq!(result, None, "Should heal by pruning, no unwind needed");

        // Verify data was pruned
        assert!(
            rocksdb.last::<tables::TransactionHashNumbers>().unwrap().is_none(),
            "RocksDB should be empty after pruning"
        );
    }

    #[test]
    fn test_check_consistency_storages_history_empty_with_checkpoint_needs_unwind() {
        let temp_dir = TempDir::new().unwrap();
        let rocksdb = RocksDBBuilder::new(temp_dir.path())
            .with_table::<tables::StoragesHistory>()
            .build()
            .unwrap();

        // Create a test provider factory for MDBX
        let factory = create_test_provider_factory();
        factory.set_storage_settings_cache(
            StorageSettings::legacy().with_storages_history_in_rocksdb(true),
        );

        // Set a checkpoint indicating we should have processed up to block 100
        {
            let provider = factory.database_provider_rw().unwrap();
            provider
                .save_stage_checkpoint(StageId::IndexStorageHistory, StageCheckpoint::new(100))
                .unwrap();
            provider.commit().unwrap();
        }

        let provider = factory.database_provider_ro().unwrap();

        // RocksDB is empty but checkpoint says block 100 was processed
        let result = rocksdb.check_consistency(&provider).unwrap();
        assert_eq!(result, Some(0), "Should require unwind to block 0 to rebuild StoragesHistory");
    }

    #[test]
    fn test_check_consistency_storages_history_has_data_no_checkpoint_prunes_data() {
        let temp_dir = TempDir::new().unwrap();
        let rocksdb = RocksDBBuilder::new(temp_dir.path())
            .with_table::<tables::StoragesHistory>()
            .build()
            .unwrap();

        // Insert data into RocksDB
        let key = StorageShardedKey::new(Address::ZERO, B256::ZERO, 50);
        let block_list = BlockNumberList::new_pre_sorted([10, 20, 30, 50]);
        rocksdb.put::<tables::StoragesHistory>(key, &block_list).unwrap();

        // Verify data exists
        assert!(rocksdb.last::<tables::StoragesHistory>().unwrap().is_some());

        // Create a test provider factory for MDBX with NO checkpoint
        let factory = create_test_provider_factory();
        factory.set_storage_settings_cache(
            StorageSettings::legacy().with_storages_history_in_rocksdb(true),
        );

        let provider = factory.database_provider_ro().unwrap();

        // RocksDB has data but checkpoint is 0
        // This means RocksDB has stale data that should be pruned (healed)
        let result = rocksdb.check_consistency(&provider).unwrap();
        assert_eq!(result, None, "Should heal by pruning, no unwind needed");

        // Verify data was pruned
        assert!(
            rocksdb.last::<tables::StoragesHistory>().unwrap().is_none(),
            "RocksDB should be empty after pruning"
        );
    }

    #[test]
    fn test_check_consistency_rocksdb_behind_checkpoint_needs_unwind() {
        let temp_dir = TempDir::new().unwrap();
        let rocksdb = RocksDBBuilder::new(temp_dir.path())
            .with_table::<tables::TransactionHashNumbers>()
            .build()
            .unwrap();

        // Insert data into RocksDB - tx hash mapping to tx_number 50
        let tx_hash = B256::from([1u8; 32]);
        rocksdb.put::<tables::TransactionHashNumbers>(tx_hash, &50).unwrap();

        // Create a test provider factory for MDBX
        let factory = create_test_provider_factory();
        factory.set_storage_settings_cache(
            StorageSettings::legacy().with_transaction_hash_numbers_in_rocksdb(true),
        );

        // Set up MDBX with:
        // - TransactionBlocks: tx 50 is in block 10, tx 100 is in block 20
        // - Checkpoint at block 20 (meaning we should have tx hashes up to tx 100)
        {
            let provider = factory.database_provider_rw().unwrap();

            // Insert TransactionBlocks entries
            let mut cursor = provider.tx_ref().cursor_write::<tables::TransactionBlocks>().unwrap();
            cursor.append(50, &10).unwrap(); // tx 50 is the last tx in block 10
            cursor.append(100, &20).unwrap(); // tx 100 is the last tx in block 20

            // Set checkpoint to block 20
            provider
                .save_stage_checkpoint(StageId::TransactionLookup, StageCheckpoint::new(20))
                .unwrap();
            provider.commit().unwrap();
        }

        let provider = factory.database_provider_ro().unwrap();

        // RocksDB has tx hashes only up to tx 50 (block 10)
        // But checkpoint says we processed up to block 20 (tx 100)
        // This means RocksDB is behind and needs to unwind to block 10 to rebuild
        let result = rocksdb.check_consistency(&provider).unwrap();
        assert_eq!(
            result,
            Some(10),
            "Should require unwind to block 10 where RocksDB's max tx belongs"
        );
    }

    #[test]
    fn test_check_consistency_rocksdb_ahead_of_checkpoint_prunes_excess() {
        let temp_dir = TempDir::new().unwrap();
        let rocksdb = RocksDBBuilder::new(temp_dir.path())
            .with_table::<tables::TransactionHashNumbers>()
            .build()
            .unwrap();

        // Create a test provider factory for MDBX
        let factory = create_test_provider_factory();
        factory.set_storage_settings_cache(
            StorageSettings::legacy().with_transaction_hash_numbers_in_rocksdb(true),
        );

        // Generate blocks with real transactions:
        // Blocks 0-5, each with 2 transactions = 12 total transactions (0-11)
        let mut rng = generators::rng();
        let blocks = generators::random_block_range(
            &mut rng,
            0..=5,
            BlockRangeParams { parent: Some(B256::ZERO), tx_count: 2..3, ..Default::default() },
        );

        // Track which hashes belong to which blocks
        let mut tx_hashes = Vec::new();
        let mut tx_count = 0u64;
        {
            let provider = factory.database_provider_rw().unwrap();
            for block in &blocks {
                provider.insert_block(block.clone().try_recover().expect("recover block")).unwrap();
                for tx in &block.body().transactions {
                    let hash = tx.trie_hash();
                    tx_hashes.push(hash);
                    rocksdb.put::<tables::TransactionHashNumbers>(hash, &tx_count).unwrap();
                    tx_count += 1;
                }
            }

            // Set checkpoint to block 2 (meaning we should only have tx hashes for blocks 0-2)
            // Blocks 0, 1, 2 have 6 transactions total
            provider
                .save_stage_checkpoint(StageId::TransactionLookup, StageCheckpoint::new(2))
                .unwrap();
            provider.commit().unwrap();
        }

        let provider = factory.database_provider_ro().unwrap();

        // RocksDB has tx hashes for all blocks (0-5)
        // But checkpoint says we only processed up to block 2
        // This means RocksDB is ahead and should prune entries for blocks 3-5
        let result = rocksdb.check_consistency(&provider).unwrap();
        assert_eq!(result, None, "Should heal by pruning, no unwind needed");

        // Verify: hashes for blocks 0-2 (tx 0-5) should remain, blocks 3-5 (tx 6-11) should be
        // pruned First 6 hashes should remain
        for (i, hash) in tx_hashes.iter().take(6).enumerate() {
            assert!(
                rocksdb.get::<tables::TransactionHashNumbers>(*hash).unwrap().is_some(),
                "tx {} should remain",
                i
            );
        }
        // Last 6 hashes should be pruned
        for (i, hash) in tx_hashes.iter().skip(6).enumerate() {
            assert!(
                rocksdb.get::<tables::TransactionHashNumbers>(*hash).unwrap().is_none(),
                "tx {} should be pruned",
                i + 6
            );
        }
    }

    #[test]
    fn test_check_consistency_storages_history_ahead_of_checkpoint_prunes_excess() {
        let temp_dir = TempDir::new().unwrap();
        let rocksdb = RocksDBBuilder::new(temp_dir.path())
            .with_table::<tables::StoragesHistory>()
            .build()
            .unwrap();

        // Insert data into RocksDB with different highest_block_numbers
        let key_block_50 = StorageShardedKey::new(Address::ZERO, B256::ZERO, 50);
        let key_block_100 = StorageShardedKey::new(Address::ZERO, B256::from([1u8; 32]), 100);
        let key_block_150 = StorageShardedKey::new(Address::ZERO, B256::from([2u8; 32]), 150);
        let key_block_max = StorageShardedKey::new(Address::ZERO, B256::from([3u8; 32]), u64::MAX);

        let block_list = BlockNumberList::new_pre_sorted([10, 20, 30]);
        rocksdb.put::<tables::StoragesHistory>(key_block_50.clone(), &block_list).unwrap();
        rocksdb.put::<tables::StoragesHistory>(key_block_100.clone(), &block_list).unwrap();
        rocksdb.put::<tables::StoragesHistory>(key_block_150.clone(), &block_list).unwrap();
        rocksdb.put::<tables::StoragesHistory>(key_block_max.clone(), &block_list).unwrap();

        // Create a test provider factory for MDBX
        let factory = create_test_provider_factory();
        factory.set_storage_settings_cache(
            StorageSettings::legacy().with_storages_history_in_rocksdb(true),
        );

        // Set checkpoint to block 100
        {
            let provider = factory.database_provider_rw().unwrap();
            provider
                .save_stage_checkpoint(StageId::IndexStorageHistory, StageCheckpoint::new(100))
                .unwrap();
            provider.commit().unwrap();
        }

        let provider = factory.database_provider_ro().unwrap();

        // RocksDB has entries with highest_block = 150 which exceeds checkpoint (100)
        // Should prune entries where highest_block > 100 (but not u64::MAX sentinel)
        let result = rocksdb.check_consistency(&provider).unwrap();
        assert_eq!(result, None, "Should heal by pruning, no unwind needed");

        // Verify key_block_150 was pruned, but others remain
        assert!(
            rocksdb.get::<tables::StoragesHistory>(key_block_50).unwrap().is_some(),
            "Entry with highest_block=50 should remain"
        );
        assert!(
            rocksdb.get::<tables::StoragesHistory>(key_block_100).unwrap().is_some(),
            "Entry with highest_block=100 should remain"
        );
        assert!(
            rocksdb.get::<tables::StoragesHistory>(key_block_150).unwrap().is_none(),
            "Entry with highest_block=150 should be pruned"
        );
        assert!(
            rocksdb.get::<tables::StoragesHistory>(key_block_max).unwrap().is_some(),
            "Entry with highest_block=u64::MAX (sentinel) should remain"
        );
    }

    /// Test that pruning works by fetching transactions and computing their hashes,
    /// rather than iterating all rows. This test uses random blocks with unique
    /// transactions so we can verify the correct entries are pruned.
    #[test]
    fn test_prune_transaction_hash_numbers_by_range() {
        let temp_dir = TempDir::new().unwrap();
        let rocksdb = RocksDBBuilder::new(temp_dir.path())
            .with_table::<tables::TransactionHashNumbers>()
            .build()
            .unwrap();

        // Create a test provider factory for MDBX
        let factory = create_test_provider_factory();
        factory.set_storage_settings_cache(
            StorageSettings::legacy().with_transaction_hash_numbers_in_rocksdb(true),
        );

        // Generate random blocks with unique transactions
        // Block 0 (genesis) has no transactions
        // Blocks 1-5 each have 2 transactions = 10 transactions total
        let mut rng = generators::rng();
        let blocks = generators::random_block_range(
            &mut rng,
            0..=5,
            BlockRangeParams { parent: Some(B256::ZERO), tx_count: 2..3, ..Default::default() },
        );

        // Insert blocks into the database
        let mut tx_count = 0u64;
        let mut tx_hashes = Vec::new();
        {
            let provider = factory.database_provider_rw().unwrap();

            for block in &blocks {
                provider.insert_block(block.clone().try_recover().expect("recover block")).unwrap();

                // Store transaction hash -> tx_number mappings in RocksDB
                for tx in &block.body().transactions {
                    let hash = tx.trie_hash();
                    tx_hashes.push(hash);
                    rocksdb.put::<tables::TransactionHashNumbers>(hash, &tx_count).unwrap();
                    tx_count += 1;
                }
            }

            // Set checkpoint to block 2 (meaning we should only have tx hashes for blocks 0-2)
            // Blocks 0, 1, 2 have 6 transactions (2 each), so tx 0-5 should remain
            provider
                .save_stage_checkpoint(StageId::TransactionLookup, StageCheckpoint::new(2))
                .unwrap();
            provider.commit().unwrap();
        }

        // At this point:
        // - RocksDB has tx hashes for blocks 0-5 (10 total: 2 per block)
        // - Checkpoint says we only processed up to block 2
        // - We need to prune tx hashes for blocks 3, 4, 5 (tx 6-9)

        // Verify RocksDB has the expected number of entries before pruning
        let rocksdb_count_before: usize =
            rocksdb.iter::<tables::TransactionHashNumbers>().unwrap().count();
        assert_eq!(
            rocksdb_count_before, tx_count as usize,
            "RocksDB should have all {} transaction hashes before pruning",
            tx_count
        );

        let provider = factory.database_provider_ro().unwrap();

        // Verify we can fetch transactions by tx range
        let all_txs = provider.transactions_by_tx_range(0..tx_count).unwrap();
        assert_eq!(all_txs.len(), tx_count as usize, "Should be able to fetch all transactions");

        // Verify the hashes match between what we stored and what we compute from fetched txs
        for (i, tx) in all_txs.iter().enumerate() {
            let computed_hash = tx.trie_hash();
            assert_eq!(
                computed_hash, tx_hashes[i],
                "Hash mismatch for tx {}: stored {:?} vs computed {:?}",
                i, tx_hashes[i], computed_hash
            );
        }

        // Blocks 0, 1, 2 have 2 tx each = 6 tx total (indices 0-5)
        // We want to keep tx 0-5, prune tx 6-9
        let max_tx_to_keep = 5u64;
        let tx_to_prune_start = max_tx_to_keep + 1;

        // Prune transactions 6-9 (blocks 3-5)
        rocksdb
            .prune_transaction_hash_numbers_in_range(&provider, tx_to_prune_start..=(tx_count - 1))
            .expect("prune should succeed");

        // Verify: transactions 0-5 should remain, 6-9 should be pruned
        let mut remaining_count = 0;
        for result in rocksdb.iter::<tables::TransactionHashNumbers>().unwrap() {
            let (_hash, tx_num) = result.unwrap();
            assert!(
                tx_num <= max_tx_to_keep,
                "Transaction {} should have been pruned (> {})",
                tx_num,
                max_tx_to_keep
            );
            remaining_count += 1;
        }
        assert_eq!(
            remaining_count,
            (max_tx_to_keep + 1) as usize,
            "Should have {} transactions (0-{})",
            max_tx_to_keep + 1,
            max_tx_to_keep
        );
    }
}
