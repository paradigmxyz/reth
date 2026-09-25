//! `reth db migrate-v2` command for migrating v1 storage layout to v2.

use crate::common::CliNodeTypes;
use alloy_primitives::Address;
use clap::Parser;
use reth_db::{
    mdbx::{self, ffi},
    models::StorageBeforeTx,
    DatabaseEnv,
};
use reth_db_api::{
    cursor::DbCursorRO,
    database::Database,
    table::Table,
    tables,
    transaction::{DbTx, DbTxMut},
};
use reth_node_builder::NodeTypesWithDBAdapter;
use reth_provider::{
    providers::ProviderNodeTypes, BlockNumReader, DBProvider, DatabaseProviderFactory,
    MetadataProvider, MetadataWriter, ProviderFactory, PruneCheckpointReader,
    RocksDBProviderFactory, StageCheckpointWriter, StaticFileProviderFactory, StaticFileWriter,
    StorageSettings,
};
use reth_prune_types::PruneSegment;
use reth_stages_types::{StageCheckpoint, StageId};
use reth_static_file_types::StaticFileSegment;
use reth_storage_api::StageCheckpointReader;
use tracing::{info, warn};

/// `reth db migrate-v2` command
#[derive(Debug, Parser)]
pub struct Command;

impl Command {
    /// Execute the full v1 → v2 migration:
    ///
    /// 1. Migrate changesets + receipts to static files
    /// 2. Migrate transaction senders to static files (when the table is complete)
    /// 3. Flip `StorageSettings` to v2
    /// 4. Clear recomputable MDBX tables + reset stage checkpoints
    /// 5. Compact MDBX
    pub async fn execute<N: CliNodeTypes>(
        self,
        provider_factory: ProviderFactory<NodeTypesWithDBAdapter<N, DatabaseEnv>>,
    ) -> eyre::Result<()>
    where
        N::Primitives: reth_primitives_traits::NodePrimitives<
            Receipt: reth_db_api::table::Value + reth_codecs::Compact,
        >,
    {
        // === Phase 0: Preflight ===
        info!(target: "reth::cli", "Starting v1 → v2 storage migration");

        let provider = provider_factory.provider()?;
        let current_settings = provider.storage_settings()?;

        if current_settings.is_some_and(|s| s.is_v2()) {
            info!(target: "reth::cli", "Storage is already v2, nothing to do");
            return Ok(());
        }

        let tip =
            provider.get_stage_checkpoint(StageId::Execution)?.map(|c| c.block_number).unwrap_or(0);

        info!(target: "reth::cli", tip, "Chain tip block number");

        let sf_provider = provider_factory.static_file_provider();

        for segment in [
            StaticFileSegment::AccountChangeSets,
            StaticFileSegment::StorageChangeSets,
            StaticFileSegment::TransactionSenders,
        ] {
            if sf_provider.get_highest_static_file_block(segment).is_some() {
                eyre::bail!(
                    "Static file segment {segment:?} already contains data. \
                     Cannot migrate — target must be empty."
                );
            }
        }

        drop(provider);

        // === Phase 1: Migrate changesets → static files ===
        Self::migrate_account_changesets(&provider_factory, tip)?;
        Self::migrate_storage_changesets(&provider_factory, tip)?;

        // === Phase 2: Migrate receipts → static files ===
        Self::migrate_receipts::<NodeTypesWithDBAdapter<N, DatabaseEnv>>(&provider_factory, tip)?;

        // === Phase 3: Migrate transaction senders → static files ===
        //
        // Senders are normally recomputed by SenderRecovery, but not all senders are
        // recoverable: OP mainnet pre-Bedrock OVM transactions (e.g. no-signature queue
        // transactions) got their senders from `import-op` and cannot be re-derived. When the
        // table holds one sender per transaction, move it instead of clearing it.
        let senders_migrated = Self::migrate_transaction_senders(&provider_factory, tip)?;

        // === Phase 4: Migrate indices → RocksDB ===
        Self::migrate_to_rocksdb::<_, tables::TransactionHashNumbers>(&provider_factory)?;
        Self::migrate_to_rocksdb::<_, tables::AccountsHistory>(&provider_factory)?;
        Self::migrate_to_rocksdb::<_, tables::StoragesHistory>(&provider_factory)?;

        // === Phase 5: Flip metadata to v2 ===
        info!(target: "reth::cli", "Writing StorageSettings v2 metadata");
        {
            let provider_rw = provider_factory.database_provider_rw()?;
            provider_rw.write_storage_settings(StorageSettings::v2())?;
            provider_rw.commit()?;
        }
        info!(target: "reth::cli", "Storage settings updated to v2");

        // === Phase 6: Clear migrated and recomputable MDBX tables ===
        Self::clear_migrated_and_recomputable_tables(&provider_factory, senders_migrated)?;

        // === Phase 7: Compact MDBX (before pipeline, so it runs on a smaller DB) ===
        let db_path = provider_factory.db_ref().path();
        Self::compact_mdbx(provider_factory.db_ref())?;

        // Drop to release DB handle for swap
        drop(provider_factory);

        let compact_path = db_path.with_file_name("db_compact");
        Self::swap_compacted_db(&db_path, &compact_path)?;

        // === Phase 8: Reopen DB and run pipeline ===
        // The caller will reopen the environment and run the pipeline.
        // We return here — the pipeline step is handled in mod.rs after
        // reopening the database with the compacted copy.
        info!(target: "reth::cli", "Migration complete. You should now restart the node and let it run the pipeline to rebuild the remaining data.");
        Ok(())
    }

