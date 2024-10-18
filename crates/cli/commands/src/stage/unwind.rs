//! Unwinding a certain block range

use crate::common::{AccessRights, Environment, EnvironmentArgs};
use alloy_eips::BlockHashOrNumber;
use alloy_primitives::{BlockNumber, B256};
use clap::{Parser, Subcommand};
use reth_beacon_consensus::EthBeaconConsensus;
use reth_chainspec::{EthChainSpec, EthereumHardforks};
use reth_cli::chainspec::ChainSpecParser;
use reth_config::Config;
use reth_consensus::Consensus;
use reth_db::DatabaseEnv;
use reth_downloaders::{bodies::noop::NoopBodiesDownloader, headers::noop::NoopHeaderDownloader};
use reth_evm::noop::NoopBlockExecutorProvider;
use reth_exex::ExExManagerHandle;
use reth_node_builder::{NodeTypesWithDB, NodeTypesWithEngine};
use reth_node_core::args::NetworkArgs;
use reth_provider::{
    providers::ProviderNodeTypes, BlockExecutionWriter, BlockNumReader, ChainSpecProvider,
    ChainStateBlockReader, ChainStateBlockWriter, ProviderFactory, StaticFileProviderFactory,
};
use reth_prune::PruneModes;
use reth_stages::{
    sets::{DefaultStages, OfflineStages},
    stages::ExecutionStage,
    ExecutionStageThresholds, Pipeline, StageSet,
};
use reth_static_file::StaticFileProducer;
use std::{ops::RangeInclusive, sync::Arc};
use tokio::sync::watch;
use tracing::info;

/// `reth stage unwind` command
#[derive(Debug, Parser)]
pub struct Command<C: ChainSpecParser> {
    #[command(flatten)]
    env: EnvironmentArgs<C>,

    #[command(flatten)]
    network: NetworkArgs,

    #[command(subcommand)]
    command: Subcommands,

    /// If this is enabled, then all stages except headers, bodies, and sender recovery will be
    /// unwound.
    #[arg(long)]
    offline: bool,
}

impl<C: ChainSpecParser<ChainSpec: EthChainSpec + EthereumHardforks>> Command<C> {
    /// Execute `db stage unwind` command
    pub async fn execute<N: NodeTypesWithEngine<ChainSpec = C::ChainSpec>>(
        self,
    ) -> eyre::Result<()> {
        let Environment { provider_factory, config, .. } = self.env.init::<N>(AccessRights::RW)?;

        let range = self.command.unwind_range(provider_factory.clone())?;
        if *range.start() == 0 {
            eyre::bail!("Cannot unwind genesis block")
        }

        let highest_static_file_block = provider_factory
            .static_file_provider()
            .get_highest_static_files()
            .max()
            .filter(|highest_static_file_block| highest_static_file_block >= range.start());

        // Execute a pipeline unwind if the start of the range overlaps the existing static
        // files. If that's the case, then copy all available data from MDBX to static files, and
        // only then, proceed with the unwind.
        //
        // We also execute a pipeline unwind if `offline` is specified, because we need to only
        // unwind the data associated with offline stages.
        if highest_static_file_block.is_some() || self.offline {
            if self.offline {
                info!(target: "reth::cli", "Performing an unwind for offline-only data!");
            }

            if let Some(highest_static_file_block) = highest_static_file_block {
                info!(target: "reth::cli", ?range, ?highest_static_file_block, "Executing a pipeline unwind.");
            } else {
                info!(target: "reth::cli", ?range, "Executing a pipeline unwind.");
            }

            // This will build an offline-only pipeline if the `offline` flag is enabled
            let mut pipeline = self.build_pipeline(config, provider_factory)?;

            // Move all applicable data from database to static files.
            pipeline.move_to_static_files()?;

            pipeline.unwind((*range.start()).saturating_sub(1), None)?;
        } else {
            info!(target: "reth::cli", ?range, "Executing a database unwind.");
            let provider = provider_factory.provider_rw()?;

            let _ = provider
                .take_block_and_execution_range(range.clone())
                .map_err(|err| eyre::eyre!("Transaction error on unwind: {err}"))?;

            // update finalized block if needed
            let last_saved_finalized_block_number = provider.last_finalized_block_number()?;
            let range_min =
                range.clone().min().ok_or(eyre::eyre!("Could not fetch lower range end"))?;
            if last_saved_finalized_block_number.is_none() ||
                Some(range_min) < last_saved_finalized_block_number
            {
                provider.save_finalized_block_number(BlockNumber::from(range_min))?;
            }

            provider.commit()?;
        }

        info!(target: "reth::cli", range=?range.clone(), count=range.count(), "Unwound blocks");

        Ok(())
    }

