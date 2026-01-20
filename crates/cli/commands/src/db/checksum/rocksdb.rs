//! RocksDB checksum implementation.

use super::{checksum_hasher, PROGRESS_LOG_INTERVAL};
use crate::common::CliNodeTypes;
use clap::ValueEnum;
use reth_chainspec::EthereumHardforks;
use reth_db::{tables, DatabaseEnv};
use reth_db_api::table::{Compress, Encode, Table};
use reth_db_common::DbTool;
use reth_node_builder::NodeTypesWithDBAdapter;
use reth_provider::RocksDBProviderFactory;
use std::{hash::Hasher, sync::Arc, time::Instant};
use tracing::info;

/// RocksDB tables that can be checksummed.
#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum RocksDbTable {
    /// Transaction hash to transaction number mapping
    TransactionHashNumbers,
    /// Account history indices
    AccountsHistory,
    /// Storage history indices
    StoragesHistory,
}

impl RocksDbTable {
    /// Returns the table name as a string
    const fn name(&self) -> &'static str {
        match self {
            Self::TransactionHashNumbers => tables::TransactionHashNumbers::NAME,
            Self::AccountsHistory => tables::AccountsHistory::NAME,
            Self::StoragesHistory => tables::StoragesHistory::NAME,
        }
    }
}

/// Computes a checksum for a RocksDB table.
pub fn checksum_rocksdb<N: CliNodeTypes<ChainSpec: EthereumHardforks>>(
    tool: &DbTool<NodeTypesWithDBAdapter<N, Arc<DatabaseEnv>>>,
    table: RocksDbTable,
    limit: Option<usize>,
) -> eyre::Result<()> {
    let rocksdb = tool.provider_factory.rocksdb_provider();

    let start_time = Instant::now();
    let limit = limit.unwrap_or(usize::MAX);

    info!(
        "Computing checksum for RocksDB table `{}`, limit={:?}",
        table.name(),
        if limit == usize::MAX { None } else { Some(limit) }
    );

    let (checksum, total) = match table {
        RocksDbTable::TransactionHashNumbers => {
            checksum_rocksdb_table::<tables::TransactionHashNumbers>(&rocksdb, limit)?
        }
        RocksDbTable::AccountsHistory => {
            checksum_rocksdb_table::<tables::AccountsHistory>(&rocksdb, limit)?
        }
        RocksDbTable::StoragesHistory => {
            checksum_rocksdb_table::<tables::StoragesHistory>(&rocksdb, limit)?
        }
    };

    let elapsed = start_time.elapsed();

    info!(
        "Checksum for RocksDB table `{}`: {:#x} ({} entries, elapsed: {:?})",
        table.name(),
        checksum,
        total,
        elapsed
    );

    Ok(())
}

/// Computes checksum for a specific RocksDB table by iterating over all entries.
fn checksum_rocksdb_table<T: Table>(
    rocksdb: &reth_provider::providers::RocksDBProvider,
    limit: usize,
) -> eyre::Result<(u64, usize)> {
    let iter = rocksdb.iter::<T>()?;
    let mut hasher = checksum_hasher();
    let mut total = 0usize;
    let mut compress_buf = Vec::new();

    for entry in iter {
        let (key, value) = entry?;

        let key_bytes = key.encode();
        hasher.write(key_bytes.as_ref());

        compress_buf.clear();
        value.compress_to_buf(&mut compress_buf);
        hasher.write(&compress_buf);

        total += 1;

        if total.is_multiple_of(PROGRESS_LOG_INTERVAL) {
            info!("Hashed {total} entries.");
        }

        if total >= limit {
            break;
        }
    }

    Ok((hasher.finish(), total))
}
