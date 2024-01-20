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
use comfy_table::{Cell, Row, Table as ComfyTable};
use eyre::WrapErr;
use human_bytes::human_bytes;
use reth_db::{
    database::Database,
    mdbx, open_db, open_db_read_only,
    version::{get_db_version, DatabaseVersionError, DB_VERSION},
    Tables,
};
use reth_primitives::ChainSpec;
use std::{
    io::{self, Write},
    sync::Arc,
};

mod clear;
mod diff;
mod get;
mod list;
mod snapshots;
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
    Stats,
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
            Subcommands::Stats { .. } => {
                let db = open_db_read_only(&db_path, self.db.log_level)?;
                let tool = DbTool::new(&db, self.chain.clone())?;
                let mut stats_table = ComfyTable::new();
                stats_table.load_preset(comfy_table::presets::ASCII_MARKDOWN);
                stats_table.set_header([
                    "Table Name",
                    "# Entries",
                    "Branch Pages",
                    "Leaf Pages",
                    "Overflow Pages",
                    "Total Size",
                ]);

                tool.db.view(|tx| {
                    let mut tables =
                        Tables::ALL.iter().map(|table| table.name()).collect::<Vec<_>>();
                    tables.sort();
                    let mut total_size = 0;
                    for table in tables {
                        let table_db =
                            tx.inner.open_db(Some(table)).wrap_err("Could not open db.")?;

                        let stats = tx
                            .inner
                            .db_stat(&table_db)
                            .wrap_err(format!("Could not find table: {table}"))?;

                        // Defaults to 16KB right now but we should
                        // re-evaluate depending on the DB we end up using
                        // (e.g. REDB does not have these options as configurable intentionally)
                        let page_size = stats.page_size() as usize;
                        let leaf_pages = stats.leaf_pages();
                        let branch_pages = stats.branch_pages();
                        let overflow_pages = stats.overflow_pages();
                        let num_pages = leaf_pages + branch_pages + overflow_pages;
                        let table_size = page_size * num_pages;

                        total_size += table_size;
                        let mut row = Row::new();
                        row.add_cell(Cell::new(table))
                            .add_cell(Cell::new(stats.entries()))
                            .add_cell(Cell::new(branch_pages))
                            .add_cell(Cell::new(leaf_pages))
                            .add_cell(Cell::new(overflow_pages))
                            .add_cell(Cell::new(human_bytes(table_size as f64)));
                        stats_table.add_row(row);
                    }

                    let max_widths = stats_table.column_max_content_widths();

                    let mut seperator = Row::new();
                    for width in max_widths {
                        seperator.add_cell(Cell::new("-".repeat(width as usize)));
                    }
                    stats_table.add_row(seperator);

                    let mut row = Row::new();
                    row.add_cell(Cell::new("Total DB size"))
                        .add_cell(Cell::new(""))
                        .add_cell(Cell::new(""))
                        .add_cell(Cell::new(""))
                        .add_cell(Cell::new(""))
                        .add_cell(Cell::new(human_bytes(total_size as f64)));
                    stats_table.add_row(row);

                    let freelist = tx.inner.env().freelist()?;
                    let freelist_size = freelist *
                        tx.inner.db_stat(&mdbx::Database::freelist_db())?.page_size() as usize;

                    let mut row = Row::new();
                    row.add_cell(Cell::new("Freelist size"))
                        .add_cell(Cell::new(freelist))
                        .add_cell(Cell::new(""))
                        .add_cell(Cell::new(""))
                        .add_cell(Cell::new(""))
                        .add_cell(Cell::new(human_bytes(freelist_size as f64)));
                    stats_table.add_row(row);

                    Ok::<(), eyre::Report>(())
                })??;

                println!("{stats_table}");
            }
            Subcommands::List(command) => {
                let db = open_db_read_only(&db_path, self.db.log_level)?;
                let tool = DbTool::new(&db, self.chain.clone())?;
                command.execute(&tool)?;
            }
            Subcommands::Diff(command) => {
                let db = open_db_read_only(&db_path, self.db.log_level)?;
                let tool = DbTool::new(&db, self.chain.clone())?;
                command.execute(&tool)?;
            }
            Subcommands::Get(command) => {
                let db = open_db_read_only(&db_path, self.db.log_level)?;
                let tool = DbTool::new(&db, self.chain.clone())?;
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

                let db = open_db(&db_path, self.db.log_level)?;
                let mut tool = DbTool::new(&db, self.chain.clone())?;
                tool.drop(db_path)?;
            }
            Subcommands::Clear(command) => {
                let db = open_db(&db_path, self.db.log_level)?;
                command.execute(&db)?;
            }
            Subcommands::Snapshot(command) => {
                command.execute(&db_path, self.db.log_level, self.chain.clone())?;
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
