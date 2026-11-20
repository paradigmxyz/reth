//! Database debugging tool
use crate::common::{AccessRights, CliNodeComponents, CliNodeTypes, Environment, EnvironmentArgs};
use clap::Parser;
use reth_chainspec::{EthChainSpec, EthereumHardforks};
use reth_cli::chainspec::ChainSpecParser;
use reth_config::config::ExecutionConfig;
use reth_consensus::FullConsensus;
use reth_db::{init_db, mdbx::DatabaseArguments, DatabaseEnv};
use reth_db_api::{
    cursor::DbCursorRO, database::Database, models::ClientVersion, table::TableImporter, tables,
    transaction::DbTx,
};
use reth_db_common::DbTool;
use reth_evm::ConfigureEvm;
use reth_node_builder::NodeTypesWithDB;
use reth_node_core::{
    args::DatadirArgs,
    dirs::{DataDirPath, PlatformPath},
};
use reth_stages::{
    stages::{ExecutionStage, MERKLE_STAGE_DEFAULT_REBUILD_THRESHOLD},
    Stage, UnwindInput,
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

/// `reth dump-stage` command.
///
/// Note: mutates the source datadir (unwinds hashing/merkle/execution before copying tables).
/// Stop the node and back up the datadir first.
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

impl StageCommand {
    /// Validates the block range before any database is opened.
    fn validate(&self) -> eyre::Result<()> {
        eyre::ensure!(self.from > 0, "`--from` must be at least 1");
        eyre::ensure!(
            self.from < self.to,
            "`--from` ({}) must be lower than `--to` ({})",
            self.from,
            self.to
        );
        Ok(())
    }
}

macro_rules! handle_stage {
    ($stage_fn:ident, $tool:expr, $command:expr, $runtime:expr) => {{
        let StageCommand { output_datadir, from, to, dry_run, .. } = $command;
        let output_datadir =
            output_datadir.with_chain($tool.chain().chain(), DatadirArgs::default());
        $stage_fn($tool, *from, *to, output_datadir, *dry_run, $runtime).await?
    }};

    ($stage_fn:ident, $tool:expr, $command:expr, $executor:expr, $consensus:expr, $runtime:expr) => {{
        let StageCommand { output_datadir, from, to, dry_run, .. } = $command;
        let output_datadir =
            output_datadir.with_chain($tool.chain().chain(), DatadirArgs::default());
        $stage_fn($tool, *from, *to, output_datadir, *dry_run, $executor, $consensus, $runtime)
            .await?
    }};
}

impl<C: ChainSpecParser<ChainSpec: EthChainSpec + EthereumHardforks>> Command<C> {
    /// Execute `dump-stage` command
    pub async fn execute<N, Comp, F>(
        self,
        components: F,
        runtime: reth_tasks::Runtime,
    ) -> eyre::Result<()>
    where
        N: CliNodeTypes<ChainSpec = C::ChainSpec>,
        Comp: CliNodeComponents<N>,
        F: FnOnce(Arc<C::ChainSpec>) -> Comp,
    {
        match &self.command {
            Stages::Execution(cmd) |
            Stages::StorageHashing(cmd) |
            Stages::AccountHashing(cmd) |
            Stages::Merkle(cmd) => cmd.validate()?,
        }

        // `unwind_and_copy` opens a RW provider on the source datadir, so open RW here.
        let Environment { provider_factory, .. } =
            self.env.init::<N>(AccessRights::RW, runtime.clone())?;
        let tool = DbTool::new(provider_factory)?;
        let components = components(tool.chain());
        let evm_config = components.evm_config().clone();
        let consensus = components.consensus().clone();

        match &self.command {
            Stages::Execution(cmd) => {
                handle_stage!(
                    dump_execution_stage,
                    &tool,
                    cmd,
                    evm_config,
                    consensus,
                    runtime.clone()
                )
            }
            Stages::StorageHashing(cmd) => {
                handle_stage!(dump_hashing_storage_stage, &tool, cmd, runtime.clone())
            }
            Stages::AccountHashing(cmd) => {
                handle_stage!(dump_hashing_account_stage, &tool, cmd, runtime.clone())
            }
            Stages::Merkle(cmd) => {
                handle_stage!(dump_merkle_stage, &tool, cmd, evm_config, consensus, runtime.clone())
            }
        }

        Ok(())
    }
}

impl<C: ChainSpecParser> Command<C> {
    /// Returns the underlying chain being used to run this command
    pub fn chain_spec(&self) -> Option<&Arc<C::ChainSpec>> {
        Some(&self.env.chain)
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
        .ok_or_else(|| eyre::eyre!("`BlockBodyIndices` table of the source database is empty"))?;

    Ok((output_datadir, tip_block_number))
}

/// Execution queues static-file prunes, so the source transaction must unwind in one batch.
fn unwind_execution_for_dump<E: ConfigureEvm, P>(
    evm_config: E,
    consensus: Arc<dyn FullConsensus<E::Primitives>>,
    provider: &P,
    input: UnwindInput,
) -> eyre::Result<()>
where
    ExecutionStage<E>: Stage<P>,
{
    let output = ExecutionStage::from_config(
        evm_config,
        consensus,
        ExecutionConfig { max_blocks: None, ..Default::default() },
        MERKLE_STAGE_DEFAULT_REBUILD_THRESHOLD,
    )
    .unwind(provider, input)?;
    eyre::ensure!(
        output.checkpoint.block_number == input.unwind_to,
        "execution dump unwind stopped at block {}, expected {}",
        output.checkpoint.block_number,
        input.unwind_to
    );
    Ok(())
}

/// Finish every hashing batch inside the uncommitted source transaction before copying the state.
fn unwind_hashing_for_dump<P>(
    mut stage: impl Stage<P>,
    provider: &P,
    mut input: UnwindInput,
) -> eyre::Result<()> {
    while input.checkpoint.block_number > input.unwind_to {
        let output = stage.unwind(provider, input)?;
        eyre::ensure!(
            (input.unwind_to..input.checkpoint.block_number)
                .contains(&output.checkpoint.block_number),
            "{} dump unwind returned block {} for range {}..={}",
            stage.id(),
            output.checkpoint.block_number,
            input.unwind_to,
            input.checkpoint.block_number
        );
        input.checkpoint = output.checkpoint;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{keccak256, Address};
    use reth_db_api::{models::AccountBeforeTx, transaction::DbTxMut};
    use reth_primitives_traits::Account;
    use reth_provider::{
        test_utils::create_test_provider_factory, DBProvider, DatabaseProviderFactory,
    };
    use reth_stages::{stages::AccountHashingStage, StageCheckpoint};

    #[test]
    fn hashing_dump_finishes_in_source_transaction() {
        let factory = create_test_provider_factory();
        let address = Address::repeat_byte(1);
        let hash = keccak256(address);
        let provider = factory.database_provider_rw().unwrap();
        let tx = provider.tx_ref();
        for block in 2..=3 {
            tx.put::<tables::AccountChangeSets>(
                block,
                AccountBeforeTx {
                    address,
                    info: Some(Account { nonce: block - 1, ..Default::default() }),
                },
            )
            .unwrap();
        }
        tx.put::<tables::HashedAccounts>(hash, Account { nonce: 3, ..Default::default() }).unwrap();
        provider.commit().unwrap();

        let provider = factory.database_provider_rw().unwrap();
        unwind_hashing_for_dump(
            AccountHashingStage { commit_threshold: 1, ..Default::default() },
            &provider,
            UnwindInput { checkpoint: StageCheckpoint::new(3), unwind_to: 1, bad_block: None },
        )
        .unwrap();
        assert_eq!(
            provider.tx_ref().get::<tables::HashedAccounts>(hash).unwrap().unwrap().nonce,
            1
        );
        drop(provider);
        assert_eq!(
            factory
                .provider()
                .unwrap()
                .tx_ref()
                .get::<tables::HashedAccounts>(hash)
                .unwrap()
                .unwrap()
                .nonce,
            3
        );
    }
}
