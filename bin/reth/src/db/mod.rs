//! Database debugging tool
use crate::dirs::{DbPath, PlatformPath};
use clap::{Parser, Subcommand};
use comfy_table::{Cell, Row, Table as ComfyTable};
use eyre::{Result, WrapErr};
use reth_db::{
    cursor::{DbCursorRO, Walker},
    database::Database,
    mdbx::{Env, EnvironmentKind},
    table::Table,
    tables,
    transaction::{DbTx, DbTxMut},
};
use reth_interfaces::test_utils::generators::random_block_range;
use reth_provider::insert_canonical_block;
use std::collections::HashMap;
use tracing::{error, info};

/// `reth db` command
#[derive(Debug, Parser)]
pub struct Command {
    /// The path to the database folder.
    ///
    /// Defaults to the OS-specific data directory:
    ///
    /// - Linux: `$XDG_DATA_HOME/reth/db` or `$HOME/.local/share/reth/db`
    /// - Windows: `{FOLDERID_RoamingAppData}/reth/db`
    /// - macOS: `$HOME/Library/Application Support/reth/db`
    #[arg(long, value_name = "PATH", verbatim_doc_comment, default_value_t)]
    db: PlatformPath<DbPath>,

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
    /// Seeds the database with random blocks on top of each other
    Seed {
        /// How many blocks to generate
        #[arg(default_value = DEFAULT_NUM_ITEMS)]
        len: u64,
    },
    /// Deletes all database entries
    Drop,
}

#[derive(Parser, Debug)]
/// The arguments for the `reth db list` command
pub struct ListArgs {
    /// The table name
    table: String, // TODO: Convert to enum
    /// Where to start iterating
    #[arg(long, short, default_value = "0")]
    start: usize,
    /// How many items to take from the walker
    #[arg(long, short, default_value = DEFAULT_NUM_ITEMS)]
    len: usize,
}

impl Command {
    /// Execute `db` command
    pub async fn execute(&self) -> eyre::Result<()> {
        std::fs::create_dir_all(&self.db)?;

        // TODO: Auto-impl for Database trait
        let db = reth_db::mdbx::Env::<reth_db::mdbx::WriteMap>::open(
            self.db.as_ref(),
            reth_db::mdbx::EnvKind::RW,
        )?;

        let mut tool = DbTool::new(&db)?;

        match &self.command {
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
                    "Total Size (KB)",
                ]);

                tool.db.view(|tx| {
                    for table in tables::TABLES.iter().map(|(_, name)| name) {
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
                            .add_cell(Cell::new(table_size / 1024));
                        stats_table.add_row(row);
                    }
                    Ok::<(), eyre::Report>(())
                })??;

                println!("{stats_table}");
            }
            Subcommands::Seed { len } => {
                tool.seed(*len)?;
            }
            Subcommands::List(args) => {
                tool.list_table(args)?;
            }
            Subcommands::Drop => {
                tool.drop()?;
            }
        }

        Ok(())
    }
}

/// Wrapper over DB that implements many useful DB queries.
struct DbTool<'a, DB: Database> {
    pub(crate) db: &'a DB,
}

impl<'a, DB: Database> DbTool<'a, DB> {
    /// Takes a DB where the tables have already been created.
    fn new(db: &'a DB) -> eyre::Result<Self> {
        Ok(Self { db })
    }

    /// Seeds the database with some random data, only used for testing
    fn seed(&mut self, len: u64) -> Result<()> {
        info!(target: "reth::cli", "Generating random block range from 0 to {len}");
        let chain = random_block_range(0..len, Default::default(), 0..64);

        self.db.update(|tx| {
            chain.iter().try_for_each(|block| {
                insert_canonical_block(tx, block, true)?;
                Ok::<_, eyre::Error>(())
            })
        })??;

        info!(target: "reth::cli", "Database seeded with {len} blocks");
        Ok(())
    }

