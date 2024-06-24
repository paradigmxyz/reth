//! Command that runs pruning without any limits.

use crate::commands::common::{AccessRights, Environment, EnvironmentArgs};
use clap::Parser;
use reth_provider::StageCheckpointReader;
use reth_prune::PrunerBuilder;
use reth_stages::StageId;
use reth_static_file::{HighestStaticFiles, StaticFileProducer};

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
        let prune_config = config.prune.unwrap_or_default();

        let static_file_producer =
            StaticFileProducer::new(provider_factory.clone(), prune_config.segments.clone());
        let static_file_producer = static_file_producer.lock();

        // Copies data from database to static files
        let lowest_static_file_height = {
            let provider = provider_factory.provider()?;
            let stages_checkpoints = [StageId::Headers, StageId::Execution, StageId::Bodies]
                .into_iter()
                .map(|stage| {
                    provider.get_stage_checkpoint(stage).map(|c| c.map(|c| c.block_number))
                })
                .collect::<Result<Vec<_>, _>>()?;

            let targets = static_file_producer.get_static_file_targets(HighestStaticFiles {
                headers: stages_checkpoints[0],
                receipts: stages_checkpoints[1],
                transactions: stages_checkpoints[2],
            })?;
            static_file_producer.run(targets)?;
            stages_checkpoints.into_iter().min().expect("exists")
        };

        // Deletes data which has been copied to static files.
        if let Some(prune_tip) = lowest_static_file_height {
            // Run the pruner according to the configuration, and don't enforce any limits on it
            let mut pruner = PrunerBuilder::new(prune_config)
                .prune_delete_limit(usize::MAX)
                .build(provider_factory);

            pruner.run(prune_tip)?;
        }

        Ok(())
    }
}
