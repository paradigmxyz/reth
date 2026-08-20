//! clap [Args](clap::Args) for storage configuration

use alloy_eips::merge::EPOCH_SLOTS;
use clap::Args;
use std::sync::OnceLock;

/// Global static storage defaults
static STORAGE_DEFAULTS: OnceLock<DefaultStorageValues> = OnceLock::new();

/// Default values for storage that can be customized
///
/// Global defaults can be set via [`DefaultStorageValues::try_init`].
#[derive(Debug, Clone)]
pub struct DefaultStorageValues {
    v2: bool,
    historical_state_root_warning_threshold: u64,
}

impl DefaultStorageValues {
    /// Initialize the global storage defaults with this configuration
    pub fn try_init(self) -> Result<(), Self> {
        STORAGE_DEFAULTS.set(self)
    }

    /// Get a reference to the global storage defaults
    pub fn get_global() -> &'static Self {
        STORAGE_DEFAULTS.get_or_init(Self::default)
    }

    /// Set the default V2 storage layout flag
    pub const fn with_v2(mut self, v: bool) -> Self {
        self.v2 = v;
        self
    }

    /// Set the historical state root warning threshold.
    pub const fn with_historical_state_root_warning_threshold(mut self, threshold: u64) -> Self {
        self.historical_state_root_warning_threshold = threshold;
        self
    }
}

impl Default for DefaultStorageValues {
    fn default() -> Self {
        Self { v2: true, historical_state_root_warning_threshold: EPOCH_SLOTS }
    }
}

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
    #[arg(
        long = "storage.v2",
        default_value_t = DefaultStorageValues::get_global().v2,
        num_args = 0..=1,
        default_missing_value = "true",
    )]
    pub v2: bool,

    /// Number of blocks into the past after which historical state root calculation emits an OOM
    /// warning.
    #[arg(
        long = "storage.historical-state-root-warning-threshold",
        value_name = "BLOCKS",
        default_value_t = DefaultStorageValues::get_global().historical_state_root_warning_threshold,
    )]
    pub historical_state_root_warning_threshold: u64,
}

impl Default for StorageArgs {
    fn default() -> Self {
        let defaults = DefaultStorageValues::get_global();
        Self {
            v2: defaults.v2,
            historical_state_root_warning_threshold: defaults
                .historical_state_root_warning_threshold,
        }
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
        assert_eq!(args.historical_state_root_warning_threshold, EPOCH_SLOTS);
    }

    #[test]
    fn test_storage_v2_implicit_true() {
        let args = CommandParser::<StorageArgs>::parse_from(["reth", "--storage.v2"]).args;
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

    #[test]
    fn test_historical_state_root_warning_threshold() {
        let args = CommandParser::<StorageArgs>::parse_from([
            "reth",
            "--storage.historical-state-root-warning-threshold",
            "128",
        ])
        .args;
        assert_eq!(args.historical_state_root_warning_threshold, 128);
    }
}
