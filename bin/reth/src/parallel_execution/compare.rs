//! Command for comparing execution speed of regular and parallel execution.
use crate::{
    args::{utils::genesis_value_parser, DatabaseArgs},
    dirs::{DataDirPath, MaybePlatformPath},
    runner::CliContext,
};
use clap::Parser;
use eyre::Context;
use reth_db::init_db;
use reth_primitives::{fs, stage::StageId, ChainSpec};
use reth_provider::{
    BlockReader, HeaderProvider, HistoricalStateProviderRef, ProviderError, ProviderFactory,
    RangeExecutorFactory, StageCheckpointReader, TransactionVariant,
};
use reth_revm::{
    parallel::{factory::ParallelExecutorFactory, queue::TransitionQueueStore},
    EVMProcessorFactory,
};
use reth_stages::PipelineError;
use std::{path::PathBuf, sync::Arc, time::Instant};
use tracing::*;

/// `reth parallel-execution compare` command
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
    /// - holesky
    #[arg(
        long,
        value_name = "CHAIN_OR_PATH",
        verbatim_doc_comment,
        default_value = "mainnet",
        value_parser = genesis_value_parser
    )]
    chain: Arc<ChainSpec>,

    #[clap(flatten)]
    db: DatabaseArgs,

    /// The start block of the range.
    #[arg(long)]
    from: u64,

    /// The end block of the range.
    #[arg(long)]
    to: u64,

    /// Path to the block queues for parallel execution.
    #[arg(long)]
    pub queue_store: PathBuf,
}

impl Command {
    /// Execute `parallel-execution compare` command
    pub async fn execute(self, _ctx: CliContext) -> eyre::Result<()> {
        if self.from > self.to {
            eyre::bail!("Invalid block range provided")
        }

        let data_dir = self.datadir.unwrap_or_chain_default(self.chain.chain);
        let db_path = data_dir.db_path();
        fs::create_dir_all(&db_path)?;
        let db = Arc::new(init_db(db_path, self.db.log_level)?);

        let factory = ProviderFactory::new(&db, self.chain.clone());
        let provider = factory.provider().map_err(PipelineError::Interface)?;

        let latest_block_number =
            provider.get_stage_checkpoint(StageId::Finish)?.map(|ch| ch.block_number);
        if latest_block_number.unwrap_or_default() < self.to {
            eyre::bail!("Block range end is higher than the node block height")
        }

        let tx = provider.into_tx();
        let state_provider = HistoricalStateProviderRef::new(&tx, self.from);

        let executor_factory = EVMProcessorFactory::new(self.chain.clone());
        let mut regular_executor = executor_factory.with_provider_and_state(&state_provider);

        let queue_store_content = std::fs::read_to_string(&self.queue_store)
            .wrap_err("failed to read parallel queue store")?;
        let queues = serde_json::from_str(&queue_store_content)
            .wrap_err("failed to deserialize queue store")?;
        let queue_store = Arc::new(TransitionQueueStore::new(queues));
        let parallel_factory =
            ParallelExecutorFactory::new(self.chain.clone(), queue_store.clone());
        let mut parallel_executor = parallel_factory.with_provider_and_state(&state_provider);

        let provider = factory.provider().map_err(PipelineError::Interface)?;

        let mut regular_better_blocks = 0;
        let mut parallel_better_blocks = 0;
        let mut total_diff_ns = 0_i128;
        for block_number in self.from..=self.to {
            debug!(target: "reth::cli", block_number, "Comparing execution");

            let td = provider
                .header_td_by_number(block_number)?
                .ok_or(ProviderError::TotalDifficultyNotFound { block_number })?;
            let (block, senders) = provider
                .block_with_senders(block_number, TransactionVariant::WithHash)?
                .ok_or(ProviderError::BlockNotFound(block_number.into()))?
                .into_components();

            let instant = Instant::now();
            regular_executor.execute_and_verify_receipt(&block, td, Some(senders.clone())).await?;
            let regular_elapsed = instant.elapsed();

            let instant = Instant::now();
            parallel_executor.execute_and_verify_receipt(&block, td, Some(senders)).await?;
            let parallel_elapsed = instant.elapsed();

            trace!(
                target: "reth::cli",
                block_number,
                regular_elapsed_ms = regular_elapsed.as_nanos(),
                parallel_elapsed_ms = parallel_elapsed.as_nanos(),
                "Finished executing block"
            );
            if regular_elapsed < parallel_elapsed {
                regular_better_blocks += 1;
            } else {
                parallel_better_blocks += 1;
                info!(
                    target: "reth::cli",
                    block_number,
                    regular_elapsed_ns = regular_elapsed.as_nanos(),
                    parallel_elapsed_ns = parallel_elapsed.as_nanos(),
                    queue = ?queue_store.get_queue(block_number),
                    "Parallel execution is better"
                );
            }
            total_diff_ns +=
                regular_elapsed.as_nanos() as i128 - parallel_elapsed.as_nanos() as i128;
        }

        info!(
            target: "reth::cli",
            regular_better_blocks,
            parallel_better_blocks,
            total_diff_ns,
            "Finished comparing execution"
        );

        Ok(())
    }
}
