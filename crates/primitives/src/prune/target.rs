use crate::{serde_helper::deserialize_opt_prune_mode_with_min_distance, BlockNumber, PruneMode};
use paste::paste;
use serde::{Deserialize, Serialize};

/// Pruning configuration for every part of the data that can be pruned.
#[derive(Debug, Clone, Default, Copy, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default)]
pub struct PruneModes {
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account_history: Option<PruneMode>,
    /// Storage History pruning configuration.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub storage_history: Option<PruneMode>,
}

macro_rules! impl_prune_parts {
    ($(($part:ident, $human_part:expr)),+) => {
        $(
            paste! {
                #[doc = concat!(
                    "Check if ",
                    $human_part,
                    " should be pruned at the target block according to the provided tip."
                )]
                pub fn [<should_prune_ $part>](&self, block: BlockNumber, tip: BlockNumber) -> bool {
                    if let Some(mode) = &self.$part {
                        return self.should_prune(mode, block, tip)
                    }
                    false
                }
            }
        )+

        $(
            paste! {
                #[doc = concat!(
                    "Returns block up to which ",
                    $human_part,
                    " pruning needs to be done, inclusive, according to the provided tip."
                )]
                pub fn [<prune_to_block_ $part>](&self, tip: BlockNumber) -> Option<(BlockNumber, PruneMode)> {
                    self.$part.as_ref().map(|mode| (self.prune_to_block(mode, tip), *mode))
                }
            }
        )+

        /// Sets pruning to all targets.
        pub fn all() -> Self {
            Self {
                $(
                    $part: Some(PruneMode::Full),
                )+
            }
        }

    };
}

impl PruneModes {
    /// Sets pruning to no target.
    pub fn none() -> Self {
        PruneModes::default()
    }

    /// Check if target block should be pruned according to the provided prune mode and tip.
    pub fn should_prune(&self, mode: &PruneMode, block: BlockNumber, tip: BlockNumber) -> bool {
        match mode {
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

    /// Returns block up to which pruning needs to be done, inclusive, according to the provided
    /// prune mode and tip.
    pub fn prune_to_block(&self, mode: &PruneMode, tip: BlockNumber) -> BlockNumber {
        match mode {
            PruneMode::Full => tip,
            PruneMode::Distance(distance) => tip.saturating_sub(*distance),
            PruneMode::Before(n) => *n,
        }
    }

    impl_prune_parts!(
        (sender_recovery, "Sender Recovery"),
        (transaction_lookup, "Transaction Lookup"),
        (receipts, "Receipts"),
        (account_history, "Account History"),
        (storage_history, "Storage History")
    );
}
