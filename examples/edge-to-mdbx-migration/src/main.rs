//! Edge to MDBX Migration Tool
//!
//! This example demonstrates how to migrate data from an edge node (using RocksDB + static files)
//! to a standard MDBX-based reth node. It copies:
//!
//! - `AccountsHistory` from RocksDB → MDBX
//! - `StoragesHistory` from RocksDB → MDBX
//! - `AccountChangeSets` from static files → MDBX
//! - `StorageChangeSets` from static files → MDBX
//! - `TransactionHashNumbers` from RocksDB → MDBX
//!
//! The migration is done in batches to avoid OOM issues.
//!
//! # Usage
//!
//! ```bash
//! cargo run --example edge-to-mdbx-migration -- \
//!     --datadir /path/to/edge/datadir
//! ```
//!
//! # Platform Support
//!
//! This example only works on Unix platforms (Linux, macOS) because RocksDB support
//! is Unix-only in reth.

#![warn(unused_crate_dependencies)]
#![cfg(unix)]

use alloy_primitives::BlockNumber;
use clap::Parser;
use eyre::Result;
use reth_db::mdbx::{open_db, DatabaseArguments};
use reth_db_api::{
    cursor::{DbCursorRW, DbDupCursorRW},
    models::BlockNumberAddress,
    table::Table,
    tables,
    transaction::DbTxMut,
};
use reth_ethereum::{chainspec::ChainSpecBuilder, node::EthereumNode};
use reth_provider::{
    providers::{ProviderNodeTypes, ReadOnlyConfig, RocksDBProvider, StaticFileProvider},
    DatabaseProviderFactory, RocksDBProviderFactory, StaticFileProviderFactory,
};
use reth_stages as _;
use reth_static_file_types::StaticFileSegment;
use reth_storage_api::{ChangeSetReader, DBProvider, MetadataProvider, StorageChangeSetReader};
use std::{path::PathBuf, sync::Arc};
use tracing::{info, warn};

/// Batch size for history index migrations (number of entries per batch)
const HISTORY_BATCH_SIZE: usize = 100_000;

/// Batch size for changeset migrations (number of blocks per batch)
const CHANGESET_BLOCK_BATCH_SIZE: u64 = 10_000;

/// Batch size for transaction hash migrations (number of entries per batch)
const TX_HASH_BATCH_SIZE: usize = 500_000;

#[derive(Parser, Debug)]
#[command(name = "edge-to-mdbx-migration")]
#[command(about = "Migrate edge node data (RocksDB + static files) to MDBX tables")]
struct Args {
    /// Path to the reth datadir (must be an edge node)
    #[arg(long)]
    datadir: PathBuf,

    /// Skip migrating AccountsHistory
    #[arg(long, default_value = "false")]
    skip_account_history: bool,

    /// Skip migrating StoragesHistory
    #[arg(long, default_value = "false")]
    skip_storage_history: bool,

    /// Skip migrating AccountChangeSets
    #[arg(long, default_value = "false")]
    skip_account_changesets: bool,

    /// Skip migrating StorageChangeSets
    #[arg(long, default_value = "false")]
    skip_storage_changesets: bool,

    /// Skip migrating TransactionHashNumbers
    #[arg(long, default_value = "false")]
    skip_tx_hashes: bool,

    /// Dry run - don't actually write to MDBX
    #[arg(long, default_value = "false")]
    dry_run: bool,
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let args = Args::parse();

    info!(datadir = %args.datadir.display(), "Starting edge to MDBX migration");

    let spec = Arc::new(ChainSpecBuilder::mainnet().build());
    let config = ReadOnlyConfig::from_datadir(&args.datadir);

    // Open database in read-write mode
    let db = Arc::new(open_db(&config.db_dir, DatabaseArguments::new(Default::default()))?);

    // Build provider factory with read-write access
    let factory = EthereumNode::provider_factory_builder()
        .db(db)
        .chainspec(spec)
        .static_file(StaticFileProvider::read_write(config.static_files_dir)?)
        .rocksdb_provider(
            RocksDBProvider::builder(&config.rocksdb_dir).with_default_tables().build()?,
        )
        .build_provider_factory()?;

    let storage_settings = factory.storage_settings()?.unwrap_or_default();
    info!(?storage_settings, "Detected storage settings");

    if !storage_settings.any_in_rocksdb() &&
        !storage_settings.account_changesets_in_static_files &&
        !storage_settings.storage_changesets_in_static_files
    {
        warn!("This datadir does not appear to be an edge node (no RocksDB tables or static file changesets configured)");
        return Ok(());
    }

    if !args.skip_account_history && storage_settings.account_history_in_rocksdb {
        migrate_account_history(&factory, args.dry_run)?;
    }

    if !args.skip_storage_history && storage_settings.storages_history_in_rocksdb {
        migrate_storage_history(&factory, args.dry_run)?;
    }

