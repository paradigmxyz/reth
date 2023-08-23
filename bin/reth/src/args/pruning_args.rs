//! Pruning and full node arguments

use clap::Args;
use reth_config::config::PruneConfig;
use reth_primitives::{
    ChainSpec, PruneMode, PruneModes, ReceiptsLogPruneConfig, MINIMUM_PRUNING_DISTANCE,
};
use std::sync::Arc;

/// Parameters for pruning and full node
#[derive(Debug, Args, PartialEq, Default)]
#[command(next_help_heading = "Pruning")]
pub struct PruningArgs {
    /// Run full node. Only the most recent 128 block states are stored. This flag takes
    /// priority over pruning configuration in reth.toml.
    #[arg(long, default_value_t = false)]
    pub full: bool,
}

impl PruningArgs {
    /// Returns pruning configuration.
    pub fn prune_config(&self, chain_spec: Arc<ChainSpec>) -> eyre::Result<Option<PruneConfig>> {
        Ok(if self.full {
            Some(PruneConfig {
                block_interval: 5,
                parts: PruneModes {
                    sender_recovery: Some(PruneMode::Distance(MINIMUM_PRUNING_DISTANCE)),
                    transaction_lookup: None,
                    receipts: chain_spec
                        .deposit_contract
                        .as_ref()
                        .map(|contract| PruneMode::Before(contract.block)),
                    account_history: Some(PruneMode::Distance(MINIMUM_PRUNING_DISTANCE)),
                    storage_history: Some(PruneMode::Distance(MINIMUM_PRUNING_DISTANCE)),
                    receipts_log_filter: ReceiptsLogPruneConfig(
                        chain_spec
                            .deposit_contract
                            .as_ref()
                            .map(|contract| (contract.address, PruneMode::Before(contract.block)))
                            .into_iter()
                            .collect(),
                    ),
                },
            })
        } else {
            None
        })
    }
}
