//! Database debugging tool
use crate::{
    dirs::{DataDirPath, MaybePlatformPath},
    utils::DbTool,
};
use clap::Parser;
use reth_db::{
    cursor::DbCursorRO, database::Database, init_db, table::TableImporter, tables,
    transaction::DbTx, DatabaseEnv,
};
use reth_primitives::ChainSpec;
use reth_provider::{providers::DiskFileTransactionDataStore, ProviderFactory};
use std::{path::PathBuf, sync::Arc};
use tracing::info;

mod hashing_storage;
use hashing_storage::dump_hashing_storage_stage;

mod hashing_account;
use hashing_account::dump_hashing_account_stage;

mod execution;
use execution::dump_execution_stage;

mod merkle;
use crate::args::{
    utils::{chain_help, genesis_value_parser, SUPPORTED_CHAINS},
    DatabaseArgs,
};
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
    #[arg(
        long,
        value_name = "CHAIN_OR_PATH",
        long_help = chain_help(),
        default_value = SUPPORTED_CHAINS[0],
        value_parser = genesis_value_parser
    )]
    chain: Arc<ChainSpec>,

    #[clap(flatten)]
    db: DatabaseArgs,

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

    /// The path to the new transaction store folder.
    #[arg(long, value_name = "OUTPUT_PATH", verbatim_doc_comment)]
    output_transaction_store: PathBuf,

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
        let db = Arc::new(init_db(db_path, self.db.log_level)?);
        info!(target: "reth::cli", "Database opened");

        let src_db = DbTool::new(
            &db,
            Arc::new(DiskFileTransactionDataStore::new(data_dir.transaction_data_store_path())?),
            self.chain.clone(),
        )?;

        let cmd = match &self.command {
            Stages::Execution(cmd) |
            Stages::StorageHashing(cmd) |
            Stages::AccountHashing(cmd) |
            Stages::Merkle(cmd) => cmd,
        };

        let StageCommand { output_db, output_transaction_store, from, to, dry_run, .. } = cmd;
        let (dest_db, tip) = setup(*from, *to, output_db, output_transaction_store, &src_db)?;

        match &self.command {
            Stages::Execution(_) => {
                dump_execution_stage(&src_db, &dest_db, *from..=*to, tip, *dry_run).await?
            }
            Stages::StorageHashing(_) => {
                dump_hashing_storage_stage(&src_db, &dest_db, *from..=*to, tip, *dry_run).await?
            }
            Stages::AccountHashing(_) => {
                dump_hashing_account_stage(&src_db, &dest_db, *from..=*to, tip, *dry_run).await?
            }
            Stages::Merkle(_) => {
                dump_merkle_stage(&src_db, &dest_db, *from..=*to, tip, *dry_run).await?
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
    output_transaction_store: &PathBuf,
    db_tool: &DbTool<'_, DB>,
) -> eyre::Result<(ProviderFactory<Arc<DatabaseEnv>>, u64)> {
    assert!(from < to, "FROM block should be bigger than TO block.");

    info!(target: "reth::cli", ?output_db, "Creating separate db");

    let output_db = init_db(output_db, None)?;

    output_db.update(|tx| {
        tx.import_table_with_range::<tables::BlockBodyIndices, _>(
            &db_tool.db.tx()?,
            Some(from - 1),
            to + 1,
        )
    })??;

    let (tip_block_number, _) =
        db_tool.db.view(|tx| tx.cursor_read::<tables::BlockBodyIndices>()?.last())??.expect("some");

    Ok((
        ProviderFactory::new(
            Arc::new(output_db),
            Arc::new(DiskFileTransactionDataStore::new(output_transaction_store.clone())?),
            db_tool.chain.clone(),
        ),
        tip_block_number,
    ))
}
