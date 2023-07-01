//! clap [Args](clap::Args) for database configuration

use clap::Args;
use reth_interfaces::db::LogLevel;

/// Parameters for database configuration
#[derive(Debug, Args, PartialEq, Default, Clone, Copy)]
#[command(next_help_heading = "DATABASE")]
pub struct DatabaseArgs {
    /// Database logging level.
    #[arg(long = "db.log-level", value_enum)]
    pub log_level: Option<LogLevel>,
}
