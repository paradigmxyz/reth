//! Command that initializes the node from a genesis file.

use crate::commands::common::{AccessRights, Environment, EnvironmentArgs};
use clap::Parser;
use reth_provider::BlockHashReader;
use reth_prune::PrunerBuilder;
use tracing::info;

#[derive(Debug, Parser)]
pub struct PruneCommand {
    #[command(flatten)]
    env: EnvironmentArgs,
}

impl PruneCommand {
    /// Execute the `prune` command
    pub async fn execute(self) -> eyre::Result<()> {
        let Environment { config, provider_factory, .. } = self.env.init(AccessRights::RW)?;

        let pruner = PrunerBuilder::new(config.prune.clone().unwrap_or_default())
            .prune_delete_limit(usize::MAX)
            .build(provider_factory);

        let hash = provider_factory
            .block_hash(0)?
            .ok_or_else(|| eyre::eyre!("Genesis hash not found."))?;

        info!(target: "reth::cli", hash = ?hash, "Genesis block written");
        Ok(())
    }
}
