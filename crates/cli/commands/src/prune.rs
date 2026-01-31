//! Command that runs pruning.
use crate::common::{AccessRights, CliNodeTypes, EnvironmentArgs};
use clap::Parser;
use reth_chainspec::{ChainSpecProvider, EthChainSpec, EthereumHardforks};
use reth_cli::chainspec::ChainSpecParser;
use reth_cli_runner::CliContext;
use reth_node_builder::common::metrics_hooks;
use reth_node_core::{args::MetricArgs, version::version_metadata};
use reth_node_metrics::{
    chain::ChainSpecInfo,
    server::{MetricServer, MetricServerConfig},
    version::VersionInfo,
};
use reth_prune::PrunerBuilder;
use reth_static_file::StaticFileProducer;
use std::sync::Arc;
use tracing::info;

/// Prunes according to the configuration
#[derive(Debug, Parser)]
pub struct PruneCommand<C: ChainSpecParser> {
    #[command(flatten)]
    env: EnvironmentArgs<C>,

    /// Prometheus metrics configuration.
    #[command(flatten)]
    metrics: MetricArgs,
}

impl<C: ChainSpecParser<ChainSpec: EthChainSpec + EthereumHardforks>> PruneCommand<C> {
    /// Execute the `prune` command
    pub async fn execute<N: CliNodeTypes<ChainSpec = C::ChainSpec>>(
        self,
        ctx: CliContext,
    ) -> eyre::Result<()> {
        let env = self.env.init::<N>(AccessRights::RW)?;
        let provider_factory = env.provider_factory;
        let config = env.config.prune;
        let data_dir = env.data_dir;

        if let Some(listen_addr) = self.metrics.prometheus {
            let config = MetricServerConfig::new(
                listen_addr,
                VersionInfo {
                    version: version_metadata().cargo_pkg_version.as_ref(),
                    build_timestamp: version_metadata().vergen_build_timestamp.as_ref(),
                    cargo_features: version_metadata().vergen_cargo_features.as_ref(),
                    git_sha: version_metadata().vergen_git_sha.as_ref(),
                    target_triple: version_metadata().vergen_cargo_target_triple.as_ref(),
                    build_profile: version_metadata().build_profile_name.as_ref(),
                },
                ChainSpecInfo { name: provider_factory.chain_spec().chain().to_string() },
                ctx.task_executor,
                metrics_hooks(&provider_factory),
                data_dir.pprof_dumps(),
            );

            MetricServer::new(config).serve().await?;
        }

        // Copy data from database to static files
        info!(target: "reth::cli", "Copying data from database to static files...");
        let static_file_producer =
            StaticFileProducer::new(provider_factory.clone(), config.segments.clone());
        let lowest_static_file_height =
            static_file_producer.lock().copy_to_static_files()?.min_block_num();
        info!(target: "reth::cli", ?lowest_static_file_height, "Copied data from database to static files");

        // Delete data which has been copied to static files.
        if let Some(prune_tip) = lowest_static_file_height {
            info!(target: "reth::cli", ?prune_tip, ?config, "Pruning data from database...");

            // With edge feature (static files), use batched pruning with a limit to bound memory.
            // Run in a loop until all data is pruned.
            #[cfg(feature = "edge")]
            {
                const DELETE_LIMIT: usize = 200_000;
                let mut pruner = PrunerBuilder::new(config.clone())
                    .delete_limit(DELETE_LIMIT)
                    .build_with_provider_factory(provider_factory);

                let mut total_pruned = 0usize;
                loop {
                    let output = pruner.run(prune_tip)?;
                    let batch_pruned: usize =
                        output.segments.iter().map(|(_, seg)| seg.pruned).sum();
                    total_pruned = total_pruned.saturating_add(batch_pruned);

                    if output.progress.is_finished() {
                        break;
                    }

                    if batch_pruned == 0 {
                        return Err(eyre::eyre!(
                            "pruner made no progress but reported more data remaining; \
                             aborting to prevent infinite loop"
                        ));
                    }

                    info!(
                        target: "reth::cli",
                        batch_pruned,
                        total_pruned,
                        "Pruning batch complete, continuing..."
                    );
                }
                info!(target: "reth::cli", total_pruned, "Pruned data from database");
            }

            #[cfg(not(feature = "edge"))]
            {
                let mut pruner = PrunerBuilder::new(config)
                    .delete_limit(usize::MAX)
                    .build_with_provider_factory(provider_factory);
                pruner.run(prune_tip)?;
                info!(target: "reth::cli", "Pruned data from database");
            }
        }

        Ok(())
    }
}

impl<C: ChainSpecParser> PruneCommand<C> {
    /// Returns the underlying chain being used to run this command
    pub fn chain_spec(&self) -> Option<&Arc<C::ChainSpec>> {
        Some(&self.env.chain)
    }
}
