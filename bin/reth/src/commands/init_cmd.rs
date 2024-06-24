//! Command that initializes the node from a genesis file.

use crate::commands::common::{AccessRights, Environment, EnvironmentArgs};
use clap::Parser;
use reth_provider::BlockHashReader;
use tracing::info;

/// Initializes the database with the genesis block.
#[derive(Debug, Parser)]
pub struct InitCommand {
    #[command(flatten)]
    env: EnvironmentArgs,
}

impl InitCommand {
    /// Execute the `init` command
    pub async fn execute(self) -> eyre::Result<()> {
        info!(target: "reth::cli", "reth init starting");

        let Environment { provider_factory, .. } = self.env.init(AccessRights::RW)?;

        let hash = provider_factory
            .block_hash(0)?
            .ok_or_else(|| eyre::eyre!("Genesis hash not found."))?;

        info!(target: "reth::cli", hash = ?hash, "Genesis block written");
        Ok(())
    }
}
