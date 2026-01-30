//! Command that runs pruning with a configurable delete limit.
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
use tracing::{info, warn};

/// Default delete limit per prune run to prevent OOM when static files are involved.
const DEFAULT_DELETE_LIMIT: usize = 100_000;

/// Prunes according to the configuration with a configurable delete limit.
///
/// For edge builds (compiled with `--features edge`), this command defaults to limiting
/// deletions to 100,000 entries per pruner run to prevent OOM with static file operations.
/// For legacy builds, no limit is applied by default.
///
/// Use `--delete-limit <N>` to override the default, or `--delete-limit 0` for unlimited.
#[derive(Debug, Parser)]
pub struct PruneCommand<C: ChainSpecParser> {
    #[command(flatten)]
    env: EnvironmentArgs<C>,

    /// Prometheus metrics configuration.
    #[command(flatten)]
    metrics: MetricArgs,

    /// Maximum number of entries to delete per pruner run (across all segments).
    ///
    /// For edge builds, defaults to 100,000 to prevent OOM.
    /// For legacy builds, defaults to unlimited.
    /// Set to 0 for unlimited deletions per run.
    #[arg(long)]
    delete_limit: Option<usize>,
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

        // Resolve delete limit based on build configuration:
        // - If user specified a limit, use it (0 means unlimited)
        // - Edge builds default to batched deletions to prevent OOM with static files
        // - Legacy builds default to unlimited (no OOM risk with MDBX-only)
        let delete_limit = match self.delete_limit {
            Some(0) => usize::MAX,
            Some(limit) => limit,
            #[cfg(feature = "edge")]
            None => DEFAULT_DELETE_LIMIT,
            #[cfg(not(feature = "edge"))]
            None => usize::MAX,
        };

        // Copy data from database to static files
        info!(target: "reth::cli", "Copying data from database to static files...");
        let static_file_producer =
            StaticFileProducer::new(provider_factory.clone(), config.segments.clone());
        let lowest_static_file_height =
            static_file_producer.lock().copy_to_static_files()?.min_block_num();
        info!(target: "reth::cli", ?lowest_static_file_height, "Copied data from database to static files");

        // Delete data which has been copied to static files.
        if let Some(prune_tip) = lowest_static_file_height {
            info!(target: "reth::cli", ?prune_tip, ?config, ?delete_limit, "Pruning data from database...");
            let mut pruner = PrunerBuilder::new(config)
                .delete_limit(delete_limit)
                .build_with_provider_factory(provider_factory);

            // Loop until pruning is complete, respecting delete_limit per iteration
            let mut total_pruned = 0usize;
            let mut runs = 0usize;
            let mut consecutive_zero_runs = 0usize;
            loop {
                runs += 1;
                let output = pruner.run(prune_tip)?;

                let pruned_this_run: usize =
                    output.segments.iter().map(|(_, seg)| seg.pruned).sum();
                total_pruned += pruned_this_run;

                if output.progress.is_finished() {
                    info!(target: "reth::cli", runs, total_pruned, "Pruned data from database");
                    break;
                }

                // Stuck-loop guard: bail after consecutive runs with no progress
                if pruned_this_run == 0 {
                    consecutive_zero_runs += 1;
                    if consecutive_zero_runs >= 2 {
                        warn!(target: "reth::cli", runs, total_pruned, consecutive_zero_runs, "Pruner returned HasMoreData but made no progress");
                        eyre::bail!(
                            "Pruner returned HasMoreData but made no progress after \
                             {consecutive_zero_runs} consecutive runs. \
                             Try increasing --delete-limit or use --delete-limit 0 (unlimited)."
                        );
                    }
                } else {
                    consecutive_zero_runs = 0;
                }

                info!(target: "reth::cli", runs, pruned_this_run, total_pruned, "Pruner has more data, continuing...");
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
