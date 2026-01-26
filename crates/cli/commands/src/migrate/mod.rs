//! Storage migration command for Reth.
//!
//! Migrates data from legacy MDBX storage to RocksDB + static files.

mod progress;

use std::{
    hash::{BuildHasher, Hasher},
    sync::Arc,
    time::Instant,
};

use alloy_primitives::map::foldhash::fast::FixedState;
use clap::Parser;
use eyre::Result;
use rayon::prelude::*;
use reth_chainspec::{EthChainSpec, EthereumHardforks};
use reth_cli::chainspec::ChainSpecParser;
use reth_cli_runner::CliContext;
use reth_db::{
    cursor::{DbCursorRO, DbDupCursorRO},
    tables,
    transaction::DbTx,
    DatabaseEnv,
};
use reth_db_api::{RawKey, RawTable, RawValue};
use reth_primitives_traits::BlockNumber;
use reth_provider::{
    BlockNumReader, BlockReader, DBProvider, EitherWriter, ProviderFactory,
    StaticFileProviderFactory, StaticFileWriter, TransactionsProvider,
};
use reth_static_file_types::StaticFileSegment;
use tracing::{error, info, warn};

use crate::common::{AccessRights, CliNodeTypes, Environment, EnvironmentArgs};

pub use self::progress::MigrationProgress;

/// Migrate from legacy MDBX storage to new RocksDB + static files.
#[derive(Debug, Parser)]
pub struct Command<C: ChainSpecParser> {
    #[command(flatten)]
    env: EnvironmentArgs<C>,

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

impl<C: ChainSpecParser<ChainSpec: EthChainSpec + EthereumHardforks>> Command<C> {
    /// Execute the migration command.
    pub async fn execute<N: CliNodeTypes<ChainSpec = C::ChainSpec>>(
        self,
        _ctx: CliContext,
    ) -> Result<()> {
        let Environment { provider_factory, .. } = self.env.init::<N>(AccessRights::RW)?;

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
            warn!(target: "reth::cli", "Receipts will NOT be migrated due to contract log pruning configuration");
        }

        let start_time = Instant::now();

        // Phase 1: Migrate to static files (parallel)
        if !self.skip_static_files {
            info!(target: "reth::cli", "Phase 1: Migrating data to static files (parallel)");
            self.migrate_to_static_files_parallel::<N>(
                &provider_factory,
                self.from_block,
                to_block,
                can_migrate_receipts,
            )?;
        } else {
            info!(target: "reth::cli", "Skipping static file migration");
        }

        // Phase 2: Migrate indexes to RocksDB (parallel)
        if !self.skip_rocksdb {
            info!(target: "reth::cli", "Phase 2: Migrating indexes to RocksDB (parallel)");
            self.migrate_to_rocksdb_parallel::<N>(&provider_factory, self.batch_size)?;
        } else {
            info!(target: "reth::cli", "Skipping RocksDB migration");
        }

        // Phase 3: Verify checksums
        info!(target: "reth::cli", "Phase 3: Verifying data integrity with checksums");
        self.verify_checksums::<N>(&provider_factory, can_migrate_receipts)?;

        // Phase 4: Update storage settings
        info!(target: "reth::cli", "Phase 4: Updating storage settings");
        self.update_storage_settings::<N>(&provider_factory, can_migrate_receipts)?;

        let elapsed = start_time.elapsed();

        info!(
            target: "reth::cli",
            elapsed_secs = elapsed.as_secs(),
            blocks_migrated = total_blocks,
            "Migration completed successfully"
        );

        Ok(())
    }

    fn migrate_to_static_files_parallel<N: CliNodeTypes<ChainSpec = C::ChainSpec>>(
        &self,
        provider_factory: &ProviderFactory<
            reth_node_builder::NodeTypesWithDBAdapter<N, Arc<DatabaseEnv>>,
        >,
        from_block: BlockNumber,
        to_block: BlockNumber,
        can_migrate_receipts: bool,
    ) -> Result<()> {
        // Build list of segments to migrate
        let mut segments = vec![
            StaticFileSegment::TransactionSenders,
            StaticFileSegment::AccountChangeSets,
            StaticFileSegment::StorageChangeSets,
        ];
        if can_migrate_receipts {
            segments.push(StaticFileSegment::Receipts);
        }

        // Run each segment migration in parallel
        segments.into_par_iter().try_for_each(|segment| {
            self.migrate_segment_to_static_file::<N>(
                provider_factory,
                segment,
                from_block,
                to_block,
            )
        })?;

        Ok(())
    }

