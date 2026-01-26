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
use reth_chainspec::{EthChainSpec, EthereumHardforks};
use reth_cli::chainspec::ChainSpecParser;
use reth_cli_runner::CliContext;
use reth_db::{cursor::DbCursorRO, tables, transaction::DbTx, DatabaseEnv};
use reth_db_api::{RawKey, RawTable, RawValue};
use reth_primitives_traits::BlockNumber;
use reth_provider::{
    BlockNumReader, BlockReader, DBProvider, HeaderProvider, ProviderFactory, ReceiptProvider,
    StaticFileProviderFactory, StaticFileWriter, TransactionsProvider,
};
use reth_static_file_types::StaticFileSegment;
use tracing::{debug, error, info};

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

        let mut progress = MigrationProgress::new(total_blocks);
        let start_time = Instant::now();

        // Phase 1: Migrate to static files
        if !self.skip_static_files {
            info!(target: "reth::cli", "Phase 1: Migrating finalized data to static files");
            self.migrate_to_static_files::<N>(
                &provider_factory,
                self.from_block,
                to_block,
                self.batch_size,
                &mut progress,
            )?;
        } else {
            info!(target: "reth::cli", "Skipping static file migration");
        }

        // Phase 2: Migrate indexes to RocksDB
        if !self.skip_rocksdb {
            info!(target: "reth::cli", "Phase 2: Migrating indexes to RocksDB");
            self.migrate_to_rocksdb::<N>(&provider_factory, self.batch_size, &mut progress)?;
        } else {
            info!(target: "reth::cli", "Skipping RocksDB migration");
        }

        // Phase 3: Verify checksums before updating settings
        info!(target: "reth::cli", "Phase 3: Verifying data integrity with checksums");
        self.verify_checksums::<N>(&provider_factory)?;

        // Phase 4: Update storage settings
        info!(target: "reth::cli", "Phase 4: Updating storage settings");
        self.update_storage_settings::<N>(&provider_factory)?;

        let elapsed = start_time.elapsed();
        progress.finish();

        info!(
            target: "reth::cli",
            elapsed_secs = elapsed.as_secs(),
            blocks_migrated = total_blocks,
            "Migration completed successfully"
        );

        Ok(())
    }

    fn migrate_to_static_files<N: CliNodeTypes<ChainSpec = C::ChainSpec>>(
        &self,
        provider_factory: &ProviderFactory<
            reth_node_builder::NodeTypesWithDBAdapter<N, Arc<DatabaseEnv>>,
        >,
        from_block: BlockNumber,
        to_block: BlockNumber,
        batch_size: u64,
        progress: &mut MigrationProgress,
    ) -> Result<()> {
        let static_file_provider = provider_factory.static_file_provider();
        let segments = [
            StaticFileSegment::Headers,
            StaticFileSegment::Transactions,
            StaticFileSegment::Receipts,
        ];

        for segment in segments {
            info!(target: "reth::cli", ?segment, "Migrating segment to static files");

            let highest_static =
                static_file_provider.get_highest_static_file_block(segment).unwrap_or(0);

            if highest_static >= to_block {
                info!(target: "reth::cli", ?segment, highest_static, "Segment already up to date");
                continue;
            }

            let start_block = highest_static.max(from_block);
            let mut current_block = start_block;

            while current_block <= to_block {
                let batch_end = (current_block + batch_size - 1).min(to_block);

                debug!(
                    target: "reth::cli",
                    ?segment,
                    from = current_block,
                    to = batch_end,
                    "Processing batch"
                );

                self.copy_segment_batch(provider_factory, segment, current_block, batch_end)?;

                progress.update(batch_end - current_block + 1);
                progress.set_phase(&format!("Static files: {:?}", segment));
                progress.log_progress();

                current_block = batch_end + 1;
            }
        }

        Ok(())
    }

    fn copy_segment_batch<N: CliNodeTypes<ChainSpec = C::ChainSpec>>(
        &self,
        provider_factory: &ProviderFactory<
            reth_node_builder::NodeTypesWithDBAdapter<N, Arc<DatabaseEnv>>,
        >,
        segment: StaticFileSegment,
        from_block: BlockNumber,
        to_block: BlockNumber,
    ) -> Result<()> {
        let provider = provider_factory.provider()?;
        let static_file_provider = provider_factory.static_file_provider();

        match segment {
            StaticFileSegment::Headers => {
                let mut writer = static_file_provider.latest_writer(segment)?;
                for block_num in from_block..=to_block {
                    if let Some(header) = provider.header_by_number(block_num)? {
                        let hash = provider
                            .block_hash(block_num)?
                            .ok_or_else(|| eyre::eyre!("Missing hash for block {}", block_num))?;
                        let td = provider
                            .header_td_by_number(block_num)?
                            .ok_or_else(|| eyre::eyre!("Missing TD for block {}", block_num))?;
                        writer.append_header(&header, td, &hash)?;
                    }
                }
                writer.commit()?;
            }
            StaticFileSegment::Transactions => {
                let mut writer = static_file_provider.latest_writer(segment)?;
                for block_num in from_block..=to_block {
                    if let Some(body) = provider.block_body_indices(block_num)? {
                        let txs = provider.transactions_by_tx_range(
                            body.first_tx_num..body.first_tx_num + body.tx_count as u64,
                        )?;
                        for (idx, tx) in txs.into_iter().enumerate() {
                            writer.append_transaction(body.first_tx_num + idx as u64, &tx)?;
                        }
                    }
                }
                writer.commit()?;
            }
            StaticFileSegment::Receipts => {
                let mut writer = static_file_provider.latest_writer(segment)?;
                for block_num in from_block..=to_block {
                    if let Some(receipts) = provider.receipts_by_block(block_num.into())? {
                        for receipt in receipts {
                            writer.append_receipt(block_num, &receipt)?;
                        }
                    }
                }
                writer.commit()?;
            }
            _ => {}
        }

        Ok(())
    }

    fn migrate_to_rocksdb<N: CliNodeTypes<ChainSpec = C::ChainSpec>>(
        &self,
        provider_factory: &ProviderFactory<
            reth_node_builder::NodeTypesWithDBAdapter<N, Arc<DatabaseEnv>>,
        >,
        batch_size: u64,
        progress: &mut MigrationProgress,
    ) -> Result<()> {
        let provider = provider_factory.provider()?;
        let rocksdb = provider_factory.rocksdb_provider();

        // Migrate TransactionHashNumbers
        info!(target: "reth::cli", "Migrating transaction hash -> number index to RocksDB");
        progress.set_phase("RocksDB: TxHashNumbers");

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
            if count % 100_000 == 0 {
                progress.log_progress();
            }
        }

        if !batch.is_empty() {
            rocksdb.insert_tx_hash_numbers_batch(&batch)?;
        }

        info!(target: "reth::cli", count, "Migrated transaction hash numbers");

        // Migrate AccountsHistory
        info!(target: "reth::cli", "Migrating accounts history to RocksDB");
        progress.set_phase("RocksDB: AccountsHistory");

        let mut cursor = tx.cursor_read::<tables::AccountsHistory>()?;
        count = 0;

        for result in cursor.walk(None)? {
            let (key, value) = result?;
            rocksdb.insert_account_history(key, value)?;

            count += 1;
            if count % batch_size == 0 {
                rocksdb.flush()?;
            }
            if count % 100_000 == 0 {
                progress.log_progress();
            }
        }
        rocksdb.flush()?;

        info!(target: "reth::cli", count, "Migrated accounts history entries");

        // Migrate StoragesHistory
        info!(target: "reth::cli", "Migrating storages history to RocksDB");
        progress.set_phase("RocksDB: StoragesHistory");

        let mut cursor = tx.cursor_read::<tables::StoragesHistory>()?;
        count = 0;

        for result in cursor.walk(None)? {
            let (key, value) = result?;
            rocksdb.insert_storage_history(key, value)?;

            count += 1;
            if count % batch_size == 0 {
                rocksdb.flush()?;
            }
            if count % 100_000 == 0 {
                progress.log_progress();
            }
        }
        rocksdb.flush()?;

        info!(target: "reth::cli", count, "Migrated storages history entries");

        Ok(())
    }

    fn verify_checksums<N: CliNodeTypes<ChainSpec = C::ChainSpec>>(
        &self,
        provider_factory: &ProviderFactory<
            reth_node_builder::NodeTypesWithDBAdapter<N, Arc<DatabaseEnv>>,
        >,
    ) -> Result<()> {
        let provider = provider_factory.provider()?;
        let static_file_provider = provider_factory.static_file_provider();

        // Verify static file segments against MDBX
        for segment in [
            StaticFileSegment::Headers,
            StaticFileSegment::Transactions,
            StaticFileSegment::Receipts,
        ] {
            info!(target: "reth::cli", ?segment, "Computing checksums");

            let sf_checksum = self.compute_static_file_checksum(&static_file_provider, segment)?;
            let mdbx_checksum = self.compute_mdbx_checksum_for_segment(&provider, segment)?;

            if sf_checksum != mdbx_checksum {
                return Err(eyre::eyre!(
                    "Checksum mismatch for {:?}: static_file={:#x}, mdbx={:#x}",
                    segment,
                    sf_checksum,
                    mdbx_checksum
                ));
            }

            info!(target: "reth::cli", ?segment, checksum = %format!("{:#x}", sf_checksum), "Checksum verified");
        }

        // Verify RocksDB tables
        let rocksdb = provider_factory.rocksdb_provider();
        let tx = provider.tx_ref();

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

        info!(target: "reth::cli", "All checksums verified successfully");
        Ok(())
    }

    fn compute_static_file_checksum<P: reth_provider::StaticFileProviderFactory>(
        &self,
        provider: &P::StaticFileProvider,
        segment: StaticFileSegment,
    ) -> Result<u64> {
        use reth_db::static_file::iter_static_files;

        let static_files = iter_static_files(provider.directory())?;
        let ranges = static_files
            .get(segment)
            .ok_or_else(|| eyre::eyre!("No static files found for segment: {}", segment))?;

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

    fn compute_mdbx_checksum_for_segment<TX: DbTx>(
        &self,
        provider: &impl DBProvider<Tx = TX>,
        segment: StaticFileSegment,
    ) -> Result<u64> {
        let tx = provider.tx_ref();
        let mut hasher = checksum_hasher();

        match segment {
            StaticFileSegment::Headers => {
                let mut cursor = tx.cursor_read::<RawTable<tables::Headers>>()?;
                for result in cursor.walk(None)? {
                    let (k, v): (RawKey<_>, RawValue<_>) = result?;
                    hasher.write(k.raw_key());
                    hasher.write(v.raw_value());
                }
            }
            StaticFileSegment::Transactions => {
                let mut cursor = tx.cursor_read::<RawTable<tables::Transactions>>()?;
                for result in cursor.walk(None)? {
                    let (k, v): (RawKey<_>, RawValue<_>) = result?;
                    hasher.write(k.raw_key());
                    hasher.write(v.raw_value());
                }
            }
            StaticFileSegment::Receipts => {
                let mut cursor = tx.cursor_read::<RawTable<tables::Receipts>>()?;
                for result in cursor.walk(None)? {
                    let (k, v): (RawKey<_>, RawValue<_>) = result?;
                    hasher.write(k.raw_key());
                    hasher.write(v.raw_value());
                }
            }
            _ => {}
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

    fn update_storage_settings<N: CliNodeTypes<ChainSpec = C::ChainSpec>>(
        &self,
        provider_factory: &ProviderFactory<
            reth_node_builder::NodeTypesWithDBAdapter<N, Arc<DatabaseEnv>>,
        >,
    ) -> Result<()> {
        use reth_provider::StorageSettings;

        let provider = provider_factory.provider_rw()?;

        let new_settings = StorageSettings::base()
            .with_receipts_in_static_files(true)
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