    fn build_pipeline<N: NodeTypesWithDB<ChainSpec = C::ChainSpec>>(
        self,
        config: Config,
        provider_factory: ProviderFactory<N>,
    ) -> Result<Pipeline<N>, eyre::Error> {
        let consensus: Arc<dyn Consensus> =
            Arc::new(EthBeaconConsensus::new(provider_factory.chain_spec()));
        let stage_conf = &config.stages;
        let prune_modes = config.prune.clone().map(|prune| prune.segments).unwrap_or_default();

        let (tip_tx, tip_rx) = watch::channel(B256::ZERO);

        // Unwinding does not require a valid executor
        let executor = NoopBlockExecutorProvider::default();

        let builder = if self.offline {
            Pipeline::<N>::builder().add_stages(
                OfflineStages::new(executor, config.stages, PruneModes::default())
                    .builder()
                    .disable(reth_stages::StageId::SenderRecovery),
            )
        } else {
            Pipeline::<N>::builder().with_tip_sender(tip_tx).add_stages(
                DefaultStages::new(
                    provider_factory.clone(),
                    tip_rx,
                    Arc::clone(&consensus),
                    NoopHeaderDownloader::default(),
                    NoopBodiesDownloader::default(),
                    executor.clone(),
                    stage_conf.clone(),
                    prune_modes.clone(),
                )
                .set(ExecutionStage::new(
                    executor,
                    ExecutionStageThresholds {
                        max_blocks: None,
                        max_changes: None,
                        max_cumulative_gas: None,
                        max_duration: None,
                    },
                    stage_conf.execution_external_clean_threshold(),
                    prune_modes,
                    ExExManagerHandle::empty(),
                )),
            )
        };

        let pipeline = builder.build(
            provider_factory.clone(),
            StaticFileProducer::new(provider_factory, PruneModes::default()),
        );
        Ok(pipeline)
    }
}

/// `reth stage unwind` subcommand
#[derive(Subcommand, Debug, Eq, PartialEq)]
enum Subcommands {
    /// Unwinds the database from the latest block, until the given block number or hash has been
    /// reached, that block is not included.
    #[command(name = "to-block")]
    ToBlock { target: BlockHashOrNumber },
    /// Unwinds the database from the latest block, until the given number of blocks have been
    /// reached.
    #[command(name = "num-blocks")]
    NumBlocks { amount: u64 },
}

impl Subcommands {
    /// Returns the block range to unwind.
    ///
    /// This returns an inclusive range: [target..=latest]
    fn unwind_range<N: ProviderNodeTypes<DB = Arc<DatabaseEnv>>>(
        &self,
        factory: ProviderFactory<N>,
    ) -> eyre::Result<RangeInclusive<u64>> {
        let provider = factory.provider()?;
        let last = provider.last_block_number()?;
        let target = match self {
            Self::ToBlock { target } => match target {
                BlockHashOrNumber::Hash(hash) => provider
                    .block_number(*hash)?
                    .ok_or_else(|| eyre::eyre!("Block hash not found in database: {hash:?}"))?,
                BlockHashOrNumber::Number(num) => *num,
            },
            Self::NumBlocks { amount } => last.saturating_sub(*amount),
        } + 1;
        if target > last {
            eyre::bail!("Target block number is higher than the latest block number")
        }
        Ok(target..=last)
    }
}

#[cfg(test)]
mod tests {
    use reth_ethereum_cli::chainspec::EthereumChainSpecParser;

    use super::*;

    #[test]
    fn parse_unwind() {
        let cmd = Command::<EthereumChainSpecParser>::parse_from([
            "reth",
            "--datadir",
            "dir",
            "to-block",
            "100",
        ]);
        assert_eq!(cmd.command, Subcommands::ToBlock { target: BlockHashOrNumber::Number(100) });

        let cmd = Command::<EthereumChainSpecParser>::parse_from([
            "reth",
            "--datadir",
            "dir",
            "num-blocks",
            "100",
        ]);
        assert_eq!(cmd.command, Subcommands::NumBlocks { amount: 100 });
    }
}