    fn migrate_account_changesets<N: ProviderNodeTypes>(
        factory: &ProviderFactory<N>,
        tip: u64,
    ) -> eyre::Result<()> {
        info!(target: "reth::cli", "Migrating AccountChangeSets → static files");
        let provider = factory.provider()?.disable_long_read_transaction_safety();
        let sf_provider = factory.static_file_provider();

        let mut cursor = provider.tx_ref().cursor_read::<tables::AccountChangeSets>()?;

        let first_block = provider
            .get_prune_checkpoint(PruneSegment::AccountHistory)?
            .and_then(|cp| cp.block_number)
            .map_or(0, |b| b + 1);

        // The writer always starts at the fixed range boundary (e.g. 2500000) which may be
        // earlier than first_block (e.g. 2603897 from prune checkpoint).
        let mut writer = sf_provider.latest_writer(StaticFileSegment::AccountChangeSets)?;
        if first_block > 0 {
            writer.ensure_at_block(first_block - 1)?;
        }

        let mut count = 0u64;
        let mut walker = cursor.walk(Some(first_block))?.peekable();

        for block in first_block..=tip {
            let mut entries = Vec::new();

            while let Some(Ok((block_number, _))) = walker.peek() {
                if *block_number != block {
                    break;
                }
                let (_, entry) = walker.next().expect("peeked")?;
                entries.push(entry);
            }

            count += entries.len() as u64;
            writer.append_account_changeset(entries, block)?;
        }

        writer.commit()?;

        info!(target: "reth::cli", count, "AccountChangeSets migrated");
        Ok(())
    }

    fn migrate_storage_changesets<N: ProviderNodeTypes>(
        factory: &ProviderFactory<N>,
        tip: u64,
    ) -> eyre::Result<()> {
        info!(target: "reth::cli", "Migrating StorageChangeSets → static files");
        let provider = factory.provider()?.disable_long_read_transaction_safety();
        let sf_provider = factory.static_file_provider();

        let mut cursor = provider.tx_ref().cursor_read::<tables::StorageChangeSets>()?;

        let first_block = provider
            .get_prune_checkpoint(PruneSegment::StorageHistory)?
            .and_then(|cp| cp.block_number)
            .map_or(0, |b| b + 1);

        // The writer always starts at the fixed range boundary (e.g. 2500000) which may be
        // earlier than first_block (e.g. 2603897 from prune checkpoint).
        let mut writer = sf_provider.latest_writer(StaticFileSegment::StorageChangeSets)?;
        if first_block > 0 {
            writer.ensure_at_block(first_block - 1)?;
        }

        let mut count = 0u64;
        let mut walker = cursor.walk(Some((first_block, Address::ZERO).into()))?.peekable();

        for block in first_block..=tip {
            let mut entries = Vec::new();

            while let Some(Ok((key, _))) = walker.peek() {
                if key.block_number() != block {
                    break;
                }
                let (key, entry) = walker.next().expect("peeked")?;
                entries.push(StorageBeforeTx {
                    address: key.address(),
                    key: entry.key,
                    value: entry.value,
                });
            }

            count += entries.len() as u64;
            writer.append_storage_changeset(entries, block)?;
        }

        writer.commit()?;

        info!(target: "reth::cli", count, "StorageChangeSets migrated");
        Ok(())
    }

