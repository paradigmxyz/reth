use crate::{BlockNumber, PruneMode};
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub receipts: Option<PruneMode>,
    /// Account History pruning configuration.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account_history: Option<PruneMode>,
    /// Storage History pruning configuration.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub storage_history: Option<PruneMode>,
    /// Current known tip to calculate pruning distances.
    #[serde(skip_serializing)]
    pub tip: Option<BlockNumber>,
}

macro_rules! should_prune_method {
    ($($config:ident),+) => {
        $(
            paste! {
                #[allow(missing_docs)]
                pub fn [<should_prune_ $config>](&self, block: BlockNumber) -> bool {
                    if let Some(config) = &self.$config {
                        return self.should_prune(config, block)
                    }
                    false
                }
            }
        )+

        /// Sets pruning to all targets.
        pub fn all() -> Self {
            PruneTargets {
                tip: None,
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

    /// Creates a `PruneTargets` with a specific tip.
    pub fn with_tip(mut self, tip: Option<BlockNumber>) -> Self {
        self.tip = tip;
        self
    }

    /// Updates the tip if it is of higher value.
    pub fn update_tip(&mut self, tip: BlockNumber) {
        if self.tip.is_none() || self.tip.is_some_and(|inner| inner < tip) {
            self.tip = Some(tip);
        }
    }

    /// Check if target block should be pruned
    pub fn should_prune(&self, target: &PruneMode, block: BlockNumber) -> bool {
        match target {
            PruneMode::Full => true,
            PruneMode::Distance(distance) => {
                let tip = self.tip.expect("tip should be set on PruneTargets.");
                if *distance > tip {
                    return false
                }
                block < tip - *distance
            }
            PruneMode::Before(n) => *n > block,
        }
    }

    should_prune_method!(
        sender_recovery,
        transaction_lookup,
        receipts,
        account_history,
        storage_history
    );
}
