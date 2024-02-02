//! Database debugging tool

use crate::{
    args::{
        utils::{chain_help, genesis_value_parser, SUPPORTED_CHAINS},
        DatabaseArgs,
    },
    dirs::{DataDirPath, MaybePlatformPath},
    utils::DbTool,
};
use clap::{Parser, Subcommand};
use reth_db::{
    mdbx::DatabaseArguments,
    open_db, open_db_read_only,
    version::{get_db_version, DatabaseVersionError, DB_VERSION},
};
use reth_primitives::ChainSpec;
use reth_provider::ProviderFactory;
use std::{
    io::{self, Write},
    sync::Arc,
};

mod clear;
mod diff;
mod get;
mod list;
mod snapshots;
mod stats;
/// DB List TUI
mod tui;

/// `reth db` command
#[derive(Debug, Parser)]
pub struct Command {
    /// The path to the data dir for all reth files and subdirectories.
    ///
    /// Defaults to the OS-specific data directory:
    ///
    /// - Linux: `$XDG_DATA_HOME/reth/` or `$HOME/.local/share/reth/`
    /// - Windows: `{FOLDERID_RoamingAppData}/reth/`
    /// - macOS: `$HOME/Library/Application Support/reth/`
    #[arg(long, value_name = "DATA_DIR", verbatim_doc_comment, default_value_t, global = true)]
    datadir: MaybePlatformPath<DataDirPath>,

    /// The chain this node is running.
    ///
    /// Possible values are either a built-in chain or the path to a chain specification file.
    #[arg(
        long,
        value_name = "CHAIN_OR_PATH",
        long_help = chain_help(),
        default_value = SUPPORTED_CHAINS[0],
        value_parser = genesis_value_parser,
        global = true,
    )]
    chain: Arc<ChainSpec>,

    #[clap(flatten)]
    db: DatabaseArgs,

    #[clap(subcommand)]
    command: Subcommands,
}

#[derive(Subcommand, Debug)]
/// `reth db` subcommands
pub enum Subcommands {
    /// Lists all the tables, their entry count and their size
    Stats(stats::Command),
    /// Lists the contents of a table
    List(list::Command),
    /// Create a diff between two database tables or two entire databases.
    Diff(diff::Command),
    /// Gets the content of a table for the given key
    Get(get::Command),
    /// Deletes all database entries
    Drop {
        /// Bypasses the interactive confirmation and drops the database directly
        #[arg(short, long)]
        force: bool,
    },
    /// Deletes all table entries
    Clear(clear::Command),
    /// Snapshots tables from database
    Snapshot(snapshots::Command),
    /// Lists current and local database versions
    Version,
    /// Returns the full database path
    Path,
}

impl Command {
    /// Execute `db` command
    pub async fn execute(self) -> eyre::Result<()> {
        // add network name to data dir
        let data_dir = self.datadir.unwrap_or_chain_default(self.chain.chain);
        let db_path = data_dir.db_path();

        match self.command {
            // TODO: We'll need to add this on the DB trait.
            Subcommands::Stats(command) => {
                let db = open_db_read_only(
                    &db_path,
                    DatabaseArguments::default().log_level(self.db.log_level),
                )?;
                let provider_factory =
                    ProviderFactory::new(db, self.chain.clone(), data_dir.snapshots_path())?;

                let tool = DbTool::new(provider_factory, self.chain.clone())?;
                command.execute(data_dir, &tool)?;
            }
            Subcommands::List(command) => {
                let db = open_db_read_only(
                    &db_path,
                    DatabaseArguments::default().log_level(self.db.log_level),
                )?;
                let provider_factory =
                    ProviderFactory::new(db, self.chain.clone(), data_dir.snapshots_path())?;

                let tool = DbTool::new(provider_factory, self.chain.clone())?;
                command.execute(&tool)?;
            }
            Subcommands::Diff(command) => {
                let db = open_db_read_only(
                    &db_path,
                    DatabaseArguments::default().log_level(self.db.log_level),
                )?;
                let provider_factory =
                    ProviderFactory::new(db, self.chain.clone(), data_dir.snapshots_path())?;

                let tool = DbTool::new(provider_factory, self.chain.clone())?;
                command.execute(&tool)?;
            }
            Subcommands::Get(command) => {
                let db = open_db_read_only(
                    &db_path,
                    DatabaseArguments::default().log_level(self.db.log_level),
                )?;
                let provider_factory =
                    ProviderFactory::new(db, self.chain.clone(), data_dir.snapshots_path())?;

                let tool = DbTool::new(provider_factory, self.chain.clone())?;
                command.execute(&tool)?;
            }
            Subcommands::Drop { force } => {
                if !force {
                    // Ask for confirmation
                    print!("Are you sure you want to drop the database at {db_path:?}? This cannot be undone. (y/N): ");
                    // Flush the buffer to ensure the message is printed immediately
                    io::stdout().flush().unwrap();

                    let mut input = String::new();
                    io::stdin().read_line(&mut input).expect("Failed to read line");

                    if !input.trim().eq_ignore_ascii_case("y") {
                        println!("Database drop aborted!");
                        return Ok(())
                    }
                }

                let db =
                    open_db(&db_path, DatabaseArguments::default().log_level(self.db.log_level))?;
                let provider_factory =
                    ProviderFactory::new(db, self.chain.clone(), data_dir.snapshots_path())?;

                let mut tool = DbTool::new(provider_factory, self.chain.clone())?;
                tool.drop(db_path)?;
            }
            Subcommands::Clear(command) => {
                let db =
                    open_db(&db_path, DatabaseArguments::default().log_level(self.db.log_level))?;
                let provider_factory =
                    ProviderFactory::new(db, self.chain.clone(), data_dir.snapshots_path())?;

                command.execute(provider_factory)?;
            }
            Subcommands::Snapshot(command) => {
                command.execute(data_dir, self.db.log_level, self.chain.clone())?;
            }
            Subcommands::Version => {
                let local_db_version = match get_db_version(&db_path) {
                    Ok(version) => Some(version),
                    Err(DatabaseVersionError::MissingFile) => None,
                    Err(err) => return Err(err.into()),
                };

                println!("Current database version: {DB_VERSION}");

                if let Some(version) = local_db_version {
                    println!("Local database version: {version}");
                } else {
                    println!("Local database is uninitialized");
                }
            }
            Subcommands::Path => {
                println!("{}", db_path.display());
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn parse_stats_globals() {
        let path = format!("../{}", SUPPORTED_CHAINS[0]);
        let cmd = Command::try_parse_from(["reth", "stats", "--datadir", &path]).unwrap();
        assert_eq!(cmd.datadir.as_ref(), Some(Path::new(&path)));
    }
}
