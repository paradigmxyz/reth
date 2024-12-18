//! clap [Args](clap::Args) for datadir config

use crate::dirs::{ChainPath, DataDirPath, MaybePlatformPath};
use clap::Args;
use reth_chainspec::Chain;
use std::path::PathBuf;

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
    #[arg(
        long = "datadir.static-files",
        alias = "datadir.static_files",
        value_name = "PATH",
        verbatim_doc_comment
    )]
    pub static_files_path: Option<PathBuf>,
}

impl DatadirArgs {
    /// Resolves the final datadir path.
    pub fn resolve_datadir(self, chain: Chain) -> ChainPath<DataDirPath> {
        let datadir = self.datadir.clone();
        datadir.unwrap_or_chain_default(chain, self)
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
    fn test_parse_datadir_args() {
        let default_args = DatadirArgs::default();
        let args = CommandParser::<DatadirArgs>::parse_from(["reth"]).args;
        assert_eq!(args, default_args);
    }
}