    fn migrate_segment_to_static_file<N: CliNodeTypes<ChainSpec = C::ChainSpec>>(
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

        let highest_static =
            static_file_provider.get_highest_static_file_block(segment).unwrap_or(0);

        if highest_static >= to_block {
            info!(target: "reth::cli", ?segment, highest_static, "Segment already up to date");
            return Ok(());
        }

        let start_block = highest_static.saturating_add(1).max(from_block);
        info!(target: "reth::cli", ?segment, from = start_block, to = to_block, "Migrating segment");

        let mut writer = static_file_provider.latest_writer(segment)?;

        match segment {
            StaticFileSegment::TransactionSenders => {
                for block_num in start_block..=to_block {
                    if let Some(body) = provider.block_body_indices(block_num)? {
                        let senders = provider.senders_by_tx_range(
                            body.first_tx_num..body.first_tx_num + body.tx_count as u64,
                        )?;
                        for (idx, sender) in senders.into_iter().enumerate() {
                            writer.append_sender(body.first_tx_num + idx as u64, sender)?;
                        }
                    }
                }
            }
            StaticFileSegment::AccountChangeSets => {
                let tx = provider.tx_ref();
                let mut cursor = tx.cursor_dup_read::<tables::AccountChangeSets>()?;

                for result in cursor.walk_range(start_block..=to_block)? {
                    let (block_num, changeset) = result?;
                    writer.append_account_changeset(block_num, changeset)?;
                }
            }
            StaticFileSegment::StorageChangeSets => {
                let tx = provider.tx_ref();
                let mut cursor = tx.cursor_dup_read::<tables::StorageChangeSets>()?;

                let start_key =
                    reth_db_api::models::BlockNumberAddress((start_block, Default::default()));
                let end_key = reth_db_api::models::BlockNumberAddress((
                    to_block,
                    alloy_primitives::Address::MAX,
                ));

                for result in cursor.walk_range(start_key..=end_key)? {
                    let (key, changeset) = result?;
                    writer.append_storage_changeset(
                        key.block_number(),
                        key.address(),
                        changeset,
                    )?;
                }
            }
            StaticFileSegment::Receipts => {
                let tx = provider.tx_ref();
                let mut cursor = tx.cursor_read::<tables::Receipts<_>>()?;

                for block_num in start_block..=to_block {
                    if let Some(body) = provider.block_body_indices(block_num)? {
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
        info!(target: "reth::cli", ?segment, "Segment migration complete");

        Ok(())
    }

    fn migrate_to_rocksdb_parallel<N: CliNodeTypes<ChainSpec = C::ChainSpec>>(
        &self,
        provider_factory: &ProviderFactory<
            reth_node_builder::NodeTypesWithDBAdapter<N, Arc<DatabaseEnv>>,
        >,
        batch_size: u64,
    ) -> Result<()> {
        // Run RocksDB migrations in parallel
        let results: Vec<Result<()>> = ["tx_hash_numbers", "accounts_history", "storages_history"]
            .into_par_iter()
            .map(|table| match table {
                "tx_hash_numbers" => {
                    self.migrate_tx_hash_numbers::<N>(provider_factory, batch_size)
                }
                "accounts_history" => {
                    self.migrate_accounts_history::<N>(provider_factory, batch_size)
                }
                "storages_history" => {
                    self.migrate_storages_history::<N>(provider_factory, batch_size)
                }
                _ => Ok(()),
            })
            .collect();

        for result in results {
            result?;
        }

        Ok(())
    }

    fn migrate_tx_hash_numbers<N: CliNodeTypes<ChainSpec = C::ChainSpec>>(
        &self,
        provider_factory: &ProviderFactory<
            reth_node_builder::NodeTypesWithDBAdapter<N, Arc<DatabaseEnv>>,
        >,
        batch_size: u64,
    ) -> Result<()> {
        let provider = provider_factory.provider()?;
        let rocksdb = provider_factory.rocksdb_provider();

        info!(target: "reth::cli", "Migrating TransactionHashNumbers to RocksDB");

        let tx = provider.tx_ref();
        let mut cursor = tx.cursor_read::<tables::TransactionHashNumbers>()?;

        let mut batch = Vec::new();
        let mut count = 0u64;

        for result in cursor.walk(None)? {
            let (hash, tx_num) = result?;
            batch.push((hash, tx_num));

            if batch.len() >= batch_size as usize {
                rocksdb.insert_tx_hash_numbers_batch(&batch)?;
                batch.clear();
            }

            count += 1;
        }

        if !batch.is_empty() {
            rocksdb.insert_tx_hash_numbers_batch(&batch)?;
        }

        info!(target: "reth::cli", count, "TransactionHashNumbers migration complete");
        Ok(())
    }

    fn migrate_accounts_history<N: CliNodeTypes<ChainSpec = C::ChainSpec>>(
        &self,
        provider_factory: &ProviderFactory<
            reth_node_builder::NodeTypesWithDBAdapter<N, Arc<DatabaseEnv>>,
        >,
        batch_size: u64,
    ) -> Result<()> {
        let provider = provider_factory.provider()?;
        let rocksdb = provider_factory.rocksdb_provider();

        info!(target: "reth::cli", "Migrating AccountsHistory to RocksDB");

        let tx = provider.tx_ref();
        let mut cursor = tx.cursor_read::<tables::AccountsHistory>()?;

        let mut count = 0u64;

        for result in cursor.walk(None)? {
            let (key, value) = result?;
            rocksdb.insert_account_history(key, value)?;

            count += 1;
            if count % batch_size == 0 {
                rocksdb.flush()?;
            }
        }
        rocksdb.flush()?;

        info!(target: "reth::cli", count, "AccountsHistory migration complete");
        Ok(())
    }

    fn migrate_storages_history<N: CliNodeTypes<ChainSpec = C::ChainSpec>>(
        &self,
        provider_factory: &ProviderFactory<
            reth_node_builder::NodeTypesWithDBAdapter<N, Arc<DatabaseEnv>>,
        >,
        batch_size: u64,
    ) -> Result<()> {
        let provider = provider_factory.provider()?;
        let rocksdb = provider_factory.rocksdb_provider();

        info!(target: "reth::cli", "Migrating StoragesHistory to RocksDB");

        let tx = provider.tx_ref();
        let mut cursor = tx.cursor_read::<tables::StoragesHistory>()?;

        let mut count = 0u64;

        for result in cursor.walk(None)? {
            let (key, value) = result?;
            rocksdb.insert_storage_history(key, value)?;

            count += 1;
            if count % batch_size == 0 {
                rocksdb.flush()?;
            }
        }
        rocksdb.flush()?;

        info!(target: "reth::cli", count, "StoragesHistory migration complete");
        Ok(())
    }

    fn verify_checksums<N: CliNodeTypes<ChainSpec = C::ChainSpec>>(
        &self,
        provider_factory: &ProviderFactory<
            reth_node_builder::NodeTypesWithDBAdapter<N, Arc<DatabaseEnv>>,
        >,
        can_migrate_receipts: bool,
    ) -> Result<()> {
        let provider = provider_factory.provider()?;
        let rocksdb = provider_factory.rocksdb_provider();
        let tx = provider.tx_ref();

        // Verify RocksDB tables
        info!(target: "reth::cli", "Verifying TransactionHashNumbers checksum");
        let mdbx_checksum = self.compute_table_checksum::<tables::TransactionHashNumbers>(tx)?;
        let rocksdb_checksum = rocksdb.checksum_tx_hash_numbers()?;

        if mdbx_checksum != rocksdb_checksum {
            return Err(eyre::eyre!(
                "Checksum mismatch for TransactionHashNumbers: rocksdb={:#x}, mdbx={:#x}",
                rocksdb_checksum,
                mdbx_checksum
            ));
        }
        info!(target: "reth::cli", checksum = %format!("{:#x}", mdbx_checksum), "TransactionHashNumbers verified");

        info!(target: "reth::cli", "Verifying AccountsHistory checksum");
        let mdbx_checksum = self.compute_table_checksum::<tables::AccountsHistory>(tx)?;
        let rocksdb_checksum = rocksdb.checksum_accounts_history()?;

        if mdbx_checksum != rocksdb_checksum {
            return Err(eyre::eyre!(
                "Checksum mismatch for AccountsHistory: rocksdb={:#x}, mdbx={:#x}",
                rocksdb_checksum,
                mdbx_checksum
            ));
        }
        info!(target: "reth::cli", checksum = %format!("{:#x}", mdbx_checksum), "AccountsHistory verified");

        info!(target: "reth::cli", "Verifying StoragesHistory checksum");
        let mdbx_checksum = self.compute_table_checksum::<tables::StoragesHistory>(tx)?;
        let rocksdb_checksum = rocksdb.checksum_storages_history()?;

        if mdbx_checksum != rocksdb_checksum {
            return Err(eyre::eyre!(
                "Checksum mismatch for StoragesHistory: rocksdb={:#x}, mdbx={:#x}",
                rocksdb_checksum,
                mdbx_checksum
            ));
        }
        info!(target: "reth::cli", checksum = %format!("{:#x}", mdbx_checksum), "StoragesHistory verified");

        // Verify static file segments
        let static_file_provider = provider_factory.static_file_provider();

        let mut segments_to_verify = vec![
            (StaticFileSegment::TransactionSenders, tables::TransactionSenders::NAME),
            (StaticFileSegment::AccountChangeSets, tables::AccountChangeSets::NAME),
            (StaticFileSegment::StorageChangeSets, tables::StorageChangeSets::NAME),
        ];

        if can_migrate_receipts {
            segments_to_verify.push((StaticFileSegment::Receipts, "Receipts"));
        }

        for (segment, name) in segments_to_verify {
            info!(target: "reth::cli", segment = name, "Verifying static file checksum");

            let sf_checksum = self.compute_static_file_checksum(&static_file_provider, segment)?;

            let mdbx_checksum = match segment {
                StaticFileSegment::TransactionSenders => {
                    self.compute_table_checksum::<tables::TransactionSenders>(tx)?
                }
                StaticFileSegment::AccountChangeSets => {
                    self.compute_dup_table_checksum::<tables::AccountChangeSets>(tx)?
                }
                StaticFileSegment::StorageChangeSets => {
                    self.compute_dup_table_checksum::<tables::StorageChangeSets>(tx)?
                }
                StaticFileSegment::Receipts => {
                    self.compute_table_checksum::<tables::Receipts<_>>(tx)?
                }
                _ => continue,
            };

            if sf_checksum != mdbx_checksum {
                return Err(eyre::eyre!(
                    "Checksum mismatch for {}: static_file={:#x}, mdbx={:#x}",
                    name,
                    sf_checksum,
                    mdbx_checksum
                ));
            }
            info!(target: "reth::cli", segment = name, checksum = %format!("{:#x}", sf_checksum), "Verified");
        }

        info!(target: "reth::cli", "All checksums verified successfully");
        Ok(())
    }

    fn compute_static_file_checksum<N: reth_node_types::NodePrimitives>(
        &self,
        provider: &reth_provider::providers::StaticFileProvider<N>,
        segment: StaticFileSegment,
    ) -> Result<u64> {
        use reth_db::static_file::iter_static_files;

        let static_files = iter_static_files(provider.directory())?;
        let ranges = match static_files.get(segment) {
            Some(r) => r,
            None => return Ok(0), // No static files for this segment
        };

        let mut hasher = checksum_hasher();

        for (block_range, _) in ranges.iter() {
            let fixed_range = provider.find_fixed_range(segment, block_range.start());
            if let Some(jar_provider) =
                provider.get_segment_provider_for_range(segment, || Some(fixed_range), None)?
            {
                let mut cursor = jar_provider.cursor()?;
                while let Ok(Some(row)) = cursor.next_row() {
                    for col_data in row.iter() {
                        hasher.write(col_data);
                    }
                }
            }
        }

        Ok(hasher.finish())
    }

    fn compute_table_checksum<T: reth_db_api::table::Table>(&self, tx: &impl DbTx) -> Result<u64> {
        let mut cursor = tx.cursor_read::<RawTable<T>>()?;
        let mut hasher = checksum_hasher();

        for result in cursor.walk(None)? {
            let (k, v): (RawKey<T::Key>, RawValue<T::Value>) = result?;
            hasher.write(k.raw_key());
            hasher.write(v.raw_value());
        }

        Ok(hasher.finish())
    }

    fn compute_dup_table_checksum<T: reth_db_api::table::DupSort>(
        &self,
        tx: &impl DbTx,
    ) -> Result<u64> {
        let mut cursor = tx.cursor_dup_read::<RawTable<T>>()?;
        let mut hasher = checksum_hasher();

        for result in cursor.walk(None)? {
            let (k, v): (RawKey<T::Key>, RawValue<T::Value>) = result?;
            hasher.write(k.raw_key());
            hasher.write(v.raw_value());
        }

        Ok(hasher.finish())
    }

    fn update_storage_settings<N: CliNodeTypes<ChainSpec = C::ChainSpec>>(
        &self,
        provider_factory: &ProviderFactory<
            reth_node_builder::NodeTypesWithDBAdapter<N, Arc<DatabaseEnv>>,
        >,
        can_migrate_receipts: bool,
    ) -> Result<()> {
        use reth_provider::StorageSettings;

        let provider = provider_factory.provider_rw()?;

        let new_settings = StorageSettings::base()
            .with_receipts_in_static_files(can_migrate_receipts)
            .with_account_changesets_in_static_files(true)
            .with_transaction_senders_in_static_files(true)
            .with_transaction_hash_numbers_in_rocksdb(true)
            .with_account_history_in_rocksdb(true)
            .with_storages_history_in_rocksdb(true);

        info!(target: "reth::cli", ?new_settings, "Writing new storage settings");
        provider.write_storage_settings(new_settings)?;
        provider.commit()?;

        info!(target: "reth::cli", "Storage settings updated successfully");
        Ok(())
    }
}

/// Creates a hasher with the standard seed for checksum computation.
fn checksum_hasher() -> impl Hasher {
    FixedState::with_seed(u64::from_be_bytes(*b"RETHRETH")).build_hasher()
}
