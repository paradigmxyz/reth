//! clap [Args](clap::Args) for storage mode configuration

use clap::{ArgAction, Args};

/// Parameters for storage mode configuration.
///
/// This controls whether the node uses v2 storage defaults (with RocksDB and static file
/// optimizations) or v1/legacy storage defaults.
#[derive(Debug, Args, PartialEq, Eq, Clone, Copy, Default)]
#[command(next_help_heading = "Storage")]
pub struct StorageArgs {
    /// Enable v2 storage defaults (static files + RocksDB routing).
    ///
    /// When enabled, the node uses optimized storage settings:
    /// - Receipts and transaction senders in static files
    /// - History indices in RocksDB (accounts, storages, transaction hashes)
    /// - Account and storage changesets in static files
    ///
    /// This is a genesis-initialization-only setting: changing it after genesis requires a
    /// re-sync.
    ///
    /// Individual settings can still be overridden with `--static-files.*` and `--rocksdb.*`
    /// flags.
    #[arg(long = "storage.v2", action = ArgAction::SetTrue)]
    pub v2: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Parser)]
    struct CommandParser {
        #[command(flatten)]
        args: StorageArgs,
    }

    #[test]
    fn test_default_storage_args() {
        let args = CommandParser::parse_from(["reth"]).args;
        assert!(!args.v2);
    }

    #[test]
    fn test_parse_v2_flag() {
        let args = CommandParser::parse_from(["reth", "--storage.v2"]).args;
        assert!(args.v2);
    }
}
