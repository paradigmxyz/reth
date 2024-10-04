//! Database debugging tool
use crate::common::{AccessRights, Environment, EnvironmentArgs};
use clap::Parser;
use reth_chainspec::{EthChainSpec, EthereumHardforks};
use reth_cli::chainspec::ChainSpecParser;
use reth_db::{init_db, mdbx::DatabaseArguments, tables, DatabaseEnv};
use reth_db_api::{
    cursor::DbCursorRO, database::Database, models::ClientVersion, table::TableImporter,
    transaction::DbTx,
};
use reth_db_common::DbTool;
use reth_evm::execute::BlockExecutorProvider;
use reth_node_builder::{NodeTypesWithDB, NodeTypesWithEngine};
use reth_node_core::{
    args::DatadirArgs,
    dirs::{DataDirPath, PlatformPath},
};
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
pub struct Command<C: ChainSpecParser> {
    #[command(flatten)]
    env: EnvironmentArgs<C>,

    #[command(subcommand)]
    command: Stages,
}

/// Supported stages to be dumped
#[derive(Debug, Clone, Parser)]
pub enum Stages {
    /// Execution stage.
    Execution(StageCommand),
    /// `StorageHashing` stage.
    StorageHashing(StageCommand),
    /// `AccountHashing` stage.
    AccountHashing(StageCommand),
    /// Merkle stage.
    Merkle(StageCommand),
}

/// Stage command that takes a range
#[derive(Debug, Clone, Parser)]
pub struct StageCommand {
    /// The path to the new datadir folder.
    #[arg(long, value_name = "OUTPUT_PATH", verbatim_doc_comment)]
    output_datadir: PlatformPath<DataDirPath>,

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

macro_rules! handle_stage {
    ($stage_fn:ident, $tool:expr, $command:expr) => {{
        let StageCommand { output_datadir, from, to, dry_run, .. } = $command;
        let output_datadir =
            output_datadir.with_chain($tool.chain().chain(), DatadirArgs::default());
        $stage_fn($tool, *from, *to, output_datadir, *dry_run).await?
    }};

    ($stage_fn:ident, $tool:expr, $command:expr, $executor:expr) => {{
        let StageCommand { output_datadir, from, to, dry_run, .. } = $command;
        let output_datadir =
            output_datadir.with_chain($tool.chain().chain(), DatadirArgs::default());
        $stage_fn($tool, *from, *to, output_datadir, *dry_run, $executor).await?
    }};
}

impl<C: ChainSpecParser<ChainSpec: EthChainSpec + EthereumHardforks>> Command<C> {
    /// Execute `dump-stage` command
    pub async fn execute<N, E, F>(self, executor: F) -> eyre::Result<()>
    where
        N: NodeTypesWithEngine<ChainSpec = C::ChainSpec>,
        E: BlockExecutorProvider,
        F: FnOnce(Arc<C::ChainSpec>) -> E,
    {
        let Environment { provider_factory, .. } = self.env.init::<N>(AccessRights::RO)?;
        let tool = DbTool::new(provider_factory)?;

        match &self.command {
            Stages::Execution(cmd) => {
                let executor = executor(tool.chain());
                handle_stage!(dump_execution_stage, &tool, cmd, executor)
            }
            Stages::StorageHashing(cmd) => handle_stage!(dump_hashing_storage_stage, &tool, cmd),
            Stages::AccountHashing(cmd) => handle_stage!(dump_hashing_account_stage, &tool, cmd),
            Stages::Merkle(cmd) => handle_stage!(dump_merkle_stage, &tool, cmd),
        }

        Ok(())
    }
}

/// Sets up the database and initial state on [`tables::BlockBodyIndices`]. Also returns the tip
/// block number.
pub(crate) fn setup<N: NodeTypesWithDB>(
    from: u64,
    to: u64,
    output_db: &PathBuf,
    db_tool: &DbTool<N>,
) -> eyre::Result<(DatabaseEnv, u64)> {
    assert!(from < to, "FROM block should be bigger than TO block.");

    info!(target: "reth::cli", ?output_db, "Creating separate db");

    let output_datadir = init_db(output_db, DatabaseArguments::new(ClientVersion::default()))?;

    output_datadir.update(|tx| {
        tx.import_table_with_range::<tables::BlockBodyIndices, _>(
            &db_tool.provider_factory.db_ref().tx()?,
            Some(from - 1),
            to + 1,
        )
    })??;

    let (tip_block_number, _) = db_tool
        .provider_factory
        .db_ref()
        .view(|tx| tx.cursor_read::<tables::BlockBodyIndices>()?.last())??
        .expect("some");

    Ok((output_datadir, tip_block_number))
}