    fn migrate_receipts<N: ProviderNodeTypes>(
        factory: &ProviderFactory<N>,
        tip: u64,
    ) -> eyre::Result<()>
    where
        N::Primitives: reth_primitives_traits::NodePrimitives<
            Receipt: reth_db_api::table::Value + reth_codecs::Compact,
        >,
    {
        let provider = factory.provider()?;
        if !provider.prune_modes_ref().receipts_log_filter.is_empty() {
            info!(target: "reth::cli", "Receipt log filter pruning is enabled, keeping receipts in MDBX");
            return Ok(());
        }
        drop(provider);

        let sf_provider = factory.static_file_provider();
        let existing = sf_provider.get_highest_static_file_block(StaticFileSegment::Receipts);

        if existing.is_some_and(|b| b >= tip) {
            info!(target: "reth::cli", "Receipts already in static files, skipping");
            return Ok(());
        }

        info!(target: "reth::cli", "Migrating Receipts → static files");

        let provider = factory.provider()?.disable_long_read_transaction_safety();
        let prune_start = provider
            .get_prune_checkpoint(PruneSegment::Receipts)?
            .and_then(|cp| cp.block_number)
            .map_or(0, |b| b + 1);
        let first_block = prune_start.max(existing.map_or(0, |b| b + 1));

        // The writer always starts at the fixed range boundary (e.g. 2500000) which may be
        // earlier than first_block (e.g. 2603897 from prune checkpoint).
        if first_block > 0 {
            let mut writer = sf_provider.latest_writer(StaticFileSegment::Receipts)?;
            writer.ensure_at_block(first_block - 1)?;
            writer.commit()?;
        }

        let before = sf_provider
            .get_highest_static_file_tx(StaticFileSegment::Receipts)
            .map_or(0, |tx| tx + 1);

        let block_range = first_block..=tip;

        let segment = reth_static_file::segments::Receipts;
        reth_static_file::segments::Segment::copy_to_static_files(&segment, provider, block_range)?;

        sf_provider.commit()?;

        let after = sf_provider
            .get_highest_static_file_tx(StaticFileSegment::Receipts)
            .map_or(0, |tx| tx + 1);
        let count = after - before;
        info!(target: "reth::cli", count, "Receipts migrated");
        Ok(())
    }

    /// Migrates the `TransactionSenders` MDBX table to static files, if it is complete.
    ///
    /// Senders are usually recomputed by the `SenderRecovery` stage after migration, but some
    /// chains contain transactions whose sender cannot be recovered from the signature (e.g. OP
    /// mainnet pre-Bedrock OVM transactions, imported with pre-computed senders via `import-op`).
    /// When the table holds one sender per transaction, moving it is both safer and cheaper than
    /// recomputing.
    ///
    /// Returns `true` if senders were migrated — the `SenderRecovery` checkpoint must then be
    /// preserved — and `false` if the table is incomplete (e.g. pruned or bootstrapped without
    /// pre-Bedrock bodies) and the stage should rebuild it.
    fn migrate_transaction_senders<N: ProviderNodeTypes>(
        factory: &ProviderFactory<N>,
        tip: u64,
    ) -> eyre::Result<bool> {
        let provider = factory.provider()?.disable_long_read_transaction_safety();
        let sf_provider = factory.static_file_provider();

        let total_txs = sf_provider
            .get_highest_static_file_tx(StaticFileSegment::Transactions)
            .map_or(0, |tx| tx + 1);
        let sender_entries = provider.tx_ref().entries::<tables::TransactionSenders>()? as u64;

        if total_txs == 0 || sender_entries != total_txs {
            info!(
                target: "reth::cli",
                sender_entries,
                total_txs,
                "TransactionSenders table is incomplete, senders will be rebuilt by SenderRecovery"
            );
            return Ok(false);
        }

        info!(target: "reth::cli", "Migrating TransactionSenders → static files");

        let mut writer = sf_provider.latest_writer(StaticFileSegment::TransactionSenders)?;

        let mut senders_cursor = provider.tx_ref().cursor_read::<tables::TransactionSenders>()?;
        let mut senders = senders_cursor.walk(None)?;
        let mut indices_cursor = provider.tx_ref().cursor_read::<tables::BlockBodyIndices>()?;

        let mut count = 0u64;
        for entry in indices_cursor.walk(None)? {
            let (block, indices) = entry?;
            if block > tip {
                break;
            }
            writer.ensure_at_block(block)?;
            for _ in 0..indices.tx_count {
                let (tx_num, sender) = senders.next().ok_or_else(|| {
                    eyre::eyre!("missing sender for transaction in block {block}")
                })??;
                if tx_num != count {
                    eyre::bail!(
                        "non-contiguous TransactionSenders table: expected tx {count}, got {tx_num}"
                    );
                }
                writer.append_transaction_sender(tx_num, &sender)?;
                count += 1;
            }
        }
        writer.ensure_at_block(tip)?;
        writer.commit()?;

        info!(target: "reth::cli", count, "TransactionSenders migrated");
        Ok(true)
    }

