//! Storage migration command for Reth.
//!
//! Migrates data from legacy MDBX storage to RocksDB + static files.

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use alloy_primitives::{Address, BlockNumber};
use clap::Parser;
use eyre::Result;
use rayon::prelude::*;
use reth_db::{cursor::DbCursorRO, tables, transaction::DbTx, DatabaseEnv};
use reth_db_api::models::{AccountBeforeTx, StorageBeforeTx};
use reth_provider::{
    BlockBodyIndicesProvider, BlockNumReader, DBProvider, MetadataWriter, ProviderFactory,
    StaticFileProviderFactory, StaticFileWriter, TransactionsProvider,
};
use reth_static_file_types::StaticFileSegment;
use tracing::{info, warn};

use crate::common::CliNodeTypes;

/// Progress logging interval.
const PROGRESS_LOG_INTERVAL: Duration = Duration::from_secs(10);

/// Migrate from legacy MDBX storage to new RocksDB + static files.
#[derive(Debug, Parser)]
pub struct Command {
    /// Block batch size for processing.
    #[arg(long, default_value = "10000")]
    batch_size: u64,

    /// Skip static file migration.
    #[arg(long)]
    skip_static_files: bool,

    /// Skip RocksDB migration.
    #[arg(long)]
    skip_rocksdb: bool,

    /// Keep migrated MDBX tables (don't drop them after migration).
    #[arg(long)]
    keep_mdbx: bool,
}

impl Command {
    /// Execute the migration command.
    pub fn execute<N: CliNodeTypes>(
        self,
        provider_factory: ProviderFactory<
            reth_node_builder::NodeTypesWithDBAdapter<N, Arc<DatabaseEnv>>,
        >,
    ) -> Result<()> {
        info!(target: "reth::cli", "Starting storage migration from legacy MDBX to new storage");

        let provider = provider_factory.provider()?.disable_long_read_transaction_safety();
        let to_block = provider.best_block_number()?;
        let prune_modes = provider.prune_modes_ref().clone();
        drop(provider);

        info!(
            target: "reth::cli",
            to_block,
            batch_size = self.batch_size,
            "Migration parameters"
        );

        // Check if receipts can be migrated (no contract log pruning)
        let can_migrate_receipts = prune_modes.receipts_log_filter.is_empty();
        if !can_migrate_receipts {
            warn!(target: "reth::cli", "Receipts will NOT be migrated due to contract log pruning");
        }

        let start_time = Instant::now();

        // Run static files and RocksDB migrations in parallel
        std::thread::scope(|s| {
            let static_files_handle = if !self.skip_static_files {
                Some(s.spawn(|| {
                    info!(target: "reth::cli", "Starting static files migration");
                    self.migrate_to_static_files::<N>(
                        &provider_factory,
                        to_block,
                        can_migrate_receipts,
                    )
                }))
            } else {
                None
            };

            #[cfg(feature = "edge")]
            let rocksdb_handle = if !self.skip_rocksdb {
                Some(s.spawn(|| {
                    info!(target: "reth::cli", "Starting RocksDB migration");
                    self.migrate_to_rocksdb::<N>(&provider_factory, self.batch_size)
                }))
            } else {
                None
            };

            #[cfg(not(feature = "edge"))]
            if !self.skip_rocksdb {
                warn!(target: "reth::cli", "Skipping RocksDB migration (requires 'edge' feature)");
            }

            // Wait for static files migration
            if let Some(handle) = static_files_handle {
                handle.join().expect("static files thread panicked")?;
            }

            // Wait for RocksDB migration
            #[cfg(feature = "edge")]
            if let Some(handle) = rocksdb_handle {
                handle.join().expect("rocksdb thread panicked")?;
            }

            Ok::<_, eyre::Error>(())
        })?;

        // Finalize: update storage settings and optionally drop migrated MDBX tables
        info!(target: "reth::cli", "Finalizing migration");
        self.finalize::<N>(&provider_factory, can_migrate_receipts)?;

        let elapsed = start_time.elapsed();
        info!(
            target: "reth::cli",
            elapsed_secs = elapsed.as_secs(),
            "Migration completed"
        );

        Ok(())
    }

