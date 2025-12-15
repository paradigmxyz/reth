//! Invariant checking for `RocksDB` tables.
//!
//! This module provides consistency checks for tables stored in `RocksDB`, similar to the
//! consistency checks for static files. The goal is to detect and potentially heal
//! inconsistencies between `RocksDB` data and MDBX checkpoints.

use super::RocksDBProvider;
use alloy_primitives::BlockNumber;
use reth_db_api::tables;
use reth_stages_types::StageId;
use reth_storage_api::{DBProvider, StageCheckpointReader, StorageSettingsCache};
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
    pub fn check_consistency<Provider>(
        &self,
        provider: &Provider,
    ) -> ProviderResult<Option<BlockNumber>>
    where
        Provider: DBProvider + StageCheckpointReader + StorageSettingsCache,
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
        Provider: DBProvider + StageCheckpointReader,
    {
        use reth_db::cursor::DbCursorRO;
        use reth_db_api::transaction::DbTx;

        // Get the TransactionLookup stage checkpoint
        let checkpoint = provider
            .get_stage_checkpoint(StageId::TransactionLookup)?
            .map(|cp| cp.block_number)
            .unwrap_or(0);

        // Get the last entry from RocksDB TransactionHashNumbers
        let rocks_last = self.last::<tables::TransactionHashNumbers>()?;

        match rocks_last {
            Some((_hash, tx_number)) => {
                // If checkpoint is 0 but we have data, that's inconsistent
                if checkpoint == 0 && tx_number > 0 {
                    // RocksDB has data but stage hasn't run - need to clear all
                    self.prune_transaction_hash_numbers_above(0)?;
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

                        if max_tx_for_checkpoint > 0 {
                            tracing::info!(
                                target: "reth::providers::rocksdb",
                                rocks_max_tx = tx_number,
                                rocks_block,
                                checkpoint,
                                max_tx_for_checkpoint,
                                "TransactionHashNumbers ahead of checkpoint, pruning excess data"
                            );
                            self.prune_transaction_hash_numbers_above(max_tx_for_checkpoint)?;
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

    /// Prunes `TransactionHashNumbers` entries where `tx_number` > `max_tx_number`.
    ///
    /// Since `TransactionHashNumbers` is keyed by `TxHash`, we must iterate all entries
    /// and delete those whose value exceeds the threshold.
    fn prune_transaction_hash_numbers_above(&self, max_tx_number: u64) -> ProviderResult<()> {
        let mut to_delete = Vec::new();

        // Iterate all entries and collect those to delete
        for result in self.iter::<tables::TransactionHashNumbers>()? {
            let (key, tx_number) = result?;

            if tx_number > max_tx_number {
                to_delete.push(key);
            }
        }

        let deleted_count = to_delete.len();
        if deleted_count > 0 {
            tracing::info!(
                target: "reth::providers::rocksdb",
                deleted_count,
                max_tx_number,
                "Pruning TransactionHashNumbers entries"
            );

            self.write_batch(|batch| {
                for key in to_delete {
                    batch.delete::<tables::TransactionHashNumbers>(key)?;
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
        providers::rocksdb::RocksDBBuilder, test_utils::create_test_provider_factory,
        DatabaseProviderFactory, StageCheckpointWriter,
    };
    use alloy_primitives::{Address, B256};
    use reth_db::cursor::DbCursorRW;
    use reth_db_api::{
        models::{storage_sharded_key::StorageShardedKey, StorageSettings},
        tables::{self, BlockNumberList},
        transaction::DbTxMut,
    };
    use reth_stages_types::StageCheckpoint;
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

        // Insert data into RocksDB
        let tx_hash = B256::from([1u8; 32]);
        rocksdb.put::<tables::TransactionHashNumbers>(tx_hash, &100).unwrap();

        // Verify data exists
        assert!(rocksdb.last::<tables::TransactionHashNumbers>().unwrap().is_some());

        // Create a test provider factory for MDBX with NO checkpoint
        let factory = create_test_provider_factory();
        factory.set_storage_settings_cache(
            StorageSettings::legacy().with_transaction_hash_numbers_in_rocksdb(true),
        );

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

        // Insert data into RocksDB - tx hashes for tx_numbers 50, 100, 150
        let tx_hash_1 = B256::from([1u8; 32]);
        let tx_hash_2 = B256::from([2u8; 32]);
        let tx_hash_3 = B256::from([3u8; 32]);
        rocksdb.put::<tables::TransactionHashNumbers>(tx_hash_1, &50).unwrap();
        rocksdb.put::<tables::TransactionHashNumbers>(tx_hash_2, &100).unwrap();
        rocksdb.put::<tables::TransactionHashNumbers>(tx_hash_3, &150).unwrap();

        // Create a test provider factory for MDBX
        let factory = create_test_provider_factory();
        factory.set_storage_settings_cache(
            StorageSettings::legacy().with_transaction_hash_numbers_in_rocksdb(true),
        );

        // Set up MDBX with:
        // - TransactionBlocks: tx 50 is in block 10, tx 100 is in block 20, tx 150 is in block 30
        // - Checkpoint at block 20 (meaning checkpoint should only have up to tx 100)
        {
            let provider = factory.database_provider_rw().unwrap();

            // Insert TransactionBlocks entries
            let mut cursor = provider.tx_ref().cursor_write::<tables::TransactionBlocks>().unwrap();
            cursor.append(50, &10).unwrap(); // tx 50 is the last tx in block 10
            cursor.append(100, &20).unwrap(); // tx 100 is the last tx in block 20
            cursor.append(150, &30).unwrap(); // tx 150 is the last tx in block 30

            // Set checkpoint to block 20
            provider
                .save_stage_checkpoint(StageId::TransactionLookup, StageCheckpoint::new(20))
                .unwrap();
            provider.commit().unwrap();
        }

        let provider = factory.database_provider_ro().unwrap();

        // RocksDB has tx hashes up to tx 150 (block 30)
        // But checkpoint says we only processed up to block 20 (tx 100)
        // This means RocksDB is ahead and should prune entries > tx 100
        let result = rocksdb.check_consistency(&provider).unwrap();
        assert_eq!(result, None, "Should heal by pruning, no unwind needed");

        // Verify tx_hash_3 (tx 150) was pruned, but tx_hash_1 (tx 50) and tx_hash_2 (tx 100) remain
        assert_eq!(
            rocksdb.get::<tables::TransactionHashNumbers>(tx_hash_1).unwrap(),
            Some(50),
            "tx 50 should remain"
        );
        assert_eq!(
            rocksdb.get::<tables::TransactionHashNumbers>(tx_hash_2).unwrap(),
            Some(100),
            "tx 100 should remain"
        );
        assert_eq!(
            rocksdb.get::<tables::TransactionHashNumbers>(tx_hash_3).unwrap(),
            None,
            "tx 150 should be pruned"
        );
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
}
