//! Command that runs pruning.
use crate::common::{CliNodeTypes, EnvironmentArgs};
use clap::Parser;
use reth_chainspec::{ChainSpecProvider, EthChainSpec, EthereumHardforks};
use reth_cli::chainspec::ChainSpecParser;
use reth_cli_runner::CliContext;
use reth_cli_util::cancellation::CancellationToken;
use reth_node_builder::common::metrics_hooks;
use reth_node_core::{args::MetricArgs, version::version_metadata};
use reth_node_metrics::{
    chain::ChainSpecInfo,
    server::{MetricServer, MetricServerConfig},
    version::VersionInfo,
};
#[cfg(all(unix, feature = "rocksdb"))]
use reth_provider::RocksDBProviderFactory;
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
        let env = self.env.init::<N>()?;
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
                ctx.task_executor.clone(),
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

            // Set up cancellation token for graceful shutdown on Ctrl+C
            let cancellation = CancellationToken::new();
            let cancellation_clone = cancellation.clone();
            ctx.task_executor.spawn_critical_task("prune-ctrl-c", async move {
                tokio::signal::ctrl_c().await.expect("failed to listen for ctrl-c");
                cancellation_clone.cancel();
            });

            // Use batched pruning with a limit to bound memory, running in a loop until complete.
            //
            // A limit of 20_000_000 results in a max memory usage of ~5G.
            const DELETE_LIMIT: usize = 20_000_000;
            let mut pruner = PrunerBuilder::new(config)
                .delete_limit(DELETE_LIMIT)
                .build_with_provider_factory(provider_factory.clone());

            let mut total_pruned = 0usize;
            loop {
                if cancellation.is_cancelled() {
                    info!(target: "reth::cli", total_pruned, "Pruning interrupted by user");
                    break;
                }

                let output = pruner.run(prune_tip)?;
                let batch_pruned: usize = output.segments.iter().map(|(_, seg)| seg.pruned).sum();
                total_pruned = total_pruned.saturating_add(batch_pruned);

                if output.progress.is_finished() {
                    info!(target: "reth::cli", total_pruned, "Pruned data from database");
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
        }

        // Flush and compact RocksDB to reclaim disk space after pruning
        #[cfg(all(unix, feature = "rocksdb"))]
        {
            info!(target: "reth::cli", "Flushing and compacting RocksDB...");
            provider_factory.rocksdb_provider().flush_and_compact()?;
            info!(target: "reth::cli", "RocksDB compaction complete");
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
