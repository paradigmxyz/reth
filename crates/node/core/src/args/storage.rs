//! clap [Args](clap::Args) for storage configuration

use clap::{ArgAction, Args};

/// Parameters for storage configuration.
///
/// This controls whether the node uses v2 storage defaults (with `RocksDB` and static file
/// optimizations) or v1/legacy storage defaults.
///
/// Individual storage settings can be overridden with `--static-files.*` and `--rocksdb.*` flags.
#[derive(Debug, Args, PartialEq, Eq, Clone, Copy)]
#[command(next_help_heading = "Storage")]
pub struct StorageArgs {
    /// Enable v2 storage defaults (static files + `RocksDB` routing).
    ///
    /// When enabled, the node uses optimized storage settings:
    /// - Receipts and transaction senders in static files
    /// - History indices in `RocksDB` (accounts, storages, transaction hashes)
    /// - Account and storage changesets in static files
    ///
    /// This is a genesis-initialization-only setting: changing it after genesis requires a
    /// re-sync.
    ///
    /// Individual settings can still be overridden with `--static-files.*` and `--rocksdb.*`
    /// flags.
    #[arg(long = "storage.v2", action = ArgAction::SetTrue)]
    pub v2: bool,

    /// Enable in-memory bloom filter for short-circuiting empty storage slot reads.
    ///
    /// When enabled, the node builds a bloom filter over all `(address, slot)` pairs on startup
    /// and checks it before every MDBX storage seek. If the filter says a slot was never
    /// written, the seek is skipped entirely. This benefits the 30-40% of SLOAD operations
    /// that target empty slots.
    #[arg(long = "storage.bloom-filter", action = ArgAction::SetTrue)]
    pub bloom_filter: bool,

    /// Bloom filter size in megabytes (default: 1024, minimum: 1).
    ///
    /// Larger filters have fewer false positives but use more RAM.
    /// With K=3 hash functions, a 1024 MB filter supports ~700M entries at <1% FP rate,
    /// covering mainnet's ~500M non-zero storage slots with room to grow.
    #[arg(
        long = "storage.bloom-filter-size-mb",
        default_value_t = 1024,
        value_parser = clap::value_parser!(u64).range(1..),
    )]
    pub bloom_filter_size_mb: u64,
}

impl Default for StorageArgs {
    fn default() -> Self {
        Self { v2: false, bloom_filter: false, bloom_filter_size_mb: 1024 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    /// A helper type to parse Args more easily
    #[derive(Parser)]
    struct CommandParser<T: Args> {
        #[command(flatten)]
        args: T,
    }

    #[test]
    fn test_default_storage_args() {
        let default_args = StorageArgs::default();
        let args = CommandParser::<StorageArgs>::parse_from(["reth"]).args;
        assert_eq!(args, default_args);
        assert!(!args.v2);
        assert!(!args.bloom_filter);
        assert_eq!(args.bloom_filter_size_mb, 1024);
    }

    #[test]
    fn test_parse_v2_flag() {
        let args = CommandParser::<StorageArgs>::parse_from(["reth", "--storage.v2"]).args;
        assert!(args.v2);
    }

    #[test]
    fn test_parse_bloom_filter_flags() {
        let args = CommandParser::<StorageArgs>::parse_from([
            "reth",
            "--storage.bloom-filter",
            "--storage.bloom-filter-size-mb",
            "128",
        ])
        .args;
        assert!(args.bloom_filter);
        assert_eq!(args.bloom_filter_size_mb, 128);
    }

    #[test]
    fn test_bloom_filter_size_zero_rejected() {
        let result = CommandParser::<StorageArgs>::try_parse_from([
            "reth",
            "--storage.bloom-filter",
            "--storage.bloom-filter-size-mb",
            "0",
        ]);
        assert!(result.is_err(), "size 0 should be rejected by value_parser");
    }
}
