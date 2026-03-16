//! clap [Args](clap::Args) for storage configuration

use clap::{ArgAction, Args};

/// Parameters for storage configuration.
///
/// V2 storage is now the default for all new databases. The `--storage.v2` flag is
/// accepted for backwards compatibility but has no effect — v2 is always used.
///
/// Existing databases always use the settings persisted in their metadata.
///
/// Individual storage settings can be overridden with `--static-files.*` and `--rocksdb.*` flags.
#[derive(Debug, Args, PartialEq, Eq, Clone, Copy, Default)]
#[command(next_help_heading = "Storage")]
pub struct StorageArgs {
    /// Deprecated no-op: v2 storage is now always enabled for new databases.
    ///
    /// Kept for backwards compatibility with existing scripts and configurations.
    /// Existing databases always use the settings persisted in their metadata.
    #[arg(long = "storage.v2", action = ArgAction::SetTrue, hide = true)]
    pub v2: bool,
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
        let args = CommandParser::<StorageArgs>::parse_from(["reth"]).args;
        assert_eq!(args, StorageArgs::default());
    }

    #[test]
    fn test_parse_v2_flag_accepted() {
        // Flag is accepted for backwards compatibility but is a no-op
        let args = CommandParser::<StorageArgs>::parse_from(["reth", "--storage.v2"]).args;
        assert!(args.v2);
    }
}