    if !args.skip_account_changesets && storage_settings.account_changesets_in_static_files {
        migrate_account_changesets(&factory, args.dry_run)?;
    }

    if !args.skip_storage_changesets && storage_settings.storage_changesets_in_static_files {
        migrate_storage_changesets(&factory, args.dry_run)?;
    }

    if !args.skip_tx_hashes && storage_settings.transaction_hash_numbers_in_rocksdb {
        migrate_tx_hash_numbers(&factory, args.dry_run)?;
    }

    info!("Migration complete!");
    Ok(())
}

/// Migrate AccountsHistory from RocksDB to MDBX
fn migrate_account_history<N>(
    factory: &reth_provider::ProviderFactory<N>,
    dry_run: bool,
) -> Result<()>
where
    N: ProviderNodeTypes,
{
    info!("Migrating AccountsHistory from RocksDB to MDBX...");

    let rocksdb = factory.rocksdb_provider();
    let mut total_migrated = 0u64;
    let mut batch = Vec::with_capacity(HISTORY_BATCH_SIZE);

    for result in rocksdb.iter::<tables::AccountsHistory>()? {
        let (key, value) = result?;
        batch.push((key, value));

        if batch.len() >= HISTORY_BATCH_SIZE {
            if !dry_run {
                write_batch_to_mdbx::<N, tables::AccountsHistory>(factory, &batch)?;
            }
            total_migrated += batch.len() as u64;
            info!(total_migrated, "AccountsHistory batch written");
            batch.clear();
        }
    }

    if !batch.is_empty() {
        if !dry_run {
            write_batch_to_mdbx::<N, tables::AccountsHistory>(factory, &batch)?;
        }
        total_migrated += batch.len() as u64;
    }

    info!(total_migrated, "AccountsHistory migration complete");
    Ok(())
}

/// Migrate StoragesHistory from RocksDB to MDBX
fn migrate_storage_history<N>(
    factory: &reth_provider::ProviderFactory<N>,
    dry_run: bool,
) -> Result<()>
where
    N: ProviderNodeTypes,
{
    info!("Migrating StoragesHistory from RocksDB to MDBX...");

    let rocksdb = factory.rocksdb_provider();
    let mut total_migrated = 0u64;
    let mut batch = Vec::with_capacity(HISTORY_BATCH_SIZE);

    for result in rocksdb.iter::<tables::StoragesHistory>()? {
        let (key, value) = result?;
        batch.push((key, value));

        if batch.len() >= HISTORY_BATCH_SIZE {
            if !dry_run {
                write_batch_to_mdbx::<N, tables::StoragesHistory>(factory, &batch)?;
            }
            total_migrated += batch.len() as u64;
            info!(total_migrated, "StoragesHistory batch written");
            batch.clear();
        }
    }

    if !batch.is_empty() {
        if !dry_run {
            write_batch_to_mdbx::<N, tables::StoragesHistory>(factory, &batch)?;
        }
        total_migrated += batch.len() as u64;
    }

    info!(total_migrated, "StoragesHistory migration complete");
    Ok(())
}

/// Migrate TransactionHashNumbers from RocksDB to MDBX
fn migrate_tx_hash_numbers<N>(
    factory: &reth_provider::ProviderFactory<N>,
    dry_run: bool,
) -> Result<()>
where
    N: ProviderNodeTypes,
{
    info!("Migrating TransactionHashNumbers from RocksDB to MDBX...");

    let rocksdb = factory.rocksdb_provider();
    let mut total_migrated = 0u64;
    let mut batch = Vec::with_capacity(TX_HASH_BATCH_SIZE);

    for result in rocksdb.iter::<tables::TransactionHashNumbers>()? {
        let (key, value) = result?;
        batch.push((key, value));

        if batch.len() >= TX_HASH_BATCH_SIZE {
            if !dry_run {
                write_batch_to_mdbx::<N, tables::TransactionHashNumbers>(factory, &batch)?;
            }
            total_migrated += batch.len() as u64;
            info!(total_migrated, "TransactionHashNumbers batch written");
            batch.clear();
        }
    }

    if !batch.is_empty() {
        if !dry_run {
            write_batch_to_mdbx::<N, tables::TransactionHashNumbers>(factory, &batch)?;
        }
        total_migrated += batch.len() as u64;
    }

    info!(total_migrated, "TransactionHashNumbers migration complete");
    Ok(())
}