    fn migrate_to_static_files<N: CliNodeTypes>(
        &self,
        provider_factory: &ProviderFactory<
            reth_node_builder::NodeTypesWithDBAdapter<N, Arc<DatabaseEnv>>,
        >,
        to_block: BlockNumber,
        can_migrate_receipts: bool,
    ) -> Result<()> {
        let mut segments = vec![
            StaticFileSegment::TransactionSenders,
            StaticFileSegment::AccountChangeSets,
            StaticFileSegment::StorageChangeSets,
        ];
        if can_migrate_receipts {
            segments.push(StaticFileSegment::Receipts);
        }

        segments.into_par_iter().try_for_each(|segment| {
            self.migrate_segment::<N>(provider_factory, segment, to_block)
        })?;

        Ok(())
    }

    fn migrate_segment<N: CliNodeTypes>(
        &self,
        provider_factory: &ProviderFactory<
            reth_node_builder::NodeTypesWithDBAdapter<N, Arc<DatabaseEnv>>,
        >,
        segment: StaticFileSegment,
        to_block: BlockNumber,
    ) -> Result<()> {
        let static_file_provider = provider_factory.static_file_provider();
        let provider = provider_factory.provider()?.disable_long_read_transaction_safety();

        let highest = static_file_provider.get_highest_static_file_block(segment).unwrap_or(0);
        if highest >= to_block {
            info!(target: "reth::cli", ?segment, "Already up to date");
            return Ok(());
        }

        let start = highest.saturating_add(1);
        let total_blocks = to_block.saturating_sub(start) + 1;
        info!(target: "reth::cli", ?segment, from = start, to = to_block, total_blocks, "Migrating");

        let mut writer = static_file_provider.latest_writer(segment)?;
        let segment_start = Instant::now();
        let mut last_log = Instant::now();
        let mut blocks_processed = 0u64;
        let mut entries_processed = 0u64;

        match segment {
            StaticFileSegment::TransactionSenders => {
                for block in start..=to_block {
                    if let Some(body) = provider.block_body_indices(block)? {
                        let senders = provider.senders_by_tx_range(
                            body.first_tx_num..body.first_tx_num + body.tx_count,
                        )?;
                        for (i, sender) in senders.into_iter().enumerate() {
                            writer
                                .append_transaction_sender(body.first_tx_num + i as u64, &sender)?;
                            entries_processed += 1;
                        }
                    }
                    blocks_processed += 1;
                    if last_log.elapsed() >= PROGRESS_LOG_INTERVAL {
                        log_progress(
                            segment,
                            blocks_processed,
                            total_blocks,
                            entries_processed,
                            segment_start.elapsed(),
                        );
                        last_log = Instant::now();
                    }
                }
            }
            StaticFileSegment::AccountChangeSets => {
                let tx = provider.tx_ref();
                let mut cursor = tx.cursor_dup_read::<tables::AccountChangeSets>()?;

                let mut current_block = start;
                let mut block_changesets: Vec<AccountBeforeTx> = Vec::new();

                for result in cursor.walk_range(start..=to_block)? {
                    let (block, changeset) = result?;

                    if block != current_block {
                        if !block_changesets.is_empty() {
                            writer.append_account_changeset(
                                std::mem::take(&mut block_changesets),
                                current_block,
                            )?;
                        }
                        // Advance writer to handle gaps in changeset data
                        writer.ensure_at_block(block.saturating_sub(1))?;
                        blocks_processed += block - current_block;
                        current_block = block;

                        if last_log.elapsed() >= PROGRESS_LOG_INTERVAL {
                            log_progress(
                                segment,
                                blocks_processed,
                                total_blocks,
                                entries_processed,
                                segment_start.elapsed(),
                            );
                            last_log = Instant::now();
                        }
                    }
                    block_changesets.push(changeset);
                    entries_processed += 1;
                }

                if !block_changesets.is_empty() {
                    writer.append_account_changeset(block_changesets, current_block)?;
                }
            }
            StaticFileSegment::StorageChangeSets => {
                let tx = provider.tx_ref();
                let mut cursor = tx.cursor_dup_read::<tables::StorageChangeSets>()?;
                let start_key =
                    reth_db_api::models::BlockNumberAddress((start, Default::default()));
                let end_key =
                    reth_db_api::models::BlockNumberAddress((to_block, Address::new([0xff; 20])));

                let mut current_block = start;
                let mut block_changesets: Vec<StorageBeforeTx> = Vec::new();

                for result in cursor.walk_range(start_key..=end_key)? {
                    let (key, entry) = result?;
                    let block = key.block_number();

                    if block != current_block {
                        if !block_changesets.is_empty() {
                            writer.append_storage_changeset(
                                std::mem::take(&mut block_changesets),
                                current_block,
                            )?;
                        }
                        // Advance writer to handle gaps in changeset data
                        writer.ensure_at_block(block.saturating_sub(1))?;
                        blocks_processed += block - current_block;
                        current_block = block;

                        if last_log.elapsed() >= PROGRESS_LOG_INTERVAL {
                            log_progress(
                                segment,
                                blocks_processed,
                                total_blocks,
                                entries_processed,
                                segment_start.elapsed(),
                            );
                            last_log = Instant::now();
                        }
                    }
                    block_changesets.push(StorageBeforeTx {
                        address: key.address(),
                        key: entry.key,
                        value: entry.value,
                    });
                    entries_processed += 1;
                }

                if !block_changesets.is_empty() {
                    writer.append_storage_changeset(block_changesets, current_block)?;
                }
            }
            StaticFileSegment::Receipts => {
                let tx = provider.tx_ref();
                let mut cursor = tx.cursor_read::<tables::Receipts<_>>()?;
                for block in start..=to_block {
                    if let Some(body) = provider.block_body_indices(block)? {
                        for tx_num in body.first_tx_num..body.first_tx_num + body.tx_count {
                            if let Some(receipt) = cursor.seek_exact(tx_num)?.map(|(_, r)| r) {
                                writer.append_receipt(tx_num, &receipt)?;
                                entries_processed += 1;
                            }
                        }
                    }
                    blocks_processed += 1;
                    if last_log.elapsed() >= PROGRESS_LOG_INTERVAL {
                        log_progress(
                            segment,
                            blocks_processed,
                            total_blocks,
                            entries_processed,
                            segment_start.elapsed(),
                        );
                        last_log = Instant::now();
                    }
                }
            }
            _ => {}
        }

        writer.commit()?;

        let elapsed = segment_start.elapsed();
        let rate = if elapsed.as_secs() > 0 {
            entries_processed / elapsed.as_secs()
        } else {
            entries_processed
        };
        info!(
            target: "reth::cli",
            ?segment,
            entries = entries_processed,
            elapsed_secs = elapsed.as_secs(),
            rate_per_sec = rate,
            "Done"
        );
        Ok(())
    }

