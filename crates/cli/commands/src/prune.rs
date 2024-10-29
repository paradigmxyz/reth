//! Command that runs pruning without any limits.
use crate::common::{AccessRights, Environment, EnvironmentArgs};
use clap::Parser;
use reth_chainspec::{EthChainSpec, EthereumHardforks};
use reth_cli::chainspec::ChainSpecParser;
use reth_node_builder::NodeTypesWithEngine;
use reth_prune::PrunerBuilder;
use reth_static_file::StaticFileProducer;
use tracing::info;

/// Prunes according to the configuration without any limits
#[derive(Debug, Parser)]
pub struct PruneCommand<C: ChainSpecParser> {
    #[command(flatten)]
    env: EnvironmentArgs<C>,
}

impl<C: ChainSpecParser<ChainSpec: EthChainSpec + EthereumHardforks>> PruneCommand<C> {
    /// Execute the `prune` command
    pub async fn execute<N: NodeTypesWithEngine<ChainSpec = C::ChainSpec>>(
        self,
    ) -> eyre::Result<()> {
        let Environment { config, provider_factory, .. } = self.env.init::<N>(AccessRights::RW)?;
        let prune_config = config.prune.unwrap_or_default();

        // Copy data from database to static files
        info!(target: "reth::cli", "Copying data from database to static files...");
        let static_file_producer =
            StaticFileProducer::new(provider_factory.clone(), prune_config.segments.clone());
        let lowest_static_file_height = static_file_producer.lock().copy_to_static_files()?.min();
        info!(target: "reth::cli", ?lowest_static_file_height, "Copied data from database to static files");

        // Delete data which has been copied to static files.
        if let Some(prune_tip) = lowest_static_file_height {
            info!(target: "reth::cli", ?prune_tip, ?prune_config, "Pruning data from database...");
            // Run the pruner according to the configuration, and don't enforce any limits on it
            let mut pruner = PrunerBuilder::new(prune_config)
                .delete_limit(usize::MAX)
                .build_with_provider_factory(provider_factory);

            pruner.run(prune_tip)?;
            info!(target: "reth::cli", "Pruned data from database");
        }

        Ok(())
    }
}
