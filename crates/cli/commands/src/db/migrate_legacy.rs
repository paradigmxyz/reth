//! Storage migration command for Reth.
//!
//! Migrates data from legacy MDBX storage to RocksDB + static files.

use std::{sync::Arc, time::Instant};

use alloy_primitives::{Address, BlockNumber};
use clap::Parser;
use eyre::Result;
use rayon::prelude::*;
use reth_db::{cursor::DbCursorRO, tables, transaction::DbTx, DatabaseEnv};
use reth_db_api::models::{AccountBeforeTx, StorageBeforeTx};
#[cfg(feature = "edge")]
use reth_provider::RocksDBProviderFactory;
use reth_provider::{
    BlockBodyIndicesProvider, BlockNumReader, MetadataWriter, ProviderFactory,
    StaticFileProviderFactory, StaticFileWriter, TransactionsProvider,
};
use reth_static_file_types::StaticFileSegment;
use tracing::{error, info, warn};

use crate::common::CliNodeTypes;

/// Migrate from legacy MDBX storage to new RocksDB + static files.
#[derive(Debug, Parser)]
pub struct Command {
    /// Block batch size for processing.
    #[arg(long, default_value = "10000")]
    batch_size: u64,

    /// Starting block number (defaults to 0).
    #[arg(long, default_value = "0")]
    from_block: u64,

    /// Ending block number (defaults to chain tip).
    #[arg(long)]
    to_block: Option<u64>,

    /// Skip static file migration.
    #[arg(long)]
    skip_static_files: bool,

    /// Skip RocksDB migration.
    #[arg(long)]
    skip_rocksdb: bool,
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

        let provider = provider_factory.provider()?;
        let chain_tip = provider.best_block_number()?;
        let prune_modes = provider.prune_modes_ref().clone();
        drop(provider);

        let to_block = self.to_block.unwrap_or(chain_tip);

        if self.from_block > to_block {
            error!(target: "reth::cli", from = self.from_block, to = to_block, "Invalid block range");
            return Err(eyre::eyre!("from_block cannot be greater than to_block"));
        }

        let total_blocks = to_block - self.from_block + 1;
        info!(
            target: "reth::cli",
            from = self.from_block,
            to = to_block,
            total = total_blocks,
            batch_size = self.batch_size,
            "Migration parameters"
        );

        // Check if receipts can be migrated (no contract log pruning)
        let can_migrate_receipts = prune_modes.receipts_log_filter.is_empty();
        if !can_migrate_receipts {
            warn!(target: "reth::cli", "Receipts will NOT be migrated due to contract log pruning");
        }

        let start_time = Instant::now();

        // Phase 1: Migrate to static files
        if !self.skip_static_files {
            info!(target: "reth::cli", "Phase 1: Migrating to static files");
            self.migrate_to_static_files::<N>(
                &provider_factory,
                self.from_block,
                to_block,
                can_migrate_receipts,
            )?;
        }

        // Phase 2: Migrate indexes to RocksDB
        if !self.skip_rocksdb {
            #[cfg(feature = "edge")]
            {
                info!(target: "reth::cli", "Phase 2: Migrating to RocksDB");
                self.migrate_to_rocksdb::<N>(&provider_factory, self.batch_size)?;
            }
            #[cfg(not(feature = "edge"))]
            {
                warn!(target: "reth::cli", "Phase 2: Skipping RocksDB migration (requires 'edge' feature)");
            }
        }

