use crate::common::{AccessRights, CliNodeTypes, Environment, EnvironmentArgs};
use clap::{Parser, Subcommand};
use reth_chainspec::{EthChainSpec, EthereumHardforks};
use reth_cli::chainspec::ChainSpecParser;
use reth_cli_runner::CliContext;
use reth_db::version::{get_db_version, DatabaseVersionError, DB_VERSION};
use reth_db_common::DbTool;
use std::{
    io::{self, Write},
    sync::Arc,
};
mod account_storage;
mod checksum;
mod clear;
mod copy;
mod diff;
mod get;
mod list;
mod prune_checkpoints;
mod repair_trie;
mod settings;
mod stage_checkpoints;
mod state;
mod static_file_header;
mod stats;
/// DB List TUI
mod tui;

/// `reth db` command
#[derive(Debug, Parser)]
pub struct Command<C: ChainSpecParser> {
    #[command(flatten)]
    env: EnvironmentArgs<C>,

    #[command(subcommand)]
    command: Subcommands,
}

#[derive(Subcommand, Debug)]
/// `reth db` subcommands
pub enum Subcommands {
    /// Lists all the tables, their entry count and their size
    Stats(stats::Command),
    /// Lists the contents of a table
    List(list::Command),
    /// Calculates the content checksum of a table or static file segment
    Checksum(checksum::Command),
    /// Copies the MDBX database to a new location (bundled mdbx_copy)
    Copy(copy::Command),
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
    /// Verifies trie consistency and outputs any inconsistencies
    RepairTrie(repair_trie::Command),
    /// Reads and displays the static file segment header
    StaticFileHeader(static_file_header::Command),
    /// Lists current and local database versions
    Version,
    /// Returns the full database path
    Path,
    /// Manage storage settings
    Settings(settings::Command),
    /// View or set prune checkpoints
    PruneCheckpoints(prune_checkpoints::Command),
    // View or set stage checkpoints
    StageCheckpoints(stage_checkpoints::Command),
    /// Gets storage size information for an account
    AccountStorage(account_storage::Command),
    /// Gets account state and storage at a specific block
    State(state::Command),
}

impl<C: ChainSpecParser<ChainSpec: EthChainSpec + EthereumHardforks>> Command<C> {
    /// Execute `db` command
    pub async fn execute<N: CliNodeTypes<ChainSpec = C::ChainSpec>>(
        self,
        ctx: CliContext,
    ) -> eyre::Result<()> {
        /// Initializes a provider factory with specified access rights, and then executes the
        /// provided command.
        macro_rules! db_exec {
            ($env:expr, $tool:ident, $N:ident, $access_rights:expr, $command:block) => {
                let Environment { provider_factory, .. } =
                    $env.init::<$N>($access_rights, ctx.task_executor.clone())?;

                let $tool = DbTool::new(provider_factory)?;
                $command;
            };
        }

        let data_dir = self.env.datadir.clone().resolve_datadir(self.env.chain.chain());
        let db_path = data_dir.db();
        let static_files_path = data_dir.static_files();
        let exex_wal_path = data_dir.exex_wal();

        // ensure the provided datadir exist
        eyre::ensure!(
            data_dir.data_dir().is_dir(),
            "Datadir does not exist: {:?}",
            data_dir.data_dir()
        );

        // ensure the provided database exist
        eyre::ensure!(db_path.is_dir(), "Database does not exist: {:?}", db_path);

        match self.command {
            // TODO: We'll need to add this on the DB trait.
            Subcommands::Stats(command) => {
                let access_rights = if command.skip_consistency_checks {
                    AccessRights::RoInconsistent
                } else {
                    AccessRights::RO
                };
                db_exec!(self.env, tool, N, access_rights, {
                    command.execute(data_dir, &tool)?;
                });
            }
            Subcommands::List(command) => {
                db_exec!(self.env, tool, N, AccessRights::RO, {
                    command.execute(&tool)?;
                });
            }
            Subcommands::Checksum(command) => {
                db_exec!(self.env, tool, N, AccessRights::RO, {
                    command.execute(&tool)?;
                });
            }
            Subcommands::Copy(command) => {
                db_exec!(self.env, tool, N, AccessRights::RO, {
                    command.execute(tool.provider_factory.db_ref())?;
                });
            }
            Subcommands::Diff(command) => {
                db_exec!(self.env, tool, N, AccessRights::RO, {
                    command.execute(&tool)?;
                });
            }
            Subcommands::Get(command) => {
                db_exec!(self.env, tool, N, AccessRights::RO, {
                    command.execute(&tool)?;
                });
            }
            Subcommands::Drop { force } => {
                if !force {
                    // Ask for confirmation
                    print!(
                        "Are you sure you want to drop the database at {data_dir}? This cannot be undone. (y/N): "
                    );
                    // Flush the buffer to ensure the message is printed immediately
                    io::stdout().flush().unwrap();

                    let mut input = String::new();
                    io::stdin().read_line(&mut input).expect("Failed to read line");

                    if !input.trim().eq_ignore_ascii_case("y") {
                        println!("Database drop aborted!");
                        return Ok(())
                    }
                }

                db_exec!(self.env, tool, N, AccessRights::RW, {
                    tool.drop(db_path, static_files_path, exex_wal_path)?;
                });
            }
            Subcommands::Clear(command) => {
                db_exec!(self.env, tool, N, AccessRights::RW, {
                    command.execute(&tool)?;
                });
            }
            Subcommands::RepairTrie(command) => {
                let access_rights =
                    if command.dry_run { AccessRights::RO } else { AccessRights::RW };
                db_exec!(self.env, tool, N, access_rights, {
                    command.execute(&tool, ctx.task_executor, &data_dir)?;
                });
            }
            Subcommands::StaticFileHeader(command) => {
                db_exec!(self.env, tool, N, AccessRights::RoInconsistent, {
                    command.execute(&tool)?;
                });
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
            Subcommands::Settings(command) => {
                db_exec!(self.env, tool, N, command.access_rights(), {
                    command.execute(&tool)?;
                });
            }
            Subcommands::PruneCheckpoints(command) => {
                db_exec!(self.env, tool, N, command.access_rights(), {
                    command.execute(&tool)?;
                });
            }
            Subcommands::StageCheckpoints(command) => {
                db_exec!(self.env, tool, N, command.access_rights(), {
                    command.execute(&tool)?;
                });
            }
            Subcommands::AccountStorage(command) => {
                db_exec!(self.env, tool, N, AccessRights::RO, {
                    command.execute(&tool)?;
                });
            }
            Subcommands::State(command) => {
                db_exec!(self.env, tool, N, AccessRights::RO, {
                    command.execute(&tool)?;
                });
            }
        }

        Ok(())
    }
}

impl<C: ChainSpecParser> Command<C> {
    /// Returns the underlying chain being used to run this command
    pub fn chain_spec(&self) -> Option<&Arc<C::ChainSpec>> {
        Some(&self.env.chain)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reth_ethereum_cli::chainspec::{EthereumChainSpecParser, SUPPORTED_CHAINS};
    use std::path::Path;

    #[test]
    fn parse_stats_globals() {
        let path = format!("../{}", SUPPORTED_CHAINS[0]);
        let cmd = Command::<EthereumChainSpecParser>::try_parse_from([
            "reth",
            "--datadir",
            &path,
            "stats",
        ])
        .unwrap();
        assert_eq!(cmd.env.datadir.resolve_datadir(cmd.env.chain.chain).as_ref(), Path::new(&path));
    }
}
