//! clap [Args](clap::Args) for datadir config

use clap::Args;
use std::path::PathBuf;

use crate::dirs::{DataDirPath, MaybePlatformPath};

/// Parameters for datadir configuration
#[derive(Debug, Args, PartialEq, Eq, Default, Clone)]
#[command(next_help_heading = "Datadir")]
pub struct DatadirArgs {
    /// The path to the data dir for all reth files and subdirectories.
    ///
    /// Defaults to the OS-specific data directory:
    ///
    /// - Linux: `$XDG_DATA_HOME/reth/` or `$HOME/.local/share/reth/`
    /// - Windows: `{FOLDERID_RoamingAppData}/reth/`
    /// - macOS: `$HOME/Library/Application Support/reth/`
    #[arg(long, value_name = "DATA_DIR", verbatim_doc_comment, default_value_t)]
    pub datadir: MaybePlatformPath<DataDirPath>,

    /// The absolute path to store static files in.
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
