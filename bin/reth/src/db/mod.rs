//! Database debugging tool
use crate::{
    args::utils::genesis_value_parser,
    dirs::{DataDirPath, MaybePlatformPath},
    utils::DbTool,
};
use clap::{Parser, Subcommand};
use comfy_table::{Cell, Row, Table as ComfyTable};
use eyre::WrapErr;
use human_bytes::human_bytes;
use reth_db::{database::Database, tables};
use reth_primitives::ChainSpec;
use std::sync::Arc;
use tracing::error;

mod get;
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
    ///
    /// Built-in chains:
    /// - mainnet
    /// - goerli
    /// - sepolia
    #[arg(
        long,
        value_name = "CHAIN_OR_PATH",
        verbatim_doc_comment,
        default_value = "mainnet",
        value_parser = genesis_value_parser,
        global = true,
    )]
    chain: Arc<ChainSpec>,

    #[clap(subcommand)]
    command: Subcommands,
}

const DEFAULT_NUM_ITEMS: &str = "5";

#[derive(Subcommand, Debug)]
/// `reth db` subcommands
pub enum Subcommands {
    /// Lists all the tables, their entry count and their size
    Stats,
    /// Lists the contents of a table
    List(ListArgs),
    /// Gets the content of a table for the given key
    Get(get::Command),
    /// Deletes all database entries
    Drop,
}

#[derive(Parser, Debug)]
/// The arguments for the `reth db list` command
pub struct ListArgs {
    /// The table name
    table: String, // TODO: Convert to enum
    /// Skip first N entries
    #[arg(long, short, default_value = "0")]
    skip: usize,
    /// Reverse the order of the entries. If enabled last table entries are read.
    #[arg(long, short, default_value = "false")]
    reverse: bool,
    /// How many items to take from the walker
    #[arg(long, short, default_value = DEFAULT_NUM_ITEMS)]
    len: usize,
    /// Dump as JSON instead of using TUI.
    #[arg(long, short)]
    json: bool,
}

impl Command {
    /// Execute `db` command
    pub async fn execute(self) -> eyre::Result<()> {
        // add network name to data dir
        let data_dir = self.datadir.unwrap_or_chain_default(self.chain.chain);
        let db_path = data_dir.db_path();
        std::fs::create_dir_all(&db_path)?;

        // TODO: Auto-impl for Database trait
        let db = reth_db::mdbx::Env::<reth_db::mdbx::WriteMap>::open(
            db_path.as_ref(),
            reth_db::mdbx::EnvKind::RW,
        )?;

        let mut tool = DbTool::new(&db, self.chain.clone())?;

        match self.command {
            // TODO: We'll need to add this on the DB trait.
            Subcommands::Stats { .. } => {
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
                        tables::TABLES.iter().map(|(_, name)| name).collect::<Vec<_>>();
                    tables.sort();
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

                        let mut row = Row::new();
                        row.add_cell(Cell::new(table))
                            .add_cell(Cell::new(stats.entries()))
                            .add_cell(Cell::new(branch_pages))
                            .add_cell(Cell::new(leaf_pages))
                            .add_cell(Cell::new(overflow_pages))
                            .add_cell(Cell::new(human_bytes(table_size as f64)));
                        stats_table.add_row(row);
                    }
                    Ok::<(), eyre::Report>(())
                })??;

                println!("{stats_table}");
            }
            Subcommands::List(args) => {
                macro_rules! table_tui {
                    ($arg:expr, $start:expr, $len:expr => [$($table:ident),*]) => {
                        match $arg {
                            $(stringify!($table) => {
                                tool.db.view(|tx| {
                                    let table_db = tx.inner.open_db(Some(stringify!($table))).wrap_err("Could not open db.")?;
                                    let stats = tx.inner.db_stat(&table_db).wrap_err(format!("Could not find table: {}", stringify!($table)))?;
                                    let total_entries = stats.entries();
                                    if $start > total_entries - 1 {
                                        error!(
                                            target: "reth::cli",
                                            "Start index {start} is greater than the final entry index ({final_entry_idx}) in the table {table}",
                                            start = $start,
                                            final_entry_idx = total_entries - 1,
                                            table = stringify!($table)
                                        );
                                        return Ok(());
                                    }

                                    if args.json {
                                        let list_result = tool.list::<tables::$table>(args.skip, args.len,args.reverse)?.into_iter().collect::<Vec<_>>();
                                        println!("{}", serde_json::to_string_pretty(&list_result)?);
                                        Ok(())
                                    } else {
                                        tui::DbListTUI::<_, tables::$table>::new(|skip, count| {
                                            tool.list::<tables::$table>(skip, count, args.reverse).unwrap()
                                        }, $start, $len, total_entries).run()
                                    }
                                })??
                            },)*
                            _ => {
                                error!(target: "reth::cli", "Unknown table.");
                                return Ok(());
                            }
                        }
                    }
                }

                table_tui!(args.table.as_str(), args.skip, args.len => [
                    CanonicalHeaders,
                    HeaderTD,
                    HeaderNumbers,
                    Headers,
                    BlockBodyIndices,
                    BlockOmmers,
                    BlockWithdrawals,
                    TransactionBlock,
                    Transactions,
                    TxHashNumber,
                    Receipts,
                    PlainStorageState,
                    PlainAccountState,
                    Bytecodes,
                    AccountHistory,
                    StorageHistory,
                    AccountChangeSet,
                    StorageChangeSet,
                    HashedAccount,
                    HashedStorage,
                    AccountsTrie,
                    StoragesTrie,
                    TxSenders,
                    SyncStage,
                    SyncStageProgress
                ]);
            }
            Subcommands::Get(command) => {
                command.execute(tool)?;
            }
            Subcommands::Drop => {
                tool.drop(db_path)?;
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
        let cmd = Command::try_parse_from(["reth", "stats", "--datadir", "../mainnet"]).unwrap();
        assert_eq!(cmd.datadir.as_ref(), Some(Path::new("../mainnet")));
    }
}
