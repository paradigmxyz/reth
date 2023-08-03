//! Pruning and full node arguments

use clap::Args;
use reth_config::config::PruneConfig;
use reth_primitives::{ChainSpec, PruneMode, PruneModes};
use std::sync::Arc;

/// Parameters for pruning and full node
#[derive(Debug, Args, PartialEq, Default)]
#[command(next_help_heading = "Pruning")]
pub struct PruningArgs {
    /// Run full node. Only the most recent 128 block states are stored. This flag takes
    /// priority over pruning configuration in reth.toml.
    // TODO(alexey): unhide when pruning is ready for production use
    #[arg(long, hide = true, default_value_t = false)]
    pub full: bool,
}

impl PruningArgs {
    /// Returns pruning configuration.
    pub fn prune_config(&self, chain_spec: Arc<ChainSpec>) -> Option<PruneConfig> {
        if self.full {
            Some(PruneConfig {
                block_interval: 5,
                parts: PruneModes {
                    sender_recovery: Some(PruneMode::Distance(128)),
                    transaction_lookup: None,
                    receipts: chain_spec
                        .deposit_contract
                        .as_ref()
                        .map(|contract| PruneMode::Before(contract.block)),
                    account_history: Some(PruneMode::Distance(128)),
                    storage_history: Some(PruneMode::Distance(128)),
                },
            })
        } else {
            None
        }
    }
}
