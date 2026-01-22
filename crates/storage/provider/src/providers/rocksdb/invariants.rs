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
use reth_db::cursor::DbCursorRO;
use reth_db_api::{tables, transaction::DbTx};
use reth_stages_types::StageId;
use reth_static_file_types::StaticFileSegment;
use reth_storage_api::{
    ChangeSetReader, DBProvider, StageCheckpointReader, StorageChangeSetReader,
    StorageSettingsCache, TransactionsProvider,
};
use reth_storage_errors::provider::ProviderResult;
use std::collections::HashSet;

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
            + StorageChangeSetReader
            + ChangeSetReader
            + TransactionsProvider<Transaction: Encodable2718>,
    {
        let mut unwind_target: Option<BlockNumber> = None;

        // Heal TransactionHashNumbers if stored in RocksDB
        if provider.cached_storage_settings().transaction_hash_numbers_in_rocksdb &&
            let Some(target) = self.heal_transaction_hash_numbers(provider)?
        {
            unwind_target = Some(unwind_target.map_or(target, |t| t.min(target)));
        }

        // Heal StoragesHistory if stored in RocksDB
        if provider.cached_storage_settings().storages_history_in_rocksdb {
            self.heal_storages_history(provider)?;
        }

        // Heal AccountsHistory if stored in RocksDB
        if provider.cached_storage_settings().account_history_in_rocksdb {
            self.heal_accounts_history(provider)?;
        }

        Ok(unwind_target)
    }

    /// Heals the `TransactionHashNumbers` table by removing stale entries.
    ///
    /// Returns a block number to unwind to if MDBX is behind the checkpoint.
    /// If static files are ahead of MDBX, prunes excess `RocksDB` entries.
    ///
    /// # Approach
    ///
    /// Uses static files and MDBX to determine what needs pruning (avoids expensive
    /// `RocksDB` iteration):
    /// - Static files are committed before `RocksDB`, so they're at least at the same height
    /// - MDBX `TransactionBlocks` tells us what's been fully committed
    /// - If static files have more transactions than MDBX, prune the excess range
    fn heal_transaction_hash_numbers<Provider>(
        &self,
        provider: &Provider,
    ) -> ProviderResult<Option<BlockNumber>>
    where
        Provider: DBProvider
            + StageCheckpointReader
            + StaticFileProviderFactory
            + TransactionsProvider<Transaction: Encodable2718>,
    {
        // Get the TransactionLookup stage checkpoint
        let checkpoint = provider
            .get_stage_checkpoint(StageId::TransactionLookup)?
            .map(|cp| cp.block_number)
            .unwrap_or(0);

        // Get last tx_num from MDBX - this tells us what MDBX has fully committed
        let mut cursor = provider.tx_ref().cursor_read::<tables::TransactionBlocks>()?;
        let mdbx_last = cursor.last()?;

        // Get highest tx_num from static files - this tells us what tx data is available
        let highest_static_tx = provider
            .static_file_provider()
            .get_highest_static_file_tx(StaticFileSegment::Transactions);

        match (mdbx_last, highest_static_tx) {
            (Some((mdbx_tx, mdbx_block)), Some(highest_tx)) if highest_tx > mdbx_tx => {
                // Static files are ahead of MDBX - prune RocksDB entries for the excess range.
                // This is the common case during recovery from a crash during unwinding.
                tracing::info!(
                    target: "reth::providers::rocksdb",
                    mdbx_last_tx = mdbx_tx,
                    mdbx_block,
                    highest_static_tx = highest_tx,
                    "Static files ahead of MDBX, pruning TransactionHashNumbers excess data"
                );
                self.prune_transaction_hash_numbers_in_range(provider, (mdbx_tx + 1)..=highest_tx)?;

                // After pruning, check if MDBX is behind checkpoint
                if checkpoint > mdbx_block {
                    tracing::warn!(
                        target: "reth::providers::rocksdb",
                        mdbx_block,
                        checkpoint,
                        "MDBX behind checkpoint after pruning, unwind needed"
                    );
                    return Ok(Some(mdbx_block));
                }
            }
            (Some((_mdbx_tx, mdbx_block)), _) => {
                // MDBX and static files are in sync (or static files don't have more data).
                // Check if MDBX is behind checkpoint.
                if checkpoint > mdbx_block {
                    tracing::warn!(
                        target: "reth::providers::rocksdb",
                        mdbx_block,
                        checkpoint,
                        "MDBX behind checkpoint, unwind needed"
                    );
                    return Ok(Some(mdbx_block));
                }
            }
            (None, Some(highest_tx)) => {
                // MDBX has no transactions but static files have data.
                // This means RocksDB might have stale entries - prune them all.
                tracing::info!(
                    target: "reth::providers::rocksdb",
                    highest_static_tx = highest_tx,
                    "MDBX empty but static files have data, pruning all TransactionHashNumbers"
                );
                self.prune_transaction_hash_numbers_in_range(provider, 0..=highest_tx)?;
            }
            (None, None) => {
                // Both MDBX and static files are empty, nothing to check.
            }
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

    /// Heals the `StoragesHistory` table by removing stale entries.
    ///
    /// Removes entries that were written after the stage checkpoint (stale data from
    /// a crash during indexing). Uses changesets to identify affected keys.
    ///
    /// - Fast path: if checkpoint == 0 AND `RocksDB` has data, clears everything
    /// - If `sf_tip` > checkpoint, iterates changesets in batches and unwinds affected keys
    fn heal_storages_history<Provider>(&self, provider: &Provider) -> ProviderResult<()>
    where
        Provider:
            DBProvider + StageCheckpointReader + StaticFileProviderFactory + StorageChangeSetReader,
    {
        let checkpoint = provider
            .get_stage_checkpoint(StageId::IndexStorageHistory)?
            .map(|cp| cp.block_number)
            .unwrap_or(0);

        // Fast path: if checkpoint is 0 and RocksDB has data, clear everything.
        // Short-circuit: only check has_data when checkpoint == 0 (rare case).
        if checkpoint == 0 && self.first::<tables::StoragesHistory>()?.is_some() {
            tracing::info!(
                target: "reth::providers::rocksdb",
                "StoragesHistory has data but checkpoint is 0, clearing all"
            );
            self.clear::<tables::StoragesHistory>()?;
            return Ok(());
        }

        let sf_tip = provider
            .static_file_provider()
            .get_highest_static_file_block(StaticFileSegment::StorageChangeSets)
            .unwrap_or(0);

        if sf_tip <= checkpoint {
            return Ok(());
        }

        tracing::info!(
            target: "reth::providers::rocksdb",
            checkpoint,
            sf_tip,
            "StoragesHistory: healing via changesets"
        );

        const BATCH_SIZE: u64 = 10_000;
        let mut batch_start = checkpoint.saturating_add(1);

        while batch_start <= sf_tip {
            let batch_end = batch_start.saturating_add(BATCH_SIZE - 1).min(sf_tip);

            let changesets = provider.storage_changesets_range(batch_start..=batch_end)?;

            if !changesets.is_empty() {
                let unique_keys: HashSet<_> = changesets
                    .into_iter()
                    .map(|(block_addr, entry)| (block_addr.address(), entry.key, checkpoint))
                    .collect();

                let indices: Vec<_> = unique_keys.into_iter().collect();

                tracing::debug!(
                    target: "reth::providers::rocksdb",
                    batch_start,
                    batch_end,
                    indices_count = indices.len(),
                    "Unwinding storage history indices batch"
                );

                let batch = self.unwind_storage_history_indices(&indices)?;
                self.commit_batch(batch)?;
            }

            batch_start = batch_end.saturating_add(1);
        }

        Ok(())
    }

    /// Heals the `AccountsHistory` table by removing stale entries.
    ///
    /// Removes entries that were written after the stage checkpoint (stale data from
    /// a crash during indexing). Uses changesets to identify affected keys.
    ///
    /// - Fast path: if checkpoint == 0 AND `RocksDB` has data, clears everything
    /// - If `sf_tip` > checkpoint, iterates changesets in batches and unwinds affected keys
    fn heal_accounts_history<Provider>(&self, provider: &Provider) -> ProviderResult<()>
    where
        Provider: DBProvider + StageCheckpointReader + StaticFileProviderFactory + ChangeSetReader,
    {
        const BATCH_SIZE: u64 = 10_000;

        let checkpoint = provider
            .get_stage_checkpoint(StageId::IndexAccountHistory)?
            .map(|cp| cp.block_number)
            .unwrap_or(0);

        // Fast path: if checkpoint is 0 and RocksDB has data, clear everything.
        // Short-circuit: only check has_data when checkpoint == 0 (rare case).
        if checkpoint == 0 && self.first::<tables::AccountsHistory>()?.is_some() {
            tracing::info!(
                target: "reth::providers::rocksdb",
                "AccountsHistory has data but checkpoint is 0, clearing all"
            );
            self.clear::<tables::AccountsHistory>()?;
            return Ok(());
        }

        let sf_tip = provider
            .static_file_provider()
            .get_highest_static_file_block(StaticFileSegment::AccountChangeSets);

        let Some(sf_tip) = sf_tip else {
            return Ok(());
        };

        if sf_tip <= checkpoint {
            return Ok(());
        }

        tracing::info!(
            target: "reth::providers::rocksdb",
            sf_tip,
            checkpoint,
            "AccountsHistory ahead of checkpoint, healing via changesets"
        );

        let mut batch_start = checkpoint + 1;
        while batch_start <= sf_tip {
            let batch_end = (batch_start + BATCH_SIZE - 1).min(sf_tip);

            let changesets = provider.account_changesets_range(batch_start..=batch_end)?;

            if !changesets.is_empty() {
                let mut addresses = HashSet::with_capacity(changesets.len());
                addresses.extend(changesets.iter().map(|(_, cs)| cs.address));

                let unwind_from = checkpoint + 1;
                let indices: Vec<_> =
                    addresses.into_iter().map(|addr| (addr, unwind_from)).collect();

                let batch = self.unwind_account_history_indices(&indices)?;
                self.commit_batch(batch)?;
            }

            batch_start = batch_end + 1;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        providers::{rocksdb::RocksDBBuilder, static_file::StaticFileWriter},
        test_utils::create_test_provider_factory,
        BlockWriter, DatabaseProviderFactory, StageCheckpointWriter, TransactionsProvider,
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
        // This is treated as a first-run/migration scenario - no unwind needed.
        let result = rocksdb.check_consistency(&provider).unwrap();
        assert_eq!(result, None, "Empty data with checkpoint is treated as first run");
    }

    #[test]
    fn test_check_consistency_mdbx_empty_static_files_have_data_prunes_rocksdb() {
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

        // Simulate crash recovery: MDBX was reset but static files and RocksDB still have data.
        // Clear TransactionBlocks to simulate empty MDBX state.
        {
            let provider = factory.database_provider_rw().unwrap();
            let mut cursor = provider.tx_ref().cursor_write::<tables::TransactionBlocks>().unwrap();
            let mut to_delete = Vec::new();
            let mut walker = cursor.walk(Some(0)).unwrap();
            while let Some((tx_num, _)) = walker.next().transpose().unwrap() {
                to_delete.push(tx_num);
            }
            drop(walker);
            for tx_num in to_delete {
                cursor.seek_exact(tx_num).unwrap();
                cursor.delete_current().unwrap();
            }
            // No checkpoint set (checkpoint = 0)
            provider.commit().unwrap();
        }

        // Verify RocksDB data exists
        assert!(rocksdb.last::<tables::TransactionHashNumbers>().unwrap().is_some());

        let provider = factory.database_provider_ro().unwrap();

        // MDBX TransactionBlocks is empty, but static files have transaction data.
        // This means RocksDB has stale data that should be pruned (healed).
        let result = rocksdb.check_consistency(&provider).unwrap();
        assert_eq!(result, None, "Should heal by pruning, no unwind needed");

        // Verify data was pruned
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
    fn test_check_consistency_mdbx_behind_checkpoint_needs_unwind() {
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

        // Now simulate a scenario where checkpoint is ahead of MDBX.
        // This happens when the checkpoint was saved but MDBX data was lost/corrupted.
        // Set checkpoint to block 10 (beyond our actual data at block 2)
        {
            let provider = factory.database_provider_rw().unwrap();
            provider
                .save_stage_checkpoint(StageId::TransactionLookup, StageCheckpoint::new(10))
                .unwrap();
            provider.commit().unwrap();
        }

        let provider = factory.database_provider_ro().unwrap();

        // MDBX has data up to block 2, but checkpoint says block 10 was processed.
        // The static files highest tx matches MDBX last tx (both at block 2).
        // Checkpoint > mdbx_block means we need to unwind to rebuild.
        let result = rocksdb.check_consistency(&provider).unwrap();
        assert_eq!(
            result,
            Some(2),
            "Should require unwind to block 2 (MDBX's last block) to rebuild from checkpoint"
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
    fn test_check_consistency_accounts_history_sf_tip_equals_checkpoint_no_action() {
        use reth_db::models::AccountBeforeTx;
        use reth_db_api::models::ShardedKey;
        use reth_static_file_types::StaticFileSegment;

        let temp_dir = TempDir::new().unwrap();
        let rocksdb = RocksDBBuilder::new(temp_dir.path())
            .with_table::<tables::AccountsHistory>()
            .build()
            .unwrap();

        // Insert some AccountsHistory entries with various highest_block_numbers
        let key1 = ShardedKey::new(Address::ZERO, 50);
        let key2 = ShardedKey::new(Address::random(), 75);
        let key3 = ShardedKey::new(Address::random(), u64::MAX); // sentinel
        let block_list1 = BlockNumberList::new_pre_sorted([10, 20, 30, 50]);
        let block_list2 = BlockNumberList::new_pre_sorted([40, 60, 75]);
        let block_list3 = BlockNumberList::new_pre_sorted([80, 90, 100]);
        rocksdb.put::<tables::AccountsHistory>(key1, &block_list1).unwrap();
        rocksdb.put::<tables::AccountsHistory>(key2, &block_list2).unwrap();
        rocksdb.put::<tables::AccountsHistory>(key3, &block_list3).unwrap();

        // Capture RocksDB state before consistency check
        let entries_before: Vec<_> =
            rocksdb.iter::<tables::AccountsHistory>().unwrap().map(|r| r.unwrap()).collect();
        assert_eq!(entries_before.len(), 3, "Should have 3 entries before check");

        // Create a test provider factory for MDBX
        let factory = create_test_provider_factory();
        factory.set_storage_settings_cache(
            StorageSettings::legacy().with_account_history_in_rocksdb(true),
        );

        // Write account changesets to static files for blocks 0-100
        {
            let sf_provider = factory.static_file_provider();
            let mut writer =
                sf_provider.latest_writer(StaticFileSegment::AccountChangeSets).unwrap();

            for block_num in 0..=100 {
                let changeset = vec![AccountBeforeTx { address: Address::random(), info: None }];
                writer.append_account_changeset(changeset, block_num).unwrap();
            }

            writer.commit().unwrap();
        }

        // Set IndexAccountHistory checkpoint to block 100 (same as sf_tip)
        {
            let provider = factory.database_provider_rw().unwrap();
            provider
                .save_stage_checkpoint(StageId::IndexAccountHistory, StageCheckpoint::new(100))
                .unwrap();
            provider.commit().unwrap();
        }

        let provider = factory.database_provider_ro().unwrap();

        // Verify sf_tip equals checkpoint (both at 100)
        let sf_tip = provider
            .static_file_provider()
            .get_highest_static_file_block(StaticFileSegment::AccountChangeSets)
            .unwrap();
        assert_eq!(sf_tip, 100, "Static file tip should be 100");

        // Run check_consistency - should return None (no unwind needed)
        let result = rocksdb.check_consistency(&provider).unwrap();
        assert_eq!(result, None, "sf_tip == checkpoint should not require unwind");

        // Verify NO entries are deleted - RocksDB state unchanged
        let entries_after: Vec<_> =
            rocksdb.iter::<tables::AccountsHistory>().unwrap().map(|r| r.unwrap()).collect();

        assert_eq!(
            entries_after.len(),
            entries_before.len(),
            "RocksDB entry count should be unchanged when sf_tip == checkpoint"
        );

        // Verify exact entries are preserved
        for (before, after) in entries_before.iter().zip(entries_after.iter()) {
            assert_eq!(before.0.key, after.0.key, "Entry key should be unchanged");
            assert_eq!(
                before.0.highest_block_number, after.0.highest_block_number,
                "Entry highest_block_number should be unchanged"
            );
            assert_eq!(before.1, after.1, "Entry block list should be unchanged");
        }
    }

    /// Tests StoragesHistory changeset-based healing with enough blocks to trigger batching.
    ///
    /// Scenario:
    /// 1. Generate 15,000 blocks worth of storage changeset data (to exceed the 10k batch size)
    /// 2. Each block has 1 storage change (address + slot + value)
    /// 3. Write storage changesets to static files for all 15k blocks
    /// 4. Set IndexStorageHistory checkpoint to block 5000
    /// 5. Insert stale StoragesHistory entries in RocksDB for (address, slot) pairs that changed in
    ///    blocks 5001-15000
    /// 6. Run check_consistency
    /// 7. Verify stale entries for blocks > 5000 are pruned and batching worked
    #[test]
    fn test_check_consistency_storages_history_heals_via_changesets_large_range() {
        use alloy_primitives::U256;
        use reth_db_api::models::StorageBeforeTx;

        const TOTAL_BLOCKS: u64 = 15_000;
        const CHECKPOINT_BLOCK: u64 = 5_000;

        let temp_dir = TempDir::new().unwrap();
        let rocksdb = RocksDBBuilder::new(temp_dir.path())
            .with_table::<tables::StoragesHistory>()
            .build()
            .unwrap();

        let factory = create_test_provider_factory();
        factory.set_storage_settings_cache(
            StorageSettings::legacy()
                .with_storages_history_in_rocksdb(true)
                .with_storage_changesets_in_static_files(true),
        );

        // Helper to generate address from block number (reuses stack arrays)
        #[inline]
        fn make_address(block_num: u64) -> Address {
            let mut addr_bytes = [0u8; 20];
            addr_bytes[0..8].copy_from_slice(&block_num.to_le_bytes());
            Address::from(addr_bytes)
        }

        // Helper to generate slot from block number (reuses stack arrays)
        #[inline]
        fn make_slot(block_num: u64) -> B256 {
            let mut slot_bytes = [0u8; 32];
            slot_bytes[0..8].copy_from_slice(&block_num.to_le_bytes());
            B256::from(slot_bytes)
        }

        // Write storage changesets to static files for 15k blocks.
        // Each block has 1 storage change with a unique (address, slot) pair.
        {
            let sf_provider = factory.static_file_provider();
            let mut writer =
                sf_provider.latest_writer(StaticFileSegment::StorageChangeSets).unwrap();

            // Reuse changeset vec to avoid repeated allocations
            let mut changeset = Vec::with_capacity(1);

            for block_num in 0..TOTAL_BLOCKS {
                changeset.clear();
                changeset.push(StorageBeforeTx {
                    address: make_address(block_num),
                    key: make_slot(block_num),
                    value: U256::from(block_num),
                });

                writer.append_storage_changeset(changeset.clone(), block_num).unwrap();
            }

            writer.commit().unwrap();
        }

        // Verify static files have data up to block 14999
        {
            let sf_provider = factory.static_file_provider();
            let highest = sf_provider
                .get_highest_static_file_block(StaticFileSegment::StorageChangeSets)
                .unwrap();
            assert_eq!(highest, TOTAL_BLOCKS - 1, "Static files should have blocks 0..14999");
        }

        // Set IndexStorageHistory checkpoint to block 5000
        {
            let provider = factory.database_provider_rw().unwrap();
            provider
                .save_stage_checkpoint(
                    StageId::IndexStorageHistory,
                    StageCheckpoint::new(CHECKPOINT_BLOCK),
                )
                .unwrap();
            provider.commit().unwrap();
        }

        // Insert stale StoragesHistory entries for blocks 5001-14999
        // These are (address, slot) pairs that changed after the checkpoint
        for block_num in (CHECKPOINT_BLOCK + 1)..TOTAL_BLOCKS {
            let key =
                StorageShardedKey::new(make_address(block_num), make_slot(block_num), block_num);
            let block_list = BlockNumberList::new_pre_sorted([block_num]);
            rocksdb.put::<tables::StoragesHistory>(key, &block_list).unwrap();
        }

        // Verify RocksDB has stale entries before healing
        let count_before: usize = rocksdb.iter::<tables::StoragesHistory>().unwrap().count();
        assert_eq!(
            count_before,
            (TOTAL_BLOCKS - CHECKPOINT_BLOCK - 1) as usize,
            "Should have {} stale entries before healing",
            TOTAL_BLOCKS - CHECKPOINT_BLOCK - 1
        );

        // Run check_consistency - this should heal by pruning stale entries
        let provider = factory.database_provider_ro().unwrap();
        let result = rocksdb.check_consistency(&provider).unwrap();
        assert_eq!(result, None, "Should heal via changesets, no unwind needed");

        // Verify all stale entries were pruned
        // After healing, entries with highest_block_number > checkpoint should be gone
        let mut remaining_stale = 0;
        for result in rocksdb.iter::<tables::StoragesHistory>().unwrap() {
            let (key, _) = result.unwrap();
            if key.sharded_key.highest_block_number > CHECKPOINT_BLOCK {
                remaining_stale += 1;
            }
        }
        assert_eq!(
            remaining_stale, 0,
            "All stale entries (block > {}) should be pruned",
            CHECKPOINT_BLOCK
        );
    }

    /// Tests AccountsHistory changeset-based healing with enough blocks to trigger batching.
    ///
    /// Scenario:
    /// 1. Generate 15,000 blocks worth of account changeset data (to exceed the 10k batch size)
    /// 2. Each block has 1 account change (simple - just random addresses)
    /// 3. Write account changesets to static files for all 15k blocks
    /// 4. Set IndexAccountHistory checkpoint to block 5000
    /// 5. Insert stale AccountsHistory entries in RocksDB for addresses that changed in blocks
    ///    5001-15000
    /// 6. Run check_consistency
    /// 7. Verify:
    ///    - Stale entries for blocks > 5000 are pruned
    ///    - The batching worked (no OOM, completed successfully)
    #[test]
    fn test_check_consistency_accounts_history_heals_via_changesets_large_range() {
        use reth_db::models::AccountBeforeTx;
        use reth_db_api::models::ShardedKey;
        use reth_static_file_types::StaticFileSegment;

        let temp_dir = TempDir::new().unwrap();
        let rocksdb = RocksDBBuilder::new(temp_dir.path())
            .with_table::<tables::AccountsHistory>()
            .build()
            .unwrap();

        // Create test provider factory
        let factory = create_test_provider_factory();
        factory.set_storage_settings_cache(
            StorageSettings::legacy()
                .with_account_history_in_rocksdb(true)
                .with_account_changesets_in_static_files(true),
        );

        const TOTAL_BLOCKS: u64 = 15_000;
        const CHECKPOINT_BLOCK: u64 = 5_000;

        // Helper to generate address from block number (avoids pre-allocating 15k addresses)
        #[inline]
        fn make_address(block_num: u64) -> Address {
            let mut addr = Address::ZERO;
            addr.0[0..8].copy_from_slice(&block_num.to_le_bytes());
            addr
        }

        // Write account changesets to static files for all 15k blocks
        {
            let sf_provider = factory.static_file_provider();
            let mut writer =
                sf_provider.latest_writer(StaticFileSegment::AccountChangeSets).unwrap();

            // Reuse changeset vec to avoid repeated allocations
            let mut changeset = Vec::with_capacity(1);

            for block_num in 0..TOTAL_BLOCKS {
                changeset.clear();
                changeset.push(AccountBeforeTx { address: make_address(block_num), info: None });
                writer.append_account_changeset(changeset.clone(), block_num).unwrap();
            }

            writer.commit().unwrap();
        }

        // Insert stale AccountsHistory entries in RocksDB for addresses that changed
        // in blocks 5001-15000 (i.e., blocks after the checkpoint)
        // These should be pruned by check_consistency
        for block_num in (CHECKPOINT_BLOCK + 1)..TOTAL_BLOCKS {
            let key = ShardedKey::new(make_address(block_num), block_num);
            let block_list = BlockNumberList::new_pre_sorted([block_num]);
            rocksdb.put::<tables::AccountsHistory>(key, &block_list).unwrap();
        }

        // Also insert some valid entries for blocks <= 5000 that should NOT be pruned
        for block_num in [100u64, 500, 1000, 2500, 5000] {
            let key = ShardedKey::new(make_address(block_num), block_num);
            let block_list = BlockNumberList::new_pre_sorted([block_num]);
            rocksdb.put::<tables::AccountsHistory>(key, &block_list).unwrap();
        }

        // Verify we have entries before healing
        let entries_before: usize = rocksdb.iter::<tables::AccountsHistory>().unwrap().count();
        let stale_count = (TOTAL_BLOCKS - CHECKPOINT_BLOCK - 1) as usize;
        let valid_count = 5usize;
        assert_eq!(
            entries_before,
            stale_count + valid_count,
            "Should have {} stale + {} valid entries before healing",
            stale_count,
            valid_count
        );

        // Set IndexAccountHistory checkpoint to block 5000
        {
            let provider = factory.database_provider_rw().unwrap();
            provider
                .save_stage_checkpoint(
                    StageId::IndexAccountHistory,
                    StageCheckpoint::new(CHECKPOINT_BLOCK),
                )
                .unwrap();
            provider.commit().unwrap();
        }

        let provider = factory.database_provider_ro().unwrap();

        // Verify sf_tip > checkpoint
        let sf_tip = provider
            .static_file_provider()
            .get_highest_static_file_block(StaticFileSegment::AccountChangeSets)
            .unwrap();
        assert_eq!(sf_tip, TOTAL_BLOCKS - 1, "Static file tip should be 14999");
        assert!(sf_tip > CHECKPOINT_BLOCK, "sf_tip should be > checkpoint to trigger healing");

        // Run check_consistency - this should trigger batched changeset-based healing
        let result = rocksdb.check_consistency(&provider).unwrap();
        assert_eq!(result, None, "Healing should succeed without requiring unwind");

        // Verify: all stale entries for blocks > 5000 should be pruned
        // Count remaining entries with highest_block_number > checkpoint
        let mut remaining_stale = 0;
        for result in rocksdb.iter::<tables::AccountsHistory>().unwrap() {
            let (key, _) = result.unwrap();
            if key.highest_block_number > CHECKPOINT_BLOCK && key.highest_block_number != u64::MAX {
                remaining_stale += 1;
            }
        }
        assert_eq!(
            remaining_stale, 0,
            "All stale entries (block > {}) should be pruned",
            CHECKPOINT_BLOCK
        );
    }

    #[test]
    fn test_check_consistency_storages_history_sf_tip_equals_checkpoint_no_action() {
        use alloy_primitives::U256;
        use reth_db::models::StorageBeforeTx;
        use reth_static_file_types::StaticFileSegment;

        let temp_dir = TempDir::new().unwrap();
        let rocksdb = RocksDBBuilder::new(temp_dir.path())
            .with_table::<tables::StoragesHistory>()
            .build()
            .unwrap();

        // Insert StoragesHistory entries into RocksDB
        let key1 = StorageShardedKey::new(Address::ZERO, B256::ZERO, 50);
        let key2 = StorageShardedKey::new(Address::random(), B256::random(), 80);
        let block_list1 = BlockNumberList::new_pre_sorted([10, 20, 30, 50]);
        let block_list2 = BlockNumberList::new_pre_sorted([40, 60, 80]);
        rocksdb.put::<tables::StoragesHistory>(key1.clone(), &block_list1).unwrap();
        rocksdb.put::<tables::StoragesHistory>(key2.clone(), &block_list2).unwrap();

        // Capture entries before consistency check
        let entries_before: Vec<_> =
            rocksdb.iter::<tables::StoragesHistory>().unwrap().map(|r| r.unwrap()).collect();

        // Create a test provider factory
        let factory = create_test_provider_factory();
        factory.set_storage_settings_cache(
            StorageSettings::legacy().with_storages_history_in_rocksdb(true),
        );

        // Write storage changesets to static files for blocks 0-100
        {
            let sf_provider = factory.static_file_provider();
            let mut writer =
                sf_provider.latest_writer(StaticFileSegment::StorageChangeSets).unwrap();

            for block_num in 0..=100u64 {
                let changeset = vec![StorageBeforeTx {
                    address: Address::ZERO,
                    key: B256::with_last_byte(block_num as u8),
                    value: U256::from(block_num),
                }];
                writer.append_storage_changeset(changeset, block_num).unwrap();
            }
            writer.commit().unwrap();
        }

        // Set IndexStorageHistory checkpoint to block 100 (same as sf_tip)
        {
            let provider = factory.database_provider_rw().unwrap();
            provider
                .save_stage_checkpoint(StageId::IndexStorageHistory, StageCheckpoint::new(100))
                .unwrap();
            provider.commit().unwrap();
        }

        let provider = factory.database_provider_ro().unwrap();

        // Verify sf_tip equals checkpoint (both at 100)
        let sf_tip = provider
            .static_file_provider()
            .get_highest_static_file_block(StaticFileSegment::StorageChangeSets)
            .unwrap();
        assert_eq!(sf_tip, 100, "Static file tip should be 100");

        // Run check_consistency - should return None (no unwind needed)
        let result = rocksdb.check_consistency(&provider).unwrap();
        assert_eq!(result, None, "sf_tip == checkpoint should not require unwind");

        // Verify NO entries are deleted - RocksDB state unchanged
        let entries_after: Vec<_> =
            rocksdb.iter::<tables::StoragesHistory>().unwrap().map(|r| r.unwrap()).collect();

        assert_eq!(
            entries_after.len(),
            entries_before.len(),
            "RocksDB entry count should be unchanged when sf_tip == checkpoint"
        );

        // Verify exact entries are preserved
        for (before, after) in entries_before.iter().zip(entries_after.iter()) {
            assert_eq!(before.0, after.0, "Entry key should be unchanged");
            assert_eq!(before.1, after.1, "Entry block list should be unchanged");
        }
    }
}
