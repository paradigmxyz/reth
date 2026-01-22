//! Invariant checking for `RocksDB` tables.
//!
//! This module provides consistency checks for tables stored in `RocksDB`, similar to the
//! consistency checks for static files. The goal is to detect and potentially heal
//! inconsistencies between `RocksDB` data and MDBX checkpoints.

use super::RocksDBProvider;
use crate::StaticFileProviderFactory;
use alloy_eips::eip2718::Encodable2718;
use alloy_primitives::BlockNumber;
use rayon::prelude::*;
use reth_db_api::tables;
use reth_stages_types::StageId;
use reth_static_file_types::StaticFileSegment;
use reth_storage_api::{
    BlockBodyIndicesProvider, DBProvider, StageCheckpointReader, StorageSettingsCache,
    TransactionsProvider,
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
            + StaticFileProviderFactory
            + BlockBodyIndicesProvider
            + TransactionsProvider<Transaction: Encodable2718>,
    {
        let mut unwind_target: Option<BlockNumber> = None;

        // Heal TransactionHashNumbers if stored in RocksDB
        if provider.cached_storage_settings().transaction_hash_numbers_in_rocksdb &&
            let Some(target) = self.heal_transaction_hash_numbers(provider)?
        {
            unwind_target = Some(unwind_target.map_or(target, |t| t.min(target)));
        }

        // Check StoragesHistory if stored in RocksDB
        if provider.cached_storage_settings().storages_history_in_rocksdb &&
            let Some(target) = self.check_storages_history(provider)?
        {
            unwind_target = Some(unwind_target.map_or(target, |t| t.min(target)));
        }

        // Check AccountsHistory if stored in RocksDB
        if provider.cached_storage_settings().account_history_in_rocksdb &&
            let Some(target) = self.check_accounts_history(provider)?
        {
            unwind_target = Some(unwind_target.map_or(target, |t| t.min(target)));
        }

        Ok(unwind_target)
    }

    /// Heals the `TransactionHashNumbers` table.
    ///
    /// - Fast path: if checkpoint == 0 AND `RocksDB` has data, clear everything
    /// - If `sf_tip` < checkpoint, return unwind target (static files behind)
    /// - If `sf_tip` == checkpoint, nothing to do
    /// - If `sf_tip` > checkpoint, heal via transaction ranges in batches
    fn heal_transaction_hash_numbers<Provider>(
        &self,
        provider: &Provider,
    ) -> ProviderResult<Option<BlockNumber>>
    where
        Provider: DBProvider
            + StageCheckpointReader
            + StaticFileProviderFactory
            + BlockBodyIndicesProvider
            + TransactionsProvider<Transaction: Encodable2718>,
    {
        let checkpoint = provider
            .get_stage_checkpoint(StageId::TransactionLookup)?
            .map(|cp| cp.block_number)
            .unwrap_or(0);

        let sf_tip = provider
            .static_file_provider()
            .get_highest_static_file_block(StaticFileSegment::Transactions)
            .unwrap_or(0);

        // Fast path: if checkpoint is 0 and RocksDB has data, clear everything.
        if checkpoint == 0 && self.first::<tables::TransactionHashNumbers>()?.is_some() {
            tracing::info!(
                target: "reth::providers::rocksdb",
                "TransactionHashNumbers has data but checkpoint is 0, clearing all"
            );
            self.clear::<tables::TransactionHashNumbers>()?;
            return Ok(None);
        }

        if sf_tip < checkpoint {
            // This should never happen in normal operation - static files are always committed
            // before RocksDB. If we get here, something is seriously wrong. The unwind is a
            // best-effort attempt but is probably futile.
            tracing::warn!(
                target: "reth::providers::rocksdb",
                sf_tip,
                checkpoint,
                "TransactionHashNumbers: static file tip behind checkpoint, unwind needed"
            );
            return Ok(Some(sf_tip));
        }

        // sf_tip == checkpoint - nothing to do
        if sf_tip == checkpoint {
            return Ok(None);
        }

        // Get end tx from static files (authoritative for sf_tip)
        let sf_tip_end_tx = provider
            .static_file_provider()
            .get_highest_static_file_tx(StaticFileSegment::Transactions)
            .unwrap_or(0);

        // Get the first tx after the checkpoint block from MDBX (authoritative up to checkpoint)
        let checkpoint_next_tx = provider
            .block_body_indices(checkpoint)?
            .map(|indices| indices.next_tx_num())
            .unwrap_or(0);

        if sf_tip_end_tx < checkpoint_next_tx {
            // This should never happen in normal operation - static files should have all
            // transactions up to sf_tip. If we get here, something is seriously wrong.
            // The unwind is a best-effort attempt but is probably futile.
            tracing::warn!(
                target: "reth::providers::rocksdb",
                sf_tip_end_tx,
                checkpoint_next_tx,
                checkpoint,
                sf_tip,
                "TransactionHashNumbers: static file tx tip behind checkpoint, unwind needed"
            );
            return Ok(Some(sf_tip));
        }

        tracing::info!(
            target: "reth::providers::rocksdb",
            checkpoint,
            sf_tip,
            checkpoint_next_tx,
            sf_tip_end_tx,
            "TransactionHashNumbers: healing via transaction ranges"
        );

        const BATCH_SIZE: u64 = 10_000;
        let mut batch_start = checkpoint_next_tx;

        while batch_start <= sf_tip_end_tx {
            let batch_end = batch_start.saturating_add(BATCH_SIZE - 1).min(sf_tip_end_tx);

            tracing::debug!(
                target: "reth::providers::rocksdb",
                batch_start,
                batch_end,
                "Pruning TransactionHashNumbers batch"
            );

            self.prune_transaction_hash_numbers_in_range(provider, batch_start..=batch_end)?;

            batch_start = batch_end.saturating_add(1);
        }

        Ok(None)
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
    fn prune_transaction_hash_numbers_in_range<Provider>(
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

        if !hashes.is_empty() {
            tracing::info!(
                target: "reth::providers::rocksdb",
                deleted_count = hashes.len(),
                tx_range_start = *tx_range.start(),
                tx_range_end = *tx_range.end(),
                "Pruning TransactionHashNumbers entries by tx range"
            );

            let mut batch = self.batch();
            for hash in hashes {
                batch.delete::<tables::TransactionHashNumbers>(hash)?;
            }
            batch.commit()?;
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
                // entries. Also track if we found any non-sentinel entries.
                let mut max_highest_block = 0u64;
                let mut found_non_sentinel = false;
                for result in self.iter::<tables::StoragesHistory>()? {
                    let (key, _) = result?;
                    let highest = key.sharded_key.highest_block_number;
                    if highest != u64::MAX {
                        found_non_sentinel = true;
                        if highest > max_highest_block {
                            max_highest_block = highest;
                        }
                    }
                }

                // If all entries are sentinel entries (u64::MAX), treat as first-run scenario.
                // This means no completed shards exist (only sentinel shards with
                // highest_block_number=u64::MAX), so no actual history has been indexed.
                if !found_non_sentinel {
                    return Ok(None);
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
                } else if max_highest_block < checkpoint {
                    // RocksDB is behind checkpoint, return highest block to signal unwind needed
                    tracing::warn!(
                        target: "reth::providers::rocksdb",
                        rocks_highest = max_highest_block,
                        checkpoint,
                        "StoragesHistory behind checkpoint, unwind needed"
                    );
                    return Ok(Some(max_highest_block));
                }

                Ok(None)
            }
            None => {
                // Empty RocksDB table, nothing to check.
                Ok(None)
            }
        }
    }

    /// Prunes `StoragesHistory` entries where `highest_block_number` > `max_block`.
    ///
    /// For `StoragesHistory`, the key contains `highest_block_number`, so we can iterate
    /// and delete entries where `key.sharded_key.highest_block_number > max_block`.
    ///
    /// TODO(<https://github.com/paradigmxyz/reth/issues/20417>): this iterates the whole table,
    /// which is inefficient. Use changeset-based pruning instead.
    fn prune_storages_history_above(&self, max_block: BlockNumber) -> ProviderResult<()> {
        use reth_db_api::models::storage_sharded_key::StorageShardedKey;

        let mut to_delete: Vec<StorageShardedKey> = Vec::new();
        for result in self.iter::<tables::StoragesHistory>()? {
            let (key, _) = result?;
            let highest_block = key.sharded_key.highest_block_number;
            if max_block == 0 || (highest_block != u64::MAX && highest_block > max_block) {
                to_delete.push(key);
            }
        }

        let deleted = to_delete.len();
        if deleted > 0 {
            tracing::info!(
                target: "reth::providers::rocksdb",
                deleted_count = deleted,
                max_block,
                "Pruning StoragesHistory entries"
            );

            let mut batch = self.batch();
            for key in to_delete {
                batch.delete::<tables::StoragesHistory>(key)?;
            }
            batch.commit()?;
        }

        Ok(())
    }

    /// Checks invariants for the `AccountsHistory` table.
    ///
    /// Returns a block number to unwind to if `RocksDB` is behind the checkpoint.
    /// If `RocksDB` is ahead of the checkpoint, excess entries are pruned (healed).
    fn check_accounts_history<Provider>(
        &self,
        provider: &Provider,
    ) -> ProviderResult<Option<BlockNumber>>
    where
        Provider: DBProvider + StageCheckpointReader,
    {
        // Get the IndexAccountHistory stage checkpoint
        let checkpoint = provider
            .get_stage_checkpoint(StageId::IndexAccountHistory)?
            .map(|cp| cp.block_number)
            .unwrap_or(0);

        // Check if RocksDB has any data
        let rocks_first = self.first::<tables::AccountsHistory>()?;

        match rocks_first {
            Some(_) => {
                // If checkpoint is 0 but we have data, clear everything
                if checkpoint == 0 {
                    tracing::info!(
                        target: "reth::providers::rocksdb",
                        "AccountsHistory has data but checkpoint is 0, clearing all"
                    );
                    self.prune_accounts_history_above(0)?;
                    return Ok(None);
                }

                // Find the max highest_block_number (excluding u64::MAX sentinel) across all
                // entries. Also track if we found any non-sentinel entries.
                let mut max_highest_block = 0u64;
                let mut found_non_sentinel = false;
                for result in self.iter::<tables::AccountsHistory>()? {
                    let (key, _) = result?;
                    let highest = key.highest_block_number;
                    if highest != u64::MAX {
                        found_non_sentinel = true;
                        if highest > max_highest_block {
                            max_highest_block = highest;
                        }
                    }
                }

                // If all entries are sentinel entries (u64::MAX), treat as first-run scenario.
                // This means no completed shards exist (only sentinel shards with
                // highest_block_number=u64::MAX), so no actual history has been indexed.
                if !found_non_sentinel {
                    return Ok(None);
                }

                // If any entry has highest_block > checkpoint, prune excess
                if max_highest_block > checkpoint {
                    tracing::info!(
                        target: "reth::providers::rocksdb",
                        rocks_highest = max_highest_block,
                        checkpoint,
                        "AccountsHistory ahead of checkpoint, pruning excess data"
                    );
                    self.prune_accounts_history_above(checkpoint)?;
                    return Ok(None);
                }

                // If RocksDB is behind the checkpoint, request an unwind to rebuild.
                if max_highest_block < checkpoint {
                    tracing::warn!(
                        target: "reth::providers::rocksdb",
                        rocks_highest = max_highest_block,
                        checkpoint,
                        "AccountsHistory behind checkpoint, unwind needed"
                    );
                    return Ok(Some(max_highest_block));
                }

                Ok(None)
            }
            None => {
                // Empty RocksDB table, nothing to check.
                Ok(None)
            }
        }
    }

    /// Prunes `AccountsHistory` entries where `highest_block_number` > `max_block`.
    ///
    /// For `AccountsHistory`, the key is `ShardedKey<Address>` which contains
    /// `highest_block_number`, so we can iterate and delete entries where
    /// `key.highest_block_number > max_block`.
    ///
    /// TODO(<https://github.com/paradigmxyz/reth/issues/20417>): this iterates the whole table,
    /// which is inefficient. Use changeset-based pruning instead.
    fn prune_accounts_history_above(&self, max_block: BlockNumber) -> ProviderResult<()> {
        use alloy_primitives::Address;
        use reth_db_api::models::ShardedKey;

        let mut to_delete: Vec<ShardedKey<Address>> = Vec::new();
        for result in self.iter::<tables::AccountsHistory>()? {
            let (key, _) = result?;
            let highest_block = key.highest_block_number;
            if max_block == 0 || (highest_block != u64::MAX && highest_block > max_block) {
                to_delete.push(key);
            }
        }

        let deleted = to_delete.len();
        if deleted > 0 {
            tracing::info!(
                target: "reth::providers::rocksdb",
                deleted_count = deleted,
                max_block,
                "Pruning AccountsHistory entries"
            );

            let mut batch = self.batch();
            for key in to_delete {
                batch.delete::<tables::AccountsHistory>(key)?;
            }
            batch.commit()?;
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
    use reth_db::cursor::{DbCursorRO, DbCursorRW};
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
    fn test_check_consistency_empty_rocksdb_with_checkpoint_is_first_run() {
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

        // RocksDB is empty but checkpoint says block 100 was processed.
        // Since static file tip defaults to 0 when None, and 0 < 100, an unwind is triggered.
        let result = rocksdb.check_consistency(&provider).unwrap();
        assert_eq!(result, Some(0), "Static file tip (0) behind checkpoint (100) triggers unwind");
    }

    /// Tests that when checkpoint=0 and `RocksDB` has data, all entries are pruned.
    /// This simulates a crash recovery scenario where the checkpoint was lost.
    #[test]
    fn test_check_consistency_checkpoint_zero_with_rocksdb_data_prunes_all() {
        let temp_dir = TempDir::new().unwrap();
        let rocksdb = RocksDBBuilder::new(temp_dir.path())
            .with_table::<tables::TransactionHashNumbers>()
            .build()
            .unwrap();

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

        let mut tx_hashes = Vec::new();
        {
            let provider = factory.database_provider_rw().unwrap();
            let mut tx_count = 0u64;
            for block in &blocks {
                provider
                    .insert_block(&block.clone().try_recover().expect("recover block"))
                    .unwrap();
                for tx in &block.body().transactions {
                    let hash = tx.trie_hash();
                    tx_hashes.push(hash);
                    rocksdb.put::<tables::TransactionHashNumbers>(hash, &tx_count).unwrap();
                    tx_count += 1;
                }
            }
            provider.commit().unwrap();
        }

        // Explicitly clear the TransactionLookup checkpoint to simulate crash recovery
        {
            let provider = factory.database_provider_rw().unwrap();
            provider
                .save_stage_checkpoint(StageId::TransactionLookup, StageCheckpoint::new(0))
                .unwrap();
            provider.commit().unwrap();
        }

        // Verify RocksDB data exists
        assert!(rocksdb.last::<tables::TransactionHashNumbers>().unwrap().is_some());

        let provider = factory.database_provider_ro().unwrap();

        // checkpoint = 0 but RocksDB has data.
        // This means RocksDB has stale data that should be cleared.
        let result = rocksdb.check_consistency(&provider).unwrap();
        assert_eq!(result, None, "Should heal by clearing, no unwind needed");

        // Verify data was cleared
        for hash in &tx_hashes {
            assert!(
                rocksdb.get::<tables::TransactionHashNumbers>(*hash).unwrap().is_none(),
                "RocksDB should be empty after pruning"
            );
        }
    }

    #[test]
    fn test_check_consistency_storages_history_empty_with_checkpoint_is_first_run() {
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

        // RocksDB is empty but checkpoint says block 100 was processed.
        // This is treated as a first-run/migration scenario - no unwind needed.
        let result = rocksdb.check_consistency(&provider).unwrap();
        assert_eq!(result, None, "Empty RocksDB with checkpoint is treated as first run");
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
    fn test_check_consistency_storages_history_behind_checkpoint_needs_unwind() {
        let temp_dir = TempDir::new().unwrap();
        let rocksdb = RocksDBBuilder::new(temp_dir.path())
            .with_table::<tables::StoragesHistory>()
            .build()
            .unwrap();

        // Insert data into RocksDB with max highest_block_number = 80
        let key_block_50 = StorageShardedKey::new(Address::ZERO, B256::ZERO, 50);
        let key_block_80 = StorageShardedKey::new(Address::ZERO, B256::from([1u8; 32]), 80);
        let key_block_max = StorageShardedKey::new(Address::ZERO, B256::from([2u8; 32]), u64::MAX);

        let block_list = BlockNumberList::new_pre_sorted([10, 20, 30]);
        rocksdb.put::<tables::StoragesHistory>(key_block_50, &block_list).unwrap();
        rocksdb.put::<tables::StoragesHistory>(key_block_80, &block_list).unwrap();
        rocksdb.put::<tables::StoragesHistory>(key_block_max, &block_list).unwrap();

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

        // RocksDB max highest_block (80) is behind checkpoint (100)
        let result = rocksdb.check_consistency(&provider).unwrap();
        assert_eq!(result, Some(80), "Should unwind to the highest block present in RocksDB");
    }

    #[test]
    fn test_check_consistency_sf_tip_behind_checkpoint_needs_unwind() {
        let temp_dir = TempDir::new().unwrap();
        let rocksdb = RocksDBBuilder::new(temp_dir.path())
            .with_table::<tables::TransactionHashNumbers>()
            .build()
            .unwrap();

        let factory = create_test_provider_factory();
        factory.set_storage_settings_cache(
            StorageSettings::legacy().with_transaction_hash_numbers_in_rocksdb(true),
        );

        // Generate blocks with real transactions (blocks 0-2, 6 transactions total)
        let mut rng = generators::rng();
        let blocks = generators::random_block_range(
            &mut rng,
            0..=2,
            BlockRangeParams { parent: Some(B256::ZERO), tx_count: 2..3, ..Default::default() },
        );

        {
            let provider = factory.database_provider_rw().unwrap();
            let mut tx_count = 0u64;
            for block in &blocks {
                provider
                    .insert_block(&block.clone().try_recover().expect("recover block"))
                    .unwrap();
                for tx in &block.body().transactions {
                    let hash = tx.trie_hash();
                    rocksdb.put::<tables::TransactionHashNumbers>(hash, &tx_count).unwrap();
                    tx_count += 1;
                }
            }
            provider.commit().unwrap();
        }

        // Set checkpoint to block 10 (beyond our actual data at block 2)
        // sf_tip is at block 2, checkpoint is at block 10
        // Since sf_tip < checkpoint, we need to unwind to sf_tip
        {
            let provider = factory.database_provider_rw().unwrap();
            provider
                .save_stage_checkpoint(StageId::TransactionLookup, StageCheckpoint::new(10))
                .unwrap();
            provider.commit().unwrap();
        }

        let provider = factory.database_provider_ro().unwrap();

        // sf_tip (2) < checkpoint (10), so unwind to sf_tip is needed
        let result = rocksdb.check_consistency(&provider).unwrap();
        assert_eq!(result, Some(2), "sf_tip < checkpoint requires unwind to sf_tip");
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
            // Insert ALL blocks (0-5) to write transactions to static files
            for block in &blocks {
                provider
                    .insert_block(&block.clone().try_recover().expect("recover block"))
                    .unwrap();
                for tx in &block.body().transactions {
                    let hash = tx.trie_hash();
                    tx_hashes.push(hash);
                    rocksdb.put::<tables::TransactionHashNumbers>(hash, &tx_count).unwrap();
                    tx_count += 1;
                }
            }
            provider.commit().unwrap();
        }

        // Simulate crash recovery scenario:
        // MDBX was unwound to block 2, but RocksDB and static files still have more data.
        // Remove TransactionBlocks entries for blocks 3-5 to simulate MDBX unwind.
        {
            let provider = factory.database_provider_rw().unwrap();
            // Delete TransactionBlocks entries for tx > 5 (i.e., for blocks 3-5)
            // TransactionBlocks maps last_tx_in_block -> block_number
            // After unwind, only entries for blocks 0-2 should remain (tx 5 -> block 2)
            let mut cursor = provider.tx_ref().cursor_write::<tables::TransactionBlocks>().unwrap();
            // Walk and delete entries where block > 2
            let mut to_delete = Vec::new();
            let mut walker = cursor.walk(Some(0)).unwrap();
            while let Some((tx_num, block_num)) = walker.next().transpose().unwrap() {
                if block_num > 2 {
                    to_delete.push(tx_num);
                }
            }
            drop(walker);
            for tx_num in to_delete {
                cursor.seek_exact(tx_num).unwrap();
                cursor.delete_current().unwrap();
            }

            // Set checkpoint to block 2
            provider
                .save_stage_checkpoint(StageId::TransactionLookup, StageCheckpoint::new(2))
                .unwrap();
            provider.commit().unwrap();
        }

        let provider = factory.database_provider_ro().unwrap();

        // RocksDB has tx hashes for all blocks (0-5)
        // MDBX TransactionBlocks only goes up to tx 5 (block 2)
        // Static files have data for all txs (0-11)
        // This means RocksDB is ahead and should prune entries for tx 6-11
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

    #[test]
    fn test_check_consistency_storages_history_sentinel_only_with_checkpoint_is_first_run() {
        let temp_dir = TempDir::new().unwrap();
        let rocksdb = RocksDBBuilder::new(temp_dir.path())
            .with_table::<tables::StoragesHistory>()
            .build()
            .unwrap();

        // Insert ONLY sentinel entries (highest_block_number = u64::MAX)
        // This simulates a scenario where history tracking started but no shards were completed
        let key_sentinel_1 = StorageShardedKey::new(Address::ZERO, B256::ZERO, u64::MAX);
        let key_sentinel_2 = StorageShardedKey::new(Address::random(), B256::random(), u64::MAX);
        let block_list = BlockNumberList::new_pre_sorted([10, 20, 30]);
        rocksdb.put::<tables::StoragesHistory>(key_sentinel_1, &block_list).unwrap();
        rocksdb.put::<tables::StoragesHistory>(key_sentinel_2, &block_list).unwrap();

        // Verify entries exist (not empty table)
        assert!(rocksdb.first::<tables::StoragesHistory>().unwrap().is_some());

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

        // RocksDB has only sentinel entries (no completed shards) but checkpoint is set.
        // This is treated as a first-run/migration scenario - no unwind needed.
        let result = rocksdb.check_consistency(&provider).unwrap();
        assert_eq!(
            result, None,
            "Sentinel-only entries with checkpoint should be treated as first run"
        );
    }

    #[test]
    fn test_check_consistency_accounts_history_sentinel_only_with_checkpoint_is_first_run() {
        use reth_db_api::models::ShardedKey;

        let temp_dir = TempDir::new().unwrap();
        let rocksdb = RocksDBBuilder::new(temp_dir.path())
            .with_table::<tables::AccountsHistory>()
            .build()
            .unwrap();

        // Insert ONLY sentinel entries (highest_block_number = u64::MAX)
        let key_sentinel_1 = ShardedKey::new(Address::ZERO, u64::MAX);
        let key_sentinel_2 = ShardedKey::new(Address::random(), u64::MAX);
        let block_list = BlockNumberList::new_pre_sorted([10, 20, 30]);
        rocksdb.put::<tables::AccountsHistory>(key_sentinel_1, &block_list).unwrap();
        rocksdb.put::<tables::AccountsHistory>(key_sentinel_2, &block_list).unwrap();

        // Verify entries exist (not empty table)
        assert!(rocksdb.first::<tables::AccountsHistory>().unwrap().is_some());

        // Create a test provider factory for MDBX
        let factory = create_test_provider_factory();
        factory.set_storage_settings_cache(
            StorageSettings::legacy().with_account_history_in_rocksdb(true),
        );

        // Set a checkpoint indicating we should have processed up to block 100
        {
            let provider = factory.database_provider_rw().unwrap();
            provider
                .save_stage_checkpoint(StageId::IndexAccountHistory, StageCheckpoint::new(100))
                .unwrap();
            provider.commit().unwrap();
        }

        let provider = factory.database_provider_ro().unwrap();

        // RocksDB has only sentinel entries (no completed shards) but checkpoint is set.
        // This is treated as a first-run/migration scenario - no unwind needed.
        let result = rocksdb.check_consistency(&provider).unwrap();
        assert_eq!(
            result, None,
            "Sentinel-only entries with checkpoint should be treated as first run"
        );
    }

    #[test]
    fn test_check_consistency_storages_history_behind_checkpoint_single_entry() {
        use reth_db_api::models::storage_sharded_key::StorageShardedKey;

        let temp_dir = TempDir::new().unwrap();
        let rocksdb = RocksDBBuilder::new(temp_dir.path())
            .with_table::<tables::StoragesHistory>()
            .build()
            .unwrap();

        // Insert data into RocksDB with highest_block_number below checkpoint
        let key_block_50 = StorageShardedKey::new(Address::ZERO, B256::ZERO, 50);
        let block_list = BlockNumberList::new_pre_sorted([10, 20, 30, 50]);
        rocksdb.put::<tables::StoragesHistory>(key_block_50, &block_list).unwrap();

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

        // RocksDB only has data up to block 50, but checkpoint says block 100 was processed
        let result = rocksdb.check_consistency(&provider).unwrap();
        assert_eq!(
            result,
            Some(50),
            "Should require unwind to block 50 to rebuild StoragesHistory"
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
                provider
                    .insert_block(&block.clone().try_recover().expect("recover block"))
                    .unwrap();

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

    #[test]
    fn test_check_consistency_accounts_history_empty_with_checkpoint_is_first_run() {
        let temp_dir = TempDir::new().unwrap();
        let rocksdb = RocksDBBuilder::new(temp_dir.path())
            .with_table::<tables::AccountsHistory>()
            .build()
            .unwrap();

        // Create a test provider factory for MDBX
        let factory = create_test_provider_factory();
        factory.set_storage_settings_cache(
            StorageSettings::legacy().with_account_history_in_rocksdb(true),
        );

        // Set a checkpoint indicating we should have processed up to block 100
        {
            let provider = factory.database_provider_rw().unwrap();
            provider
                .save_stage_checkpoint(StageId::IndexAccountHistory, StageCheckpoint::new(100))
                .unwrap();
            provider.commit().unwrap();
        }

        let provider = factory.database_provider_ro().unwrap();

        // RocksDB is empty but checkpoint says block 100 was processed.
        // This is treated as a first-run/migration scenario - no unwind needed.
        let result = rocksdb.check_consistency(&provider).unwrap();
        assert_eq!(result, None, "Empty RocksDB with checkpoint is treated as first run");
    }

    #[test]
    fn test_check_consistency_accounts_history_has_data_no_checkpoint_prunes_data() {
        use reth_db_api::models::ShardedKey;

        let temp_dir = TempDir::new().unwrap();
        let rocksdb = RocksDBBuilder::new(temp_dir.path())
            .with_table::<tables::AccountsHistory>()
            .build()
            .unwrap();

        // Insert data into RocksDB
        let key = ShardedKey::new(Address::ZERO, 50);
        let block_list = BlockNumberList::new_pre_sorted([10, 20, 30, 50]);
        rocksdb.put::<tables::AccountsHistory>(key, &block_list).unwrap();

        // Verify data exists
        assert!(rocksdb.last::<tables::AccountsHistory>().unwrap().is_some());

        // Create a test provider factory for MDBX with NO checkpoint
        let factory = create_test_provider_factory();
        factory.set_storage_settings_cache(
            StorageSettings::legacy().with_account_history_in_rocksdb(true),
        );

        let provider = factory.database_provider_ro().unwrap();

        // RocksDB has data but checkpoint is 0
        // This means RocksDB has stale data that should be pruned (healed)
        let result = rocksdb.check_consistency(&provider).unwrap();
        assert_eq!(result, None, "Should heal by pruning, no unwind needed");

        // Verify data was pruned
        assert!(
            rocksdb.last::<tables::AccountsHistory>().unwrap().is_none(),
            "RocksDB should be empty after pruning"
        );
    }

    #[test]
    fn test_check_consistency_accounts_history_ahead_of_checkpoint_prunes_excess() {
        use reth_db_api::models::ShardedKey;

        let temp_dir = TempDir::new().unwrap();
        let rocksdb = RocksDBBuilder::new(temp_dir.path())
            .with_table::<tables::AccountsHistory>()
            .build()
            .unwrap();

        // Insert data into RocksDB with different highest_block_numbers
        let key_block_50 = ShardedKey::new(Address::ZERO, 50);
        let key_block_100 = ShardedKey::new(Address::random(), 100);
        let key_block_150 = ShardedKey::new(Address::random(), 150);
        let key_block_max = ShardedKey::new(Address::random(), u64::MAX);

        let block_list = BlockNumberList::new_pre_sorted([10, 20, 30]);
        rocksdb.put::<tables::AccountsHistory>(key_block_50.clone(), &block_list).unwrap();
        rocksdb.put::<tables::AccountsHistory>(key_block_100.clone(), &block_list).unwrap();
        rocksdb.put::<tables::AccountsHistory>(key_block_150.clone(), &block_list).unwrap();
        rocksdb.put::<tables::AccountsHistory>(key_block_max.clone(), &block_list).unwrap();

        // Create a test provider factory for MDBX
        let factory = create_test_provider_factory();
        factory.set_storage_settings_cache(
            StorageSettings::legacy().with_account_history_in_rocksdb(true),
        );

        // Set checkpoint to block 100
        {
            let provider = factory.database_provider_rw().unwrap();
            provider
                .save_stage_checkpoint(StageId::IndexAccountHistory, StageCheckpoint::new(100))
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
            rocksdb.get::<tables::AccountsHistory>(key_block_50).unwrap().is_some(),
            "Entry with highest_block=50 should remain"
        );
        assert!(
            rocksdb.get::<tables::AccountsHistory>(key_block_100).unwrap().is_some(),
            "Entry with highest_block=100 should remain"
        );
        assert!(
            rocksdb.get::<tables::AccountsHistory>(key_block_150).unwrap().is_none(),
            "Entry with highest_block=150 should be pruned"
        );
        assert!(
            rocksdb.get::<tables::AccountsHistory>(key_block_max).unwrap().is_some(),
            "Entry with highest_block=u64::MAX (sentinel) should remain"
        );
    }

    #[test]
    fn test_check_consistency_accounts_history_behind_checkpoint_needs_unwind() {
        use reth_db_api::models::ShardedKey;

        let temp_dir = TempDir::new().unwrap();
        let rocksdb = RocksDBBuilder::new(temp_dir.path())
            .with_table::<tables::AccountsHistory>()
            .build()
            .unwrap();

        // Insert data into RocksDB with highest_block_number below checkpoint
        let key_block_50 = ShardedKey::new(Address::ZERO, 50);
        let block_list = BlockNumberList::new_pre_sorted([10, 20, 30, 50]);
        rocksdb.put::<tables::AccountsHistory>(key_block_50, &block_list).unwrap();

        let factory = create_test_provider_factory();
        factory.set_storage_settings_cache(
            StorageSettings::legacy().with_account_history_in_rocksdb(true),
        );

        // Set checkpoint to block 100
        {
            let provider = factory.database_provider_rw().unwrap();
            provider
                .save_stage_checkpoint(StageId::IndexAccountHistory, StageCheckpoint::new(100))
                .unwrap();
            provider.commit().unwrap();
        }

        let provider = factory.database_provider_ro().unwrap();

        // RocksDB only has data up to block 50, but checkpoint says block 100 was processed
        let result = rocksdb.check_consistency(&provider).unwrap();
        assert_eq!(
            result,
            Some(50),
            "Should require unwind to block 50 to rebuild AccountsHistory"
        );
    }
}
