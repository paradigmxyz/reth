//! Database debugging tool
use crate::dirs::{DbPath, PlatformPath};
use clap::{Parser, Subcommand};
use comfy_table::{Cell, Row, Table as ComfyTable};
use eyre::{Result, WrapErr};
use reth_db::{
    cursor::{DbCursorRO, DbCursorRW, DbDupCursorRO, DbDupCursorRW, DupWalker, Walker},
    database::{Database, DatabaseGAT},
    models::BlockNumHash,
    table::{DupSort, Table},
    tables,
    transaction::{DbTx, DbTxMut},
};
use reth_interfaces::test_utils::generators::random_block_range;
use reth_provider::insert_canonical_block;
use reth_staged_sync::utils::init::init_db;
use reth_stages::{stages::ExecutionStage, Stage, StageId, UnwindInput};
use std::{collections::BTreeMap, ops::DerefMut};
use tracing::{error, info};

/// DB List TUI
mod tui;

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
    /// Deletes all database entries
    Out,
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
                                    let map = tool.list::<tables::$table>($start, $len)?;
                                    tui::DbListTUI::<tables::$table>::show_tui(map, $start, total_entries)
                                })??
                            },)*
                            _ => {
                                error!(target: "reth::cli", "Unknown table.");
                                return Ok(());
                            }
                        }
                    }
                }

                table_tui!(args.table.as_str(), args.start, args.len => [
                    CanonicalHeaders,
                    HeaderTD,
                    HeaderNumbers,
                    Headers,
                    BlockBodies,
                    BlockOmmers,
                    TxHashNumber,
                    PlainStorageState,
                    PlainAccountState,
                    BlockTransitionIndex,
                    TxTransitionIndex,
                    SyncStage,
                    Transactions
                ]);
            }
            Subcommands::Drop => {
                tool.drop(&self.db)?;
            }
            Subcommands::Out => {
                tool.out(49790, 49810, &self.db).await?;
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
    /// entries into a [`HashMap`][std::collections::HashMap].
    fn list<T: Table>(&mut self, start: usize, len: usize) -> Result<BTreeMap<T::Key, T::Value>> {
        let data = self.db.view(|tx| {
            let mut cursor = tx.cursor_read::<T>().expect("Was not able to obtain a cursor.");

            // TODO: Upstream this in the DB trait.
            let start_walker = cursor.current().transpose();
            let walker = Walker::new(&mut cursor, start_walker);

            walker.skip(start).take(len).collect::<Vec<_>>()
        })?;

        data.into_iter()
            .collect::<Result<BTreeMap<T::Key, T::Value>, _>>()
            .map_err(|e| eyre::eyre!(e))
    }

    fn drop(&mut self, path: &PlatformPath<DbPath>) -> Result<()> {
        info!(target: "reth::cli", "Dropping db at {}", path);
        std::fs::remove_dir_all(path).wrap_err("Dropping the database failed")?;
        Ok(())
    }

    async fn out(&mut self, from: u64, to: u64, output_path: &PlatformPath<DbPath>) -> Result<()> {
        let output_path: String = "hello".to_string();
        info!(target: "reth::cli", "Creating separate db at {}", output_path);
        assert!(from < to, "FROM block should be bigger than TO block.");

        let mut output_db = init_db(output_path)?;

        // Copy input tables
        self.dump_table_with_range::<tables::CanonicalHeaders>(&mut output_db, from, to)?;
        self.dump_table_with_range::<tables::HeaderTD>(&mut output_db, from, to)?;
        self.dump_table_with_range::<tables::Headers>(&mut output_db, from, to)?;
        self.dump_table_with_range::<tables::BlockBodies>(&mut output_db, from, to)?;

        // Find range of transactions that need to be copied over
        let (from_tx, to_tx) = self.db.view(|read_tx| {
            let mut read_cursor = read_tx.cursor_read::<tables::BlockBodies>()?;
            let (_, from_block) = read_cursor.seek(from.into())?.ok_or(eyre::eyre!("error"))?;
            let (_, to_block) = read_cursor.seek(to.into())?.ok_or(eyre::eyre!("error"))?;

            Ok::<(u64, u64), eyre::ErrReport>((
                from_block.start_tx_id,
                to_block.start_tx_id + to_block.tx_count,
            ))
        })??;
        self.dump_table_with_range::<tables::Transactions>(&mut output_db, from_tx, to_tx)?;
        self.dump_table_with_range::<tables::TxSenders>(&mut output_db, from_tx, to_tx)?;

        // Find the latest block to unwind from
        let (tip_block_number, _) = self
            .db
            .view(|tx| tx.cursor_read::<tables::BlockTransitionIndex>()?.last())??
            .expect("some");

        // Unwind to FROM block, we can get the PlainState safely
        let mut tx = reth_stages::Transaction::new(self.db)?;

        let mut exec_stage = ExecutionStage::default();
        exec_stage
            .unwind(
                &mut tx,
                UnwindInput {
                    unwind_to: from,
                    stage_progress: dbg!(tip_block_number),
                    bad_block: None,
                },
            )
            .await?;

        let inner_tx = tx.deref_mut();

        Self::dump_dupsort::<tables::PlainStorageState>(&output_db, inner_tx)?;
        Self::dump_table::<tables::PlainAccountState>(&output_db, inner_tx)?;
        Self::dump_table::<tables::Bytecodes>(&output_db, inner_tx)?;

        // We don't want to actually commit these changes to our original database.
        tx.drop()?;

        // Try to re-execute the stage without comitting
        {
            let mut tx = reth_stages::Transaction::new(&output_db)?;

            let mut exec_stage = ExecutionStage::default();
            exec_stage
                .execute(
                    &mut tx,
                    reth_stages::ExecInput {
                        previous_stage: Some((StageId("Another"), to)),
                        stage_progress: Some(from - 1),
                    },
                )
                .await?;
            tx.drop()?;
        }

        Ok(())
    }

    fn dump_table_with_range<T: Table>(
        &mut self,
        output_db: &mut reth_db::mdbx::Env<reth_db::mdbx::WriteMap>,
        from: u64,
        to: u64,
    ) -> eyre::Result<()>
    where
        <T as reth_db::table::Table>::Key: From<u64>,
    {
        output_db.update(|write_tx| {
            let mut write_cursor = write_tx.cursor_write::<T>()?;

            self.db.view(|read_tx| {
                let mut read_cursor = read_tx.cursor_read::<T>()?;

                for row in read_cursor.walk(from.into())?.take((to - from) as usize) {
                    let (key, value) = row?;
                    write_cursor.append(key, value)?;
                }

                Ok::<(), eyre::ErrReport>(())
            })
        })??
    }

    fn dump_table<T: Table>(
        output_db: &reth_db::mdbx::Env<reth_db::mdbx::WriteMap>,
        inner_tx: &mut <DB as DatabaseGAT<'_>>::TXMut,
    ) -> eyre::Result<()> {
        output_db.update(|write_tx| {
            let mut read_cursor = inner_tx.cursor_read::<T>()?;
            let mut write_cursor = write_tx.cursor_write::<T>()?;

            let start_walker = read_cursor.first().transpose();
            let mut walker = Walker::new(&mut read_cursor, start_walker);
            while let Some(v) = walker.next() {
                let (k, v) = v?;
                write_cursor.append(k, v)?;
            }
            Ok::<(), eyre::ErrReport>(())
        })?
    }

    fn dump_dupsort<T: DupSort>(
        output_db: &reth_db::mdbx::Env<reth_db::mdbx::WriteMap>,
        inner_tx: &mut <DB as DatabaseGAT<'_>>::TXMut,
    ) -> eyre::Result<()>
    where
        <T as reth_db::table::Table>::Key: Default,
        <T as reth_db::table::DupSort>::SubKey: Default,
    {
        output_db.update(|write_tx| {
            let mut read_cursor = inner_tx.cursor_dup_read::<T>()?;
            let mut write_cursor = write_tx.cursor_dup_write::<T>()?;

            if let Some((first_key, _)) = read_cursor.first()? {
                let mut walker = read_cursor.walk_dup(first_key, T::SubKey::default())?;
                while let Some(v) = walker.next() {
                    let (k, v) = v?;
                    write_cursor.append_dup(k, v)?;
                }
            }

            Ok::<(), eyre::ErrReport>(())
        })?
    }
}
