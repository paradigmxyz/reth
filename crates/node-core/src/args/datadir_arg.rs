//! clap [Args](clap::Args) for datadir config

use clap::Args;
use std::path::PathBuf;

/// Parameters for database configuration
#[derive(Debug, Args, PartialEq, Eq, Default, Clone)]
#[command(next_help_heading = "Datadir")]
pub struct DatadirArgs {
    /// Set the absolute path to static files
    #[arg(long = "datadir.static_files", help_heading = "Datadir", value_name = "PATH")]
    pub static_files_path: Option<PathBuf>,
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
    fn test_parse_datadir_args() {
        let default_args = DatadirArgs::default();
        let args = CommandParser::<DatadirArgs>::parse_from(["reth"]).args;
        assert_eq!(args, default_args);
    }
}