    #[cfg(feature = "edge")]
    fn migrate_to_rocksdb<N: CliNodeTypes>(
        &self,
        provider_factory: &ProviderFactory<
            reth_node_builder::NodeTypesWithDBAdapter<N, Arc<DatabaseEnv>>,
        >,
        batch_size: u64,
    ) -> Result<()> {
        use reth_db_api::table::Table;

        [
            tables::TransactionHashNumbers::NAME,
            tables::AccountsHistory::NAME,
            tables::StoragesHistory::NAME,
        ]
        .into_par_iter()
        .try_for_each(|table| {
            self.migrate_rocksdb_table::<N>(provider_factory, table, batch_size)
        })?;
        Ok(())
    }

    #[cfg(feature = "edge")]
    fn migrate_rocksdb_table<N: CliNodeTypes>(
        &self,
        provider_factory: &ProviderFactory<
            reth_node_builder::NodeTypesWithDBAdapter<N, Arc<DatabaseEnv>>,
        >,
        table: &'static str,
        batch_size: u64,
    ) -> Result<()> {
        use reth_db_api::table::Table;
        use reth_provider::RocksDBProviderFactory;

        let provider = provider_factory.provider()?.disable_long_read_transaction_safety();
        let rocksdb = provider_factory.rocksdb_provider();
        let tx = provider.tx_ref();

        info!(target: "reth::cli", table, "Migrating");

        let table_start = Instant::now();
        let mut last_log = Instant::now();

        let count = match table {
            tables::TransactionHashNumbers::NAME => {
                let mut cursor = tx.cursor_read::<tables::TransactionHashNumbers>()?;
                let mut batch = rocksdb.batch_with_auto_commit();
                let mut count = 0u64;

                for result in cursor.walk(None)? {
                    let (hash, tx_num) = result?;
                    batch.put::<tables::TransactionHashNumbers>(hash, &tx_num)?;
                    count += 1;

                    if count.is_multiple_of(batch_size) &&
                        last_log.elapsed() >= PROGRESS_LOG_INTERVAL
                    {
                        log_rocksdb_progress(table, count, table_start.elapsed());
                        last_log = Instant::now();
                    }
                }

                batch.commit()?;
                count
            }
            tables::AccountsHistory::NAME => {
                let mut cursor = tx.cursor_read::<tables::AccountsHistory>()?;
                let mut batch = rocksdb.batch_with_auto_commit();
                let mut count = 0u64;

                for result in cursor.walk(None)? {
                    let (key, value) = result?;
                    batch.put::<tables::AccountsHistory>(key, &value)?;
                    count += 1;
                    if count.is_multiple_of(batch_size) {
                        batch.commit()?;
                        batch = rocksdb.batch_with_auto_commit();

                        if last_log.elapsed() >= PROGRESS_LOG_INTERVAL {
                            log_rocksdb_progress(table, count, table_start.elapsed());
                            last_log = Instant::now();
                        }
                    }
                }

                batch.commit()?;
                count
            }
            tables::StoragesHistory::NAME => {
                let mut cursor = tx.cursor_read::<tables::StoragesHistory>()?;
                let mut batch = rocksdb.batch_with_auto_commit();
                let mut count = 0u64;

                for result in cursor.walk(None)? {
                    let (key, value) = result?;
                    batch.put::<tables::StoragesHistory>(key, &value)?;
                    count += 1;
                    if count.is_multiple_of(batch_size) {
                        batch.commit()?;
                        batch = rocksdb.batch_with_auto_commit();

                        if last_log.elapsed() >= PROGRESS_LOG_INTERVAL {
                            log_rocksdb_progress(table, count, table_start.elapsed());
                            last_log = Instant::now();
                        }
                    }
                }

                batch.commit()?;
                count
            }
            _ => 0,
        };

        let elapsed = table_start.elapsed();
        let rate = if elapsed.as_secs() > 0 { count / elapsed.as_secs() } else { count };
        info!(
            target: "reth::cli",
            table,
            entries = count,
            elapsed_secs = elapsed.as_secs(),
            rate_per_sec = rate,
            "Done"
        );
        Ok(())
    }