    /// Grabs the contents of the table within a certain index range and places the
    /// entries into a [HashMap].
    fn list<T: Table>(&mut self, start: usize, len: usize) -> Result<HashMap<T::Key, T::Value>> {
        let data = self.db.view(|tx| {
            let mut cursor = tx.cursor_read::<T>().expect("Was not able to obtain a cursor.");

            // TODO: Upstream this in the DB trait.
            let start_walker = cursor.current().transpose();
            let walker = Walker {
                cursor: &mut cursor,
                start: start_walker,
                _tx_phantom: std::marker::PhantomData,
            };

            walker.skip(start).take(len).collect::<Vec<_>>()
        })?;

        data.into_iter()
            .collect::<Result<HashMap<T::Key, T::Value>, _>>()
            .map_err(|e| eyre::eyre!(e))
    }

    fn drop(&mut self) -> Result<()> {
        macro_rules! drop_tables {
            ([$($table:ident,)*]) => {
                let _tx = self.db.tx_mut()?;
                $(_tx.clear::<tables::$table>()?;)*
                _tx.commit()?;
            };
        }

        drop_tables!([
            CanonicalHeaders,
            HeaderTD,
            HeaderNumbers,
            Headers,
            BlockBodies,
            BlockOmmers,
            NonCanonicalTransactions,
            Transactions,
            TxHashNumber,
            Receipts,
            Logs,
            PlainAccountState,
            PlainStorageState,
            Bytecodes,
            BlockTransitionIndex,
            TxTransitionIndex,
            AccountHistory,
            StorageHistory,
            AccountChangeSet,
            StorageChangeSet,
            TxSenders,
            Config,
            SyncStage,
        ]);

        Ok(())
    }
}

impl<'a, E: EnvironmentKind> DbTool<'a, Env<E>> {
    /// Lists the contents of a db table in a human-readable format and print to stdout.
    fn list_table(&mut self, args: &ListArgs) -> Result<()> {
        macro_rules! list_tables {
            ($arg:expr, $start:expr, $len:expr => [$($table:ident),*]) => {
                match $arg {
                    $(stringify!($table) => {
                        let map = self.list::<tables::$table>($start, $len)?;
                        let mut table = ComfyTable::new();
                        table.load_preset(comfy_table::presets::ASCII_MARKDOWN);
                        table.set_header(["Key", "Value"]);
                        for (key, value) in map.into_iter() {
                            let mut row = Row::new();
                            // TODO: Cleanly format key / value types
                            // For tables with longer decompressed values, i.e. `Headers`, formatting
                            // the value via the `Debug` trait breaks the table's formatting.
                            row.add_cell(Cell::new(format!("{key:?}"))).add_cell(Cell::new(format!("{value:?}")));
                            table.add_row(row);
                        }

                        self.db.view(|tx| {
                            let table_db = tx.inner.open_db(Some(stringify!($table))).wrap_err("Could not open db.")?;
                            let stats = tx.inner.db_stat(&table_db).wrap_err(format!("Could not find table: {}", stringify!($table)))?;
                            let final_entry_idx = stats.entries() - 1;
                            if $start > final_entry_idx {
                                error!(
                                    "Start index {start} is greater than the final entry index ({final_entry_idx}) in the table {table}",
                                    start = $start,
                                    table = stringify!($table)
                                );
                                return Ok(())
                            }

                            println!("{table}");
                            println!(
                                "-> Showing {len} entries in range [{start}, {end}] out of {num_entries} entries.",
                                len = $len,
                                start = $start,
                                end = if ($start + $len) > final_entry_idx {
                                    final_entry_idx
                                } else {
                                    $start + $len - 1
                                },
                                num_entries = final_entry_idx + 1
                            );
                            Ok::<(), eyre::Report>(())
                        })?
                    },)*
                    _ => {
                        error!("Unknown table.");
                        Ok(())
                    }
                }?
            };
        }

        list_tables!(args.table.as_str(), args.start, args.len => [
            CanonicalHeaders,
            HeaderTD,
            HeaderNumbers,
            Headers,
            BlockBodies,
            BlockOmmers,
            TxHashNumber,
            PlainAccountState,
            BlockTransitionIndex,
            TxTransitionIndex,
            SyncStage,
            Transactions
        ]);

        Ok(())
    }
}
