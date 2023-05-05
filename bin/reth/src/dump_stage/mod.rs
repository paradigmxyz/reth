//! Database debugging tool
use crate::{
    dirs::{DataDirPath, MaybePlatformPath},
    utils::DbTool,
};
use clap::Parser;
use reth_db::{
    cursor::DbCursorRO, database::Database, table::TableImporter, tables, transaction::DbTx,
};
use reth_primitives::ChainSpec;
use reth_staged_sync::utils::{chainspec::genesis_value_parser, init::init_db};
use std::{path::PathBuf, sync::Arc};
use tracing::info;

mod hashing_storage;
use hashing_storage::dump_hashing_storage_stage;

mod hashing_account;
use hashing_account::dump_hashing_account_stage;

mod execution;
use execution::dump_execution_stage;

mod merkle;
use merkle::dump_merkle_stage;

/// `reth dump-stage` command
#[derive(Debug, Parser)]
pub struct Command {
    /// The path to the data dir for all reth files and subdirectories.
    ///
    /// Defaults to the OS-specific data directory:
    ///
    /// - Linux: `$XDG_DATA_HOME/reth/` or `$HOME/.local/share/reth/`
    /// - Windows: `{FOLDERID_RoamingAppData}/reth/`
    /// - macOS: `$HOME/Library/Application Support/reth/`
    #[arg(long, value_name = "DATA_DIR", verbatim_doc_comment, default_value_t)]
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
        value_parser = genesis_value_parser
    )]
    chain: Arc<ChainSpec>,

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
    #[arg(long, value_name = "OUTPUT_PATH", verbatim_doc_comment)]
    output_db: PathBuf,

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
    pub async fn execute(self) -> eyre::Result<()> {
        // add network name to data dir
        let data_dir = self.datadir.unwrap_or_chain_default(self.chain.chain);
        let db_path = data_dir.db_path();
        info!(target: "reth::cli", path = ?db_path, "Opening database");
        std::fs::create_dir_all(&db_path)?;

        // TODO: Auto-impl for Database trait
        let db = reth_db::mdbx::Env::<reth_db::mdbx::WriteMap>::open(
            db_path.as_ref(),
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
    output_db: &PathBuf,
    db_tool: &mut DbTool<'_, DB>,
) -> eyre::Result<(reth_db::mdbx::Env<reth_db::mdbx::WriteMap>, u64)> {
    assert!(from < to, "FROM block should be bigger than TO block.");

    info!(target: "reth::cli", ?output_db, "Creating separate db");

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
