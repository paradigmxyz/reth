//! Database debugging tool
mod hashing_storage;
use hashing_storage::dump_hashing_storage_stage;

mod hashing_account;
use hashing_account::dump_hashing_account_stage;

mod execution;
use execution::dump_execution_stage;

mod merkle;
use merkle::dump_merkle_stage;

use crate::{
    dirs::{DbPath, PlatformPath},
    utils::DbTool,
};
use clap::Parser;
use reth_db::{
    cursor::DbCursorRO, database::Database, table::TableImporter, tables, transaction::DbTx,
};
use reth_staged_sync::utils::init::init_db;
use tracing::info;

/// `reth dump-stage` command
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
    command: Stages,
}

/// Supported stages to be dumped
#[derive(Debug, Clone, Parser)]
pub enum Stages {
    /// Execution stage.
    Execution(StageCommand),
    /// StorageHashing stage.
    StorageHashing(StageCommand),
    /// AccountHashing stage.
    AccountHashing(StageCommand),
    /// Merkle stage.
    Merkle(StageCommand),
}

/// Stage command that takes a range
#[derive(Debug, Clone, Parser)]
pub struct StageCommand {
    /// The path to the new database folder.
    ///
    /// Defaults to the OS-specific data directory:
    ///
    /// - Linux: `$XDG_DATA_HOME/reth/db` or `$HOME/.local/share/reth/db`
    /// - Windows: `{FOLDERID_RoamingAppData}/reth/db`
    /// - macOS: `$HOME/Library/Application Support/reth/db`
    #[arg(long, value_name = "OUTPUT_PATH", verbatim_doc_comment, default_value_t)]
    output_db: PlatformPath<DbPath>,
    /// From which block.
    #[arg(long, short)]
    from: u64,
    /// To which block.
    #[arg(long, short)]
    to: u64,
    /// If passed, it will dry-run a stage execution from the newly created database right after
    /// dumping.
    #[arg(long, short, default_value = "false")]
    dry_run: bool,
}

impl Command {
    /// Execute `dump-stage` command
    pub async fn execute(&self) -> eyre::Result<()> {
        std::fs::create_dir_all(&self.db)?;

        // TODO: Auto-impl for Database trait
        let db = reth_db::mdbx::Env::<reth_db::mdbx::WriteMap>::open(
            self.db.as_ref(),
            reth_db::mdbx::EnvKind::RW,
        )?;

        let mut tool = DbTool::new(&db)?;

        match &self.command {
            Stages::Execution(StageCommand { output_db, from, to, dry_run, .. }) => {
                dump_execution_stage(&mut tool, *from, *to, output_db, *dry_run).await?
            }
            Stages::StorageHashing(StageCommand { output_db, from, to, dry_run, .. }) => {
                dump_hashing_storage_stage(&mut tool, *from, *to, output_db, *dry_run).await?
            }
            Stages::AccountHashing(StageCommand { output_db, from, to, dry_run, .. }) => {
                dump_hashing_account_stage(&mut tool, *from, *to, output_db, *dry_run).await?
            }
            Stages::Merkle(StageCommand { output_db, from, to, dry_run, .. }) => {
                dump_merkle_stage(&mut tool, *from, *to, output_db, *dry_run).await?
            }
        }

        Ok(())
    }
}

/// Sets up the database and initial state on [`tables::BlockBodyIndices`]. Also returns the tip
/// block number.
pub(crate) fn setup<DB: Database>(
    from: u64,
    to: u64,
    output_db: &PlatformPath<DbPath>,
    db_tool: &mut DbTool<'_, DB>,
) -> eyre::Result<(reth_db::mdbx::Env<reth_db::mdbx::WriteMap>, u64)> {
    assert!(from < to, "FROM block should be bigger than TO block.");

    info!(target: "reth::cli", "Creating separate db at {}", output_db);

    let output_db = init_db(output_db)?;

    output_db.update(|tx| {
        tx.import_table_with_range::<tables::BlockBodyIndices, _>(
            &db_tool.db.tx()?,
            Some(from - 1),
            to + 1,
        )
    })??;

    let (tip_block_number, _) =
        db_tool.db.view(|tx| tx.cursor_read::<tables::BlockBodyIndices>()?.last())??.expect("some");

    Ok((output_db, tip_block_number))
}