    fn migrate_to_rocksdb<N: ProviderNodeTypes, T: Table>(
        factory: &ProviderFactory<N>,
    ) -> eyre::Result<()> {
        info!(target: "reth::cli", table = T::NAME, "Migrating MDBX table → RocksDB");

        let provider = factory.provider()?.disable_long_read_transaction_safety();
        let mut cursor = provider.tx_ref().cursor_read::<T>()?;

        let rocksdb = factory.rocksdb_provider();
        rocksdb.clear::<T>()?;
        let mut batch = rocksdb.batch_with_auto_commit();

        let mut count = 0u64;
        for entry in cursor.walk(None)? {
            let (key, value) = entry?;
            batch.put::<T>(key, &value)?;
            count += 1;
        }

        batch.commit()?;
        rocksdb.flush(&[T::NAME])?;

        info!(target: "reth::cli", table = T::NAME, count, "MDBX table migrated to RocksDB");
        Ok(())
    }

    /// Clears MDBX tables that were migrated to v2 backends or can be recomputed by the pipeline,
    /// and resets only the recomputed stage checkpoints.
    fn clear_migrated_and_recomputable_tables<N: ProviderNodeTypes>(
        factory: &ProviderFactory<N>,
        senders_migrated: bool,
    ) -> eyre::Result<()> {
        info!(target: "reth::cli", "Clearing migrated and recomputable MDBX tables");
        let db = factory.db_ref();

        macro_rules! clear_table {
            ($table:ty) => {{
                let tx = db.tx_mut()?;
                tx.clear::<$table>()?;
                tx.commit()?;
                info!(target: "reth::cli", table = <$table as Table>::NAME, "Cleared");
            }};
        }

        // Migrated changeset tables (now in static files)
        clear_table!(tables::AccountChangeSets);
        clear_table!(tables::StorageChangeSets);

        // Senders — migrated to static files, or rebuilt by SenderRecovery
        clear_table!(tables::TransactionSenders);

        // Indices — migrated to RocksDB
        clear_table!(tables::TransactionHashNumbers);
        clear_table!(tables::AccountsHistory);
        clear_table!(tables::StoragesHistory);

        // Plain state — superseded by hashed state in v2
        clear_table!(tables::PlainAccountState);
        clear_table!(tables::PlainStorageState);

        // Trie — rebuilt by MerkleExecute
        clear_table!(tables::AccountsTrie);
        clear_table!(tables::StoragesTrie);

        // Reset stage checkpoints so the pipeline rebuilds everything. The SenderRecovery
        // checkpoint is preserved when senders were migrated to static files — nothing to
        // rebuild, and pre-Bedrock OVM senders could not be recovered anyway.
        info!(target: "reth::cli", "Resetting stage checkpoints");
        let provider_rw = factory.database_provider_rw()?;
        let mut stages_to_reset = vec![StageId::MerkleExecute, StageId::MerkleUnwind];
        if !senders_migrated {
            stages_to_reset.push(StageId::SenderRecovery);
        }
        for stage in stages_to_reset {
            provider_rw.save_stage_checkpoint(stage, StageCheckpoint::new(0))?;
            info!(target: "reth::cli", %stage, "Checkpoint reset to 0");
        }
        provider_rw.save_stage_checkpoint_progress(StageId::MerkleExecute, vec![])?;

        if !senders_migrated && provider_rw.last_block_number()? > 0 {
            let first_indices_entry = provider_rw
                .tx_ref()
                .cursor_read::<tables::BlockBodyIndices>()?
                .seek(1)?
                .map(|(block, _)| block)
                .ok_or_else(|| eyre::eyre!("no block body indices found"))?;

            // If the first block body indices entry is not block 1, it means that the v1 database
            // was likely initialized with dummy blocks coming from a dummy chain generated by
            // `setup_without_evm`.
            //
            // In that case, sender recovery starts from the first block that has a corresponding
            // block body indices entry.
            if first_indices_entry > 1 {
                provider_rw.save_stage_checkpoint(
                    StageId::SenderRecovery,
                    StageCheckpoint::new(first_indices_entry - 1),
                )?;

                // Make sure that senders static files segment is at the correct height.
                let static_file_provider = provider_rw.static_file_provider();
                let mut senders_writer =
                    static_file_provider.latest_writer(StaticFileSegment::TransactionSenders)?;
                senders_writer.ensure_at_block(first_indices_entry - 1)?;
                senders_writer.commit()?;

                warn!(
                    target: "reth::cli",
                    "Missing block body indices data for first {first_indices_entry} blocks, initializing sender recovery with the first block that has a corresponding block body indices entry"
                );
            }
        }
        provider_rw.commit()?;

        info!(target: "reth::cli", "Recomputable tables cleared");
        Ok(())
    }

