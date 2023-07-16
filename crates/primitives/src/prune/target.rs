use crate::{serde_helper::deserialize_opt_prune_mode_with_min_distance, BlockNumber, PruneMode};
use paste::paste;
use serde::{Deserialize, Serialize};

/// Pruning configuration for every part of the data that can be pruned.
#[derive(Debug, Clone, Default, Copy, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default)]
pub struct PruneTargets {
    /// Sender Recovery pruning configuration.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sender_recovery: Option<PruneMode>,
    /// Transaction Lookup pruning configuration.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transaction_lookup: Option<PruneMode>,
    /// Receipts pruning configuration.
    #[serde(
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_opt_prune_mode_with_min_distance::<64, _>"
    )]
    pub receipts: Option<PruneMode>,
    /// Account History pruning configuration.
    #[serde(
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_opt_prune_mode_with_min_distance::<64, _>"
    )]
    pub account_history: Option<PruneMode>,
    /// Storage History pruning configuration.
    #[serde(
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_opt_prune_mode_with_min_distance::<64, _>"
    )]
    pub storage_history: Option<PruneMode>,
}

macro_rules! gen_prune_methods {
    ($($config:ident),+) => {
        $(
            paste! {
                /// Check if target block should be pruned
                pub fn [<should_prune_ $config>](&self, block: BlockNumber, tip: BlockNumber) -> bool {
                    if let Some(config) = &self.$config {
                        return self.should_prune(config, block, tip)
                    }
                    false
                }

                /// At which block should it stop pruning.
                ///
                /// Returns `Some(BlockNumber)` on the first unprunable block, or `None` if everything should be
                /// pruned.
                pub fn [<stop_prune_ $config>](&self, tip: BlockNumber) -> Option<BlockNumber> {
                    if let Some(config) = &self.$config {
                        return self.stop_prune(config, tip)
                    }
                    Some(0)
                }
            }
        )+

        /// Sets pruning to all targets.
        pub fn all() -> Self {
            PruneTargets {
                $(
                    $config: Some(PruneMode::Full),
                )+
            }
        }

    };
}

impl PruneTargets {
    /// Sets pruning to no target.
    pub fn none() -> Self {
        PruneTargets::default()
    }

    /// Check if target block should be pruned
    pub fn should_prune(&self, target: &PruneMode, block: BlockNumber, tip: BlockNumber) -> bool {
        match target {
            PruneMode::Full => true,
            PruneMode::Distance(distance) => {
                if *distance > tip {
                    return false
                }
                block < tip - *distance
            }
            PruneMode::Before(n) => *n > block,
        }
    }

    /// At which block should it stop pruning.
    ///
    /// Returns `Some(BlockNumber)` on the first unprunable block, or `None` if everything should be
    /// pruned.
    pub fn stop_prune(&self, target: &PruneMode, tip: BlockNumber) -> Option<BlockNumber> {
        match target {
            PruneMode::Full => None,
            PruneMode::Distance(distance) => {
                if *distance > tip {
                    return Some(0)
                }
                Some(tip - *distance)
            }
            PruneMode::Before(n) => Some(*n),
        }
    }

    gen_prune_methods!(
        sender_recovery,
        transaction_lookup,
        receipts,
        account_history,
        storage_history
    );
}