    fn finalize<N: CliNodeTypes>(
        &self,
        provider_factory: &ProviderFactory<
            reth_node_builder::NodeTypesWithDBAdapter<N, Arc<DatabaseEnv>>,
        >,
        can_migrate_receipts: bool,
    ) -> Result<()> {
        use reth_db_api::transaction::DbTxMut;
        use reth_provider::StorageSettings;

        let provider = provider_factory.provider_rw()?;

        // Check if TransactionSenders actually has data in static files
        // If not, don't enable that setting to avoid consistency check failures
        let static_file_provider = provider_factory.static_file_provider();
        let senders_have_data = static_file_provider
            .get_highest_static_file_tx(StaticFileSegment::TransactionSenders)
            .is_some();

        if !senders_have_data {
            warn!(
                target: "reth::cli",
                "TransactionSenders has no data in static files, not enabling static file storage for senders"
            );
        }

        // Update storage settings - only enable senders if we have data
        #[cfg(feature = "edge")]
        let new_settings = StorageSettings::base()
            .with_receipts_in_static_files(can_migrate_receipts)
            .with_account_changesets_in_static_files(true)
            .with_transaction_senders_in_static_files(senders_have_data)
            .with_transaction_hash_numbers_in_rocksdb(true)
            .with_account_history_in_rocksdb(true)
            .with_storages_history_in_rocksdb(true);

        #[cfg(not(feature = "edge"))]
        let new_settings = StorageSettings::base()
            .with_receipts_in_static_files(can_migrate_receipts)
            .with_account_changesets_in_static_files(true)
            .with_transaction_senders_in_static_files(senders_have_data);

        info!(target: "reth::cli", ?new_settings, "Writing storage settings");
        provider.write_storage_settings(new_settings)?;

        // Drop migrated MDBX tables unless --keep-mdbx is set
        if !self.keep_mdbx {
            let tx = provider.tx_ref();

            if !self.skip_static_files {
                info!(target: "reth::cli", "Dropping migrated static file tables from MDBX");
                tx.clear::<tables::TransactionSenders>()?;
                tx.clear::<tables::AccountChangeSets>()?;
                tx.clear::<tables::StorageChangeSets>()?;
                if can_migrate_receipts {
                    tx.clear::<tables::Receipts<<<N as reth_node_builder::NodeTypes>::Primitives as reth_primitives_traits::NodePrimitives>::Receipt>>()?;
                }
            }

            #[cfg(feature = "edge")]
            if !self.skip_rocksdb {
                info!(target: "reth::cli", "Dropping migrated RocksDB tables from MDBX");
                tx.clear::<tables::TransactionHashNumbers>()?;
                tx.clear::<tables::AccountsHistory>()?;
                tx.clear::<tables::StoragesHistory>()?;
            }
        } else {
            info!(target: "reth::cli", "Keeping MDBX tables (--keep-mdbx)");
        }

        provider.commit()?;

        Ok(())
    }
}