    /// Creates a compacted copy of the MDBX database.
    fn compact_mdbx(db: &mdbx::DatabaseEnv) -> eyre::Result<()> {
        let db_path = db.path();
        let compact_path = db_path.with_file_name("db_compact");

        reth_fs_util::create_dir_all(&compact_path)?;

        info!(target: "reth::cli", ?db_path, ?compact_path, "Compacting MDBX database");

        let compact_dest = compact_path.join("mdbx.dat");
        let dest_cstr = std::ffi::CString::new(
            compact_dest.to_str().ok_or_else(|| eyre::eyre!("compact path must be valid UTF-8"))?,
        )?;

        let flags = ffi::MDBX_CP_COMPACT | ffi::MDBX_CP_FORCE_DYNAMIC_SIZE;

        let rc = db.with_raw_env_ptr(|env_ptr| unsafe {
            ffi::mdbx_env_copy(env_ptr, dest_cstr.as_ptr(), flags)
        });

        if rc != 0 {
            eyre::bail!("mdbx_env_copy failed with error code {rc}: {}", unsafe {
                std::ffi::CStr::from_ptr(ffi::mdbx_strerror(rc)).to_string_lossy()
            });
        }

        info!(target: "reth::cli", "MDBX compaction complete");
        Ok(())
    }