        // Phase 3: Update storage settings
        info!(target: "reth::cli", "Phase 3: Updating storage settings");
        self.update_storage_settings::<N>(&provider_factory, can_migrate_receipts)?;

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
        from_block: BlockNumber,
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
            self.migrate_segment::<N>(provider_factory, segment, from_block, to_block)
        })?;

        Ok(())
    }

    fn migrate_segment<N: CliNodeTypes>(
        &self,
        provider_factory: &ProviderFactory<
            reth_node_builder::NodeTypesWithDBAdapter<N, Arc<DatabaseEnv>>,
        >,
        segment: StaticFileSegment,
        from_block: BlockNumber,
        to_block: BlockNumber,
    ) -> Result<()> {
        let static_file_provider = provider_factory.static_file_provider();
        let provider = provider_factory.provider()?;

        let highest = static_file_provider.get_highest_static_file_block(segment).unwrap_or(0);
        if highest >= to_block {
            info!(target: "reth::cli", ?segment, "Already up to date");
            return Ok(());
        }

        let start = highest.saturating_add(1).max(from_block);
        info!(target: "reth::cli", ?segment, from = start, to = to_block, "Migrating");

        let mut writer = static_file_provider.latest_writer(segment)?;

        match segment {
            StaticFileSegment::TransactionSenders => {
                for block in start..=to_block {
                    if let Some(body) = provider.block_body_indices(block)? {
                        let senders = provider.senders_by_tx_range(
                            body.first_tx_num..body.first_tx_num + body.tx_count as u64,
                        )?;
                        for (i, sender) in senders.into_iter().enumerate() {
                            writer
                                .append_transaction_sender(body.first_tx_num + i as u64, &sender)?;
                        }
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
                        current_block = block;
                    }
                    block_changesets.push(changeset);
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
                        current_block = block;
                    }
                    block_changesets.push(StorageBeforeTx {
                        address: key.address(),
                        key: entry.key,
                        value: entry.value,
                    });
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
                        for tx_num in body.first_tx_num..body.first_tx_num + body.tx_count as u64 {
                            if let Some(receipt) = cursor.seek_exact(tx_num)?.map(|(_, r)| r) {
                                writer.append_receipt(tx_num, &receipt)?;
                            }
                        }
                    }
                }
            }
            _ => {}
        }

        writer.commit()?;
        info!(target: "reth::cli", ?segment, "Done");
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
        [RocksDBTable::TxHashNumbers, RocksDBTable::AccountsHistory, RocksDBTable::StoragesHistory]
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
        table: RocksDBTable,
        batch_size: u64,
    ) -> Result<()> {
        let provider = provider_factory.provider()?;
        let rocksdb = provider_factory.rocksdb_provider();
        let tx = provider.tx_ref();

        info!(target: "reth::cli", ?table, "Migrating");

        let count = match table {
            RocksDBTable::TxHashNumbers => {
                let mut cursor = tx.cursor_read::<tables::TransactionHashNumbers>()?;
                let mut batch = rocksdb.batch_with_auto_commit();
                let mut count = 0u64;

                for result in cursor.walk(None)? {
                    let (hash, tx_num) = result?;
                    batch.put::<tables::TransactionHashNumbers>(hash, &tx_num)?;
                    count += 1;
                }

                batch.commit()?;
                count
            }
            RocksDBTable::AccountsHistory => {
                let mut cursor = tx.cursor_read::<tables::AccountsHistory>()?;
                let mut batch = rocksdb.batch_with_auto_commit();
                let mut count = 0u64;

                for result in cursor.walk(None)? {
                    let (key, value) = result?;
                    batch.put::<tables::AccountsHistory>(key, &value)?;
                    count += 1;
                    if count % batch_size == 0 {
                        batch.commit()?;
                        batch = rocksdb.batch_with_auto_commit();
                    }
                }

                batch.commit()?;
                count
            }
            RocksDBTable::StoragesHistory => {
                let mut cursor = tx.cursor_read::<tables::StoragesHistory>()?;
                let mut batch = rocksdb.batch_with_auto_commit();
                let mut count = 0u64;

                for result in cursor.walk(None)? {
                    let (key, value) = result?;
                    batch.put::<tables::StoragesHistory>(key, &value)?;
                    count += 1;
                    if count % batch_size == 0 {
                        batch.commit()?;
                        batch = rocksdb.batch_with_auto_commit();
                    }
                }

                batch.commit()?;
                count
            }
        };

        info!(target: "reth::cli", ?table, count, "Done");
        Ok(())
    }

    fn update_storage_settings<N: CliNodeTypes>(
        &self,
        provider_factory: &ProviderFactory<
            reth_node_builder::NodeTypesWithDBAdapter<N, Arc<DatabaseEnv>>,
        >,
        can_migrate_receipts: bool,
    ) -> Result<()> {
        use reth_provider::StorageSettings;

        let provider = provider_factory.provider_rw()?;

        #[cfg(feature = "edge")]
        let new_settings = StorageSettings::base()
            .with_receipts_in_static_files(can_migrate_receipts)
            .with_account_changesets_in_static_files(true)
            .with_transaction_senders_in_static_files(true)
            .with_transaction_hash_numbers_in_rocksdb(true)
            .with_account_history_in_rocksdb(true)
            .with_storages_history_in_rocksdb(true);

        #[cfg(not(feature = "edge"))]
        let new_settings = StorageSettings::base()
            .with_receipts_in_static_files(can_migrate_receipts)
            .with_account_changesets_in_static_files(true)
            .with_transaction_senders_in_static_files(true);

        info!(target: "reth::cli", ?new_settings, "Writing storage settings");
        provider.write_storage_settings(new_settings)?;
        provider.commit()?;

        Ok(())
    }
}

/// RocksDB tables to migrate.
#[cfg(feature = "edge")]
#[derive(Debug, Clone, Copy)]
enum RocksDBTable {
    TxHashNumbers,
    AccountsHistory,
    StoragesHistory,
}
