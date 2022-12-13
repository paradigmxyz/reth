// TODO: Remove
#![allow(missing_docs)]
#![allow(unused_variables)]

//! Main db command
//!
//! Database debugging tool

use clap::{Parser, Subcommand};
use eyre::Result;
use reth_db::{
    cursor::{DbCursorRO, Walker},
    database::Database,
    table::Table,
    tables,
    transaction::DbTx,
};
use reth_interfaces::test_utils::generators::random_block_range;
use reth_primitives::BlockLocked;
use reth_provider::block::insert_canonical_block;
use reth_stages::db::StageDB;
use std::{path::Path, sync::Arc};
use tracing::info;

/// Execute Ethereum blockchain tests by specifying path to json files
#[derive(Debug, Parser)]
pub struct Command {
    /// Path to database folder
    #[arg(long, default_value = "~/.reth/db")]
    db: String,

    #[clap(subcommand)]
    command: Subcommands,
}

const DEFAULT_NUM_ITEMS: &str = "5";

#[derive(Subcommand, Debug)]
pub enum Subcommands {
    Stats {
        table: Option<String>,
    },
    List(ListArgs),
    /// Seeds the block db with random blocks on top of each other
    Seed {
        /// How many blocks to generate
        #[arg(default_value = DEFAULT_NUM_ITEMS)]
        len: u64,
    },
}

#[derive(Parser, Debug)]
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
    /// Execute `node` command
    pub async fn execute(&self) -> eyre::Result<()> {
        let path = shellexpand::full(&self.db)?.into_owned();
        let expanded_db_path = Path::new(&path);
        std::fs::create_dir_all(expanded_db_path)?;

        // TODO: Auto-impl for Database trait
        let db = reth_db::mdbx::Env::<reth_db::mdbx::WriteMap>::open(
            expanded_db_path,
            reth_db::mdbx::EnvKind::RW,
        )?;
        db.create_tables()?;

        let mut tool = DbTool::new(&db)?;

        match &self.command {
            Subcommands::Stats { .. } => {}
            Subcommands::Seed { len } => {
                tool.seed(*len)?;
            }
            Subcommands::List(args) => {
                tool.list(args)?;
            }
        }

        Ok(())
    }
}

/// Abstraction over StageDB for writing/reading from/to the DB
/// Wraps over the StageDB and derefs to a transaction.
struct DbTool<'a, DB: Database> {
    // TODO: StageDB derefs to Tx, is this weird or not?
    pub(crate) db: StageDB<'a, DB>,
}

impl<'a, DB: Database> DbTool<'a, DB> {
    /// Takes a DB where the tables have already been created
    fn new(db: &'a DB) -> eyre::Result<Self> {
        Ok(Self { db: StageDB::new(db)? })
    }

    /// Seeds the database with some random data, only used for testing
    fn seed(&mut self, len: u64) -> Result<()> {
        let chain = random_block_range(0..len, Default::default());
        chain.iter().try_for_each(|block| {
            insert_canonical_block(&*self.db, block, true)?;
            Ok::<_, eyre::Error>(())
        })?;

        self.db.commit()?;
        info!("Database seeded with {len} blocks");

        Ok(())
    }

    /// Lists the given table data
    fn list(&mut self, args: &ListArgs) -> Result<()> {
        match args.table.as_str() {
            "headers" => self.list_table::<tables::Headers>(args.start, args.len)?,
            "txs" => self.list_table::<tables::Transactions>(args.start, args.len)?,
            _ => panic!(),
        };
        Ok(())
    }

    fn list_table<T: Table>(&mut self, start: usize, len: usize) -> Result<()> {
        let mut cursor = self.db.cursor::<T>()?;

        // TODO: Upstream this in the DB trait.
        let start_walker = cursor.current().transpose();
        let walker = Walker {
            cursor: &mut cursor,
            start: start_walker,
            _tx_phantom: std::marker::PhantomData,
        };

        let data = walker.skip(start).take(len).collect::<Vec<_>>();
        dbg!(&data);

        Ok(())
    }
}
