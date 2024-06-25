//! Command that runs pruning without any limits.

use crate::commands::common::{AccessRights, Environment, EnvironmentArgs};
use clap::{builder::TypedValueParser, Parser};
use reth_provider::BlockNumReader;
use reth_prune::{PruneModes, PruneSegment, PrunerBuilder};
use strum::VariantNames;

/// Prunes according to the configuration without any limits
#[derive(Debug, Parser)]
pub struct PruneCommand {
    #[command(flatten)]
    env: EnvironmentArgs,

    #[arg(value_parser = clap::builder::PossibleValuesParser::new(PruneSegment::VARIANTS).map(|s| s.parse::<PruneSegment>().unwrap()))]
    segment: PruneSegment,
}

impl PruneCommand {
    /// Execute the `prune` command
    pub async fn execute(self) -> eyre::Result<()> {
        let Environment { config, provider_factory, .. } = self.env.init(AccessRights::RW)?;
        let mut prune_config = config.prune.unwrap_or_default();

        match self.segment {
            PruneSegment::SenderRecovery => {
                prune_config.segments = PruneModes {
                    sender_recovery: prune_config.segments.sender_recovery,
                    ..Default::default()
                }
            }
            PruneSegment::TransactionLookup => {
                prune_config.segments = PruneModes {
                    transaction_lookup: prune_config.segments.transaction_lookup,
                    ..Default::default()
                }
            }
            PruneSegment::Receipts => {
                prune_config.segments = PruneModes {
                    receipts: prune_config.segments.receipts,
                    receipts_log_filter: prune_config.segments.receipts_log_filter,
                    ..Default::default()
                }
            }
            PruneSegment::ContractLogs => {
                prune_config.segments = PruneModes {
                    receipts_log_filter: prune_config.segments.receipts_log_filter,
                    ..Default::default()
                }
            }
            PruneSegment::AccountHistory => {
                prune_config.segments = PruneModes {
                    account_history: prune_config.segments.account_history,
                    ..Default::default()
                }
            }
            PruneSegment::StorageHistory => {
                prune_config.segments = PruneModes {
                    storage_history: prune_config.segments.storage_history,
                    ..Default::default()
                }
            }
            PruneSegment::Headers | PruneSegment::Transactions => {
                eyre::bail!("Prune segment not supported")
            }
        }

        let mut pruner = PrunerBuilder::new(prune_config)
            .prune_delete_limit(usize::MAX)
            .build(provider_factory.clone());
        pruner.run(provider_factory.best_block_number()?)?;

        Ok(())
    }
}
