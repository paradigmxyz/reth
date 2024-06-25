//! Database debugging tool

use crate::{
    commands::common::{AccessRights, Environment, EnvironmentArgs},
    utils::DbTool,
};
use clap::{Parser, Subcommand};
use reth_db::version::{get_db_version, DatabaseVersionError, DB_VERSION};
use std::io::{self, Write};

mod checksum;
mod clear;
mod diff;
mod get;
mod list;
mod stats;
/// DB List TUI
mod tui;

/// `reth db` command
#[derive(Debug, Parser)]
pub struct Command {
    #[command(flatten)]
    env: EnvironmentArgs,

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
    /// Calculates the content checksum of a table
    Checksum(checksum::Command),
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
    /// Lists current and local database versions
    Version,
    /// Returns the full database path
    Path,
}

/// `db_ro_exec` opens a database in read-only mode, and then execute with the provided command
macro_rules! db_ro_exec {
    ($env:expr, $tool:ident, $command:block) => {
        let Environment { provider_factory, .. } = $env.init(AccessRights::RO)?;

        let $tool = DbTool::new(provider_factory.clone())?;
        $command;
    };
}

impl Command {
    /// Execute `db` command
    pub async fn execute(self) -> eyre::Result<()> {
        let data_dir = self.env.datadir.clone().resolve_datadir(self.env.chain.chain);
        let db_path = data_dir.db();
        let static_files_path = data_dir.static_files();

        match self.command {
            // TODO: We'll need to add this on the DB trait.
            Subcommands::Stats(command) => {
                db_ro_exec!(self.env, tool, {
                    command.execute(data_dir, &tool)?;
                });
            }
            Subcommands::List(command) => {
                db_ro_exec!(self.env, tool, {
                    command.execute(&tool)?;
                });
            }
            Subcommands::Checksum(command) => {
                db_ro_exec!(self.env, tool, {
                    command.execute(&tool)?;
                });
            }
            Subcommands::Diff(command) => {
                db_ro_exec!(self.env, tool, {
                    command.execute(&tool)?;
                });
            }
            Subcommands::Get(command) => {
                db_ro_exec!(self.env, tool, {
                    command.execute(&tool)?;
                });
            }
            Subcommands::Drop { force } => {
                if !force {
                    // Ask for confirmation
                    print!("Are you sure you want to drop the database at {data_dir}? This cannot be undone. (y/N): ");
                    // Flush the buffer to ensure the message is printed immediately
                    io::stdout().flush().unwrap();

                    let mut input = String::new();
                    io::stdin().read_line(&mut input).expect("Failed to read line");

                    if !input.trim().eq_ignore_ascii_case("y") {
                        println!("Database drop aborted!");
                        return Ok(())
                    }
                }

                let Environment { provider_factory, .. } = self.env.init(AccessRights::RW)?;
                let tool = DbTool::new(provider_factory)?;
                tool.drop(db_path, static_files_path)?;
            }
            Subcommands::Clear(command) => {
                let Environment { provider_factory, .. } = self.env.init(AccessRights::RW)?;
                command.execute(provider_factory)?;
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
    use reth_node_core::args::utils::SUPPORTED_CHAINS;
    use std::path::Path;

    #[test]
    fn parse_stats_globals() {
        let path = format!("../{}", SUPPORTED_CHAINS[0]);
        let cmd = Command::try_parse_from(["reth", "--datadir", &path, "stats"]).unwrap();
        assert_eq!(cmd.env.datadir.resolve_datadir(cmd.env.chain.chain).as_ref(), Path::new(&path));
    }
}
