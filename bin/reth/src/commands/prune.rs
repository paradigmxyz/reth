//! Command that runs pruning without any limits.

use crate::commands::common::{AccessRights, Environment, EnvironmentArgs};
use clap::Parser;
use reth_provider::BlockNumReader;
use reth_prune::PrunerBuilder;

/// Prunes according to the configuration without any limits
#[derive(Debug, Parser)]
pub struct PruneCommand {
    #[command(flatten)]
    env: EnvironmentArgs,
}

impl PruneCommand {
    /// Execute the `prune` command
    pub async fn execute(self) -> eyre::Result<()> {
        let Environment { config, provider_factory, .. } = self.env.init(AccessRights::RW)?;

        let tip_block_number = provider_factory.best_block_number()?;

        let mut pruner = PrunerBuilder::new(config.prune.unwrap_or_default())
            .prune_delete_limit(usize::MAX)
            .build(provider_factory);

        pruner.run(tip_block_number)?;

        Ok(())
    }
}
