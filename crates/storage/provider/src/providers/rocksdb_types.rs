//! Shared types for `RocksDB` provider.
//!
//! These types are shared between the real `RocksDB` provider and the stub implementation
//! to ensure consistent APIs across platforms.

/// Statistics for a single `RocksDB` table (column family).
#[derive(Debug, Clone)]
pub struct RocksDBTableStats {
    /// Size of SST files on disk in bytes.
    pub sst_size_bytes: u64,
    /// Size of memtables in memory in bytes.
    pub memtable_size_bytes: u64,
    /// Name of the table/column family.
    pub name: String,
    /// Estimated number of keys in the table.
    pub estimated_num_keys: u64,
    /// Estimated size of live data in bytes (SST files + memtables).
    pub estimated_size_bytes: u64,
    /// Estimated bytes pending compaction (reclaimable space).
    pub pending_compaction_bytes: u64,
}

/// Database-level statistics for `RocksDB`.
///
/// Contains both per-table statistics and DB-level metrics like WAL size.
#[derive(Debug, Clone)]
pub struct RocksDBStats {
    /// Statistics for each table (column family).
    pub tables: Vec<RocksDBTableStats>,
    /// Total size of WAL (Write-Ahead Log) files in bytes.
    ///
    /// WAL is shared across all tables and not included in per-table metrics.
    pub wal_size_bytes: u64,
}