/// Migrate AccountChangeSets from static files to MDBX
fn migrate_account_changesets<N>(
    factory: &reth_provider::ProviderFactory<N>,
    dry_run: bool,
) -> Result<()>
where
    N: ProviderNodeTypes,
{
    info!("Migrating AccountChangeSets from static files to MDBX...");

    let sf_provider = factory.static_file_provider();
    let highest_block = sf_provider
        .get_highest_static_file_block(StaticFileSegment::AccountChangeSets)
        .unwrap_or(0);

    info!(highest_block, "Found highest block with account changesets");

    let mut total_migrated = 0u64;
    let mut current_block = 0u64;

    while current_block <= highest_block {
        let end_block = (current_block + CHANGESET_BLOCK_BATCH_SIZE).min(highest_block);
        let mut batch = Vec::new();

        for block_num in current_block..=end_block {
            match sf_provider.account_block_changeset(block_num) {
                Ok(changesets) => {
                    for cs in changesets {
                        batch.push((block_num, cs));
                    }
                }
                Err(e) => {
                    warn!(block_num, ?e, "Failed to read account changeset for block");
                }
            }
        }

        if !batch.is_empty() && !dry_run {
            write_account_changesets_to_mdbx(factory, &batch)?;
        }

        total_migrated += batch.len() as u64;
        info!(
            current_block,
            end_block,
            batch_size = batch.len(),
            total_migrated,
            "AccountChangeSets batch written"
        );

        current_block = end_block + 1;
    }

    info!(total_migrated, "AccountChangeSets migration complete");
    Ok(())
}

/// Migrate StorageChangeSets from static files to MDBX
fn migrate_storage_changesets<N>(
    factory: &reth_provider::ProviderFactory<N>,
    dry_run: bool,
) -> Result<()>
where
    N: ProviderNodeTypes,
{
    info!("Migrating StorageChangeSets from static files to MDBX...");

    let sf_provider = factory.static_file_provider();
    let highest_block = sf_provider
        .get_highest_static_file_block(StaticFileSegment::StorageChangeSets)
        .unwrap_or(0);

    info!(highest_block, "Found highest block with storage changesets");

    let mut total_migrated = 0u64;
    let mut current_block = 0u64;

    while current_block <= highest_block {
        let end_block = (current_block + CHANGESET_BLOCK_BATCH_SIZE).min(highest_block);
        let mut batch = Vec::new();

        for block_num in current_block..=end_block {
            match sf_provider.storage_changeset(block_num) {
                Ok(changesets) => {
                    batch.extend(changesets);
                }
                Err(e) => {
                    warn!(block_num, ?e, "Failed to read storage changeset for block");
                }
            }
        }

        if !batch.is_empty() && !dry_run {
            write_storage_changesets_to_mdbx(factory, &batch)?;
        }

        total_migrated += batch.len() as u64;
        info!(
            current_block,
            end_block,
            batch_size = batch.len(),
            total_migrated,
            "StorageChangeSets batch written"
        );

        current_block = end_block + 1;
    }

    info!(total_migrated, "StorageChangeSets migration complete");
    Ok(())
}

/// Write a batch of key-value pairs to an MDBX table
fn write_batch_to_mdbx<N, T>(
    factory: &reth_provider::ProviderFactory<N>,
    batch: &[(T::Key, T::Value)],
) -> Result<()>
where
    N: ProviderNodeTypes,
    T: Table,
    T::Key: Clone,
    T::Value: Clone,
{
    let provider = factory.database_provider_rw()?;
    {
        let mut cursor = provider.tx_ref().cursor_write::<T>()?;
        for (key, value) in batch {
            cursor.insert(key.clone(), &value.clone())?;
        }
    }
    provider.commit()?;
    Ok(())
}

/// Write account changesets to MDBX (DUPSORT table)
fn write_account_changesets_to_mdbx<N>(
    factory: &reth_provider::ProviderFactory<N>,
    batch: &[(BlockNumber, reth_db::models::AccountBeforeTx)],
) -> Result<()>
where
    N: ProviderNodeTypes,
{
    let provider = factory.database_provider_rw()?;
    {
        let mut cursor = provider.tx_ref().cursor_dup_write::<tables::AccountChangeSets>()?;
        for (block_num, changeset) in batch {
            cursor.append_dup(*block_num, changeset.clone())?;
        }
    }
    provider.commit()?;
    Ok(())
}

/// Write storage changesets to MDBX (DUPSORT table)
fn write_storage_changesets_to_mdbx<N>(
    factory: &reth_provider::ProviderFactory<N>,
    batch: &[(BlockNumberAddress, reth_primitives_traits::StorageEntry)],
) -> Result<()>
where
    N: ProviderNodeTypes,
{
    let provider = factory.database_provider_rw()?;
    {
        let mut cursor = provider.tx_ref().cursor_dup_write::<tables::StorageChangeSets>()?;
        for (block_addr, entry) in batch {
            cursor.append_dup(*block_addr, *entry)?;
        }
    }
    provider.commit()?;
    Ok(())
}

#[cfg(not(unix))]
fn main() {
    eprintln!("This example only works on Unix platforms (Linux, macOS).");
    eprintln!("RocksDB support in reth is Unix-only.");
    std::process::exit(1);
}