/// Log progress for static file segment migration.
fn log_progress(
    segment: StaticFileSegment,
    blocks_done: u64,
    total_blocks: u64,
    entries: u64,
    elapsed: Duration,
) {
    let pct = (blocks_done * 100).checked_div(total_blocks).unwrap_or(0);
    let rate = if elapsed.as_secs() > 0 { entries / elapsed.as_secs() } else { entries };
    let eta_secs = if blocks_done > 0 && pct < 100 {
        let remaining = total_blocks.saturating_sub(blocks_done);
        let secs_per_block = elapsed.as_secs_f64() / blocks_done as f64;
        (remaining as f64 * secs_per_block) as u64
    } else {
        0
    };

    info!(
        target: "reth::cli",
        ?segment,
        progress = %format!("{blocks_done}/{total_blocks} ({pct}%)"),
        entries,
        rate_per_sec = rate,
        eta_secs,
        "Progress"
    );
}

/// Log progress for RocksDB table migration.
#[cfg(feature = "edge")]
fn log_rocksdb_progress(table: &'static str, entries: u64, elapsed: Duration) {
    let rate = if elapsed.as_secs() > 0 { entries / elapsed.as_secs() } else { entries };
    info!(
        target: "reth::cli",
        table,
        entries,
        elapsed_secs = elapsed.as_secs(),
        rate_per_sec = rate,
        "Progress"
    );
}
