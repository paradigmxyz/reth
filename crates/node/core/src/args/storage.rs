//! clap [Args](clap::Args) for storage configuration

use clap::Args;

/// Parameters for storage configuration.
///
/// `--storage.v2` controls whether new databases use the hot/cold V2 storage layout.
/// Defaults to `true`.
///
/// Existing databases always use the settings persisted in their metadata.
#[derive(Debug, Args, PartialEq, Eq, Clone, Copy)]
#[command(next_help_heading = "Storage")]
pub struct StorageArgs {
    /// Enable V2 (hot/cold) storage layout for new databases.
    ///
    /// When set, new databases will be initialized with the V2 storage layout that
    /// separates hot and cold data. Existing databases always use the settings
    /// persisted in their metadata regardless of this flag.
    #[arg(long = "storage.v2", default_value = "true", action = clap::ArgAction::Set)]
    pub v2: bool,
}

impl Default for StorageArgs {
    fn default() -> Self {
        Self { v2: true }
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
        let args = CommandParser::<StorageArgs>::parse_from(["reth"]).args;
        assert!(args.v2);
    }

    #[test]
    fn test_storage_v2_explicit_true() {
        let args = CommandParser::<StorageArgs>::parse_from(["reth", "--storage.v2=true"]).args;
        assert!(args.v2);
    }

    #[test]
    fn test_storage_v2_explicit_false() {
        let args = CommandParser::<StorageArgs>::parse_from(["reth", "--storage.v2=false"]).args;
        assert!(!args.v2);
    }
}
