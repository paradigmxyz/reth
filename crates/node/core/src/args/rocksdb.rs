//! clap [Args](clap::Args) for `RocksDB` configuration

use clap::Args;

/// Parameters for `RocksDB` configuration.
#[derive(Debug, Args, PartialEq, Eq, Clone, Copy, Default)]
#[command(next_help_heading = "RocksDB")]
pub struct RocksDbArgs {}
