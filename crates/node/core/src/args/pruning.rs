//! Pruning and full node arguments

use clap::Args;
use reth_chainspec::ChainSpec;
use reth_config::config::PruneConfig;
use reth_prune_types::{PruneMode, PruneModes, ReceiptsLogPruneConfig, MINIMUM_PRUNING_DISTANCE};

/// Parameters for pruning and full node
#[derive(Debug, Clone, Args, PartialEq, Eq, Default)]
#[command(next_help_heading = "Pruning")]
pub struct PruningArgs {
    /// Run full node. Only the most recent [`MINIMUM_PRUNING_DISTANCE`] block states are stored.
    /// This flag takes priority over pruning configuration in reth.toml.
    #[arg(long, default_value_t = false)]
    pub full: bool,
}

impl PruningArgs {
    /// Returns pruning configuration.
    pub fn prune_config(&self, chain_spec: &ChainSpec) -> Option<PruneConfig> {
        if !self.full {
            return None
        }

        Some(PruneConfig {
            block_interval: 5,
            segments: PruneModes {
                sender_recovery: Some(PruneMode::Full),
                transaction_lookup: None,
                // prune all receipts if chain doesn't have deposit contract specified in chain spec
                receipts: chain_spec
                    .deposit_contract
                    .as_ref()
                    .map(|contract| PruneMode::Before(contract.block))
                    .or(Some(PruneMode::Full)),
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    /// A helper type to parse Args more easily
    #[derive(Parser)]
    struct CommandParser<T: Args> {
        #[command(flatten)]
        args: T,
    }

    #[test]
    fn pruning_args_sanity_check() {
        let default_args = PruningArgs::default();
        let args = CommandParser::<PruningArgs>::parse_from(["reth"]).args;
        assert_eq!(args, default_args);
    }
}
