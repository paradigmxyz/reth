//! clap [Args](clap::Args) for datadir config

use crate::dirs::{ChainPath, DataDirPath, MaybePlatformPath, PlatformPath};
use clap::Args;
use reth_chainspec::Chain;
use std::path::PathBuf;

/// Parameters for datadir configuration
#[derive(Debug, Args, PartialEq, Eq, Default, Clone)]
#[command(next_help_heading = "Datadir")]
pub struct DatadirArgs {
    /// The path to the data dir for all reth files and subdirectories.
    ///
    /// IMPORTANT: This path will be used exactly as specified, without any platform-specific
    /// modifications. This ensures the --datadir argument is fully respected.
    ///
    /// Only if no path is provided, defaults to OS-specific data directory:
    /// - Linux: `$XDG_DATA_HOME/reth/` or `$HOME/.local/share/reth/`
    /// - Windows: `{FOLDERID_RoamingAppData}/reth/`
    /// - macOS: `$HOME/Library/Application Support/reth/`
    #[arg(long, value_name = "DATA_DIR", verbatim_doc_comment)]
    pub datadir: Option<PathBuf>,

    /// The absolute path to store static files in.
    /// If specified, this path will be used exactly as provided.
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
    /// 
    /// This implementation has been modified to fully respect user-provided paths:
    /// - If --datadir is provided, uses that path exactly as specified
    /// - Only falls back to platform-specific paths if no --datadir was provided
    /// 
    /// This ensures that users have full control over their data directory location
    /// while maintaining backwards compatibility with the default behavior.
    pub fn resolve_datadir(self, chain: Chain) -> ChainPath<DataDirPath> {
        match &self.datadir {
            // If --datadir was explicitly provided, use it directly without any platform-specific modifications
            Some(path) => {
                // Clone the path since we're borrowing it
                let platform_path = PlatformPath::new(path.clone());
                ChainPath::new(platform_path, chain, self)
            },
            // Only use platform-specific defaults if no path was provided
            None => {
                let default_path = MaybePlatformPath::<DataDirPath>::default();
                default_path.unwrap_or_chain_default(chain, self)
            }
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
    fn test_parse_datadir_args() {
        let default_args = DatadirArgs::default();
        let args = CommandParser::<DatadirArgs>::parse_from(["reth"]).args;
        assert_eq!(args, default_args);
    }
}