    /// Swaps the original MDBX database with a compacted copy.
    fn swap_compacted_db(
        db_path: &std::path::Path,
        compact_path: &std::path::Path,
    ) -> eyre::Result<()> {
        let backup_path = db_path.with_file_name("db_pre_compact");

        info!(target: "reth::cli", ?db_path, ?compact_path, "Swapping compacted database");

        std::fs::rename(db_path, &backup_path)?;

        if let Err(e) = std::fs::rename(compact_path, db_path) {
            let _ = std::fs::rename(&backup_path, db_path);
            return Err(e.into());
        }

        std::fs::remove_dir_all(&backup_path)?;

        info!(target: "reth::cli", "Database compaction swap complete");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reth_db_api::models::StoredBlockBodyIndices;
    use reth_provider::{
        test_utils::{create_test_provider_factory, MockNodeTypesWithDB},
        StorageSettingsCache, TransactionsProvider,
    };
    use reth_testing_utils::generators::{self, random_block_range, BlockRangeParams};

    const TIP: u64 = 19;

    fn expected_sender(tx_num: u64) -> Address {
        Address::with_last_byte((tx_num % 200) as u8 + 1)
    }

    /// Seeds a v1-layout chain: transactions in static files, block body indices and senders
    /// in MDBX. Returns the total transaction count.
    fn seed_v1_chain(factory: &ProviderFactory<MockNodeTypesWithDB>) -> u64 {
        let mut rng = generators::rng();
        let blocks = random_block_range(
            &mut rng,
            0..=TIP,
            BlockRangeParams { tx_count: 1..4, ..Default::default() },
        );

        let sf_provider = factory.static_file_provider();
        let provider_rw = factory.database_provider_rw().unwrap();
        let tx = provider_rw.tx_ref();

        let mut next_tx_num = 0u64;
        {
            // Scope the static-file writer so it is dropped (releasing its lock) before the MDBX
            // provider commits — otherwise the DB commit blocks on the still-held writer lock.
            let mut tx_writer = sf_provider.latest_writer(StaticFileSegment::Transactions).unwrap();
            for block in &blocks {
                tx.put::<tables::BlockBodyIndices>(
                    block.number,
                    StoredBlockBodyIndices {
                        first_tx_num: next_tx_num,
                        tx_count: block.transaction_count() as u64,
                    },
                )
                .unwrap();
                for body_tx in &block.body().transactions {
                    tx_writer.append_transaction(next_tx_num, body_tx).unwrap();
                    tx.put::<tables::TransactionSenders>(next_tx_num, expected_sender(next_tx_num))
                        .unwrap();
                    next_tx_num += 1;
                }
                tx_writer.increment_block(block.number).unwrap();
            }
            tx_writer.commit().unwrap();
        }
        provider_rw.commit().unwrap();
        next_tx_num
    }

    #[test]
    fn migrates_complete_senders_table() {
        let factory = create_test_provider_factory();
        let total_txs = seed_v1_chain(&factory);
        assert!(total_txs > 0);

        let migrated = Command::migrate_transaction_senders(&factory, TIP).unwrap();
        assert!(migrated);

        let sf_provider = factory.static_file_provider();
        assert_eq!(
            sf_provider.get_highest_static_file_tx(StaticFileSegment::TransactionSenders),
            Some(total_txs - 1)
        );
        assert_eq!(
            sf_provider.get_highest_static_file_block(StaticFileSegment::TransactionSenders),
            Some(TIP)
        );

        // Senders must read back from static files with the exact same values.
        factory.set_storage_settings_cache(StorageSettings::v2());
        let provider = factory.provider().unwrap();
        for tx_num in 0..total_txs {
            assert_eq!(
                provider.transaction_sender(tx_num).unwrap(),
                Some(expected_sender(tx_num)),
                "sender mismatch for tx {tx_num}"
            );
        }
    }

    #[test]
    fn incomplete_senders_table_falls_back() {
        let factory = create_test_provider_factory();
        let total_txs = seed_v1_chain(&factory);

        // Remove one sender to make the table incomplete.
        let provider_rw = factory.database_provider_rw().unwrap();
        provider_rw.tx_ref().delete::<tables::TransactionSenders>(total_txs - 1, None).unwrap();
        provider_rw.commit().unwrap();

        let migrated = Command::migrate_transaction_senders(&factory, TIP).unwrap();
        assert!(!migrated);
        assert!(factory
            .static_file_provider()
            .get_highest_static_file_block(StaticFileSegment::TransactionSenders)
            .is_none());
    }

    #[test]
    fn clear_preserves_sender_recovery_checkpoint_when_migrated() {
        let factory = create_test_provider_factory();
        seed_v1_chain(&factory);

        let provider_rw = factory.database_provider_rw().unwrap();
        provider_rw
            .save_stage_checkpoint(StageId::SenderRecovery, StageCheckpoint::new(TIP))
            .unwrap();
        provider_rw.commit().unwrap();

        Command::clear_migrated_and_recomputable_tables(&factory, true).unwrap();

        let provider = factory.provider().unwrap();
        assert_eq!(
            provider.get_stage_checkpoint(StageId::SenderRecovery).unwrap(),
            Some(StageCheckpoint::new(TIP))
        );
        // The MDBX table is cleared either way once senders live in static files.
        assert_eq!(provider.tx_ref().entries::<tables::TransactionSenders>().unwrap(), 0);
    }

    #[test]
    fn clear_resets_sender_recovery_checkpoint_when_not_migrated() {
        let factory = create_test_provider_factory();
        seed_v1_chain(&factory);

        let provider_rw = factory.database_provider_rw().unwrap();
        provider_rw
            .save_stage_checkpoint(StageId::SenderRecovery, StageCheckpoint::new(TIP))
            .unwrap();
        provider_rw.commit().unwrap();

        Command::clear_migrated_and_recomputable_tables(&factory, false).unwrap();

        let provider = factory.provider().unwrap();
        assert_eq!(
            provider.get_stage_checkpoint(StageId::SenderRecovery).unwrap(),
            Some(StageCheckpoint::new(0))
        );
    }
}
