use crate::{serde_helper::deserialize_opt_prune_mode_with_min_blocks, BlockNumber, PruneMode};
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
        deserialize_with = "deserialize_opt_prune_mode_with_min_blocks::<64, _>"
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
    ($(($part:ident, $human_part:expr, $min_blocks:expr)),+) => {
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
                pub fn [<prune_target_block_ $part>](&self, tip: BlockNumber) -> Option<(BlockNumber, PruneMode)> {
                    self.$part.as_ref().and_then(|mode| {
                        self.prune_target_block(mode, tip, $min_blocks).map(|block| {
                            (block, *mode)
                        })
                    })
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
    /// prune mode, tip block number and minimum number of blocks allowed to be pruned.
    pub fn prune_target_block(
        &self,
        mode: &PruneMode,
        tip: BlockNumber,
        min_blocks: Option<u64>,
    ) -> Option<BlockNumber> {
        match mode {
            PruneMode::Full if min_blocks.unwrap_or_default() == 0 => Some(tip),
            PruneMode::Distance(distance) if *distance >= min_blocks.unwrap_or_default() => {
                Some(tip.saturating_sub(*distance))
            }
            PruneMode::Before(n) if tip.saturating_sub(*n) >= min_blocks.unwrap_or_default() => {
                Some(*n)
            }
            _ => None,
        }
    }

    impl_prune_parts!(
        (sender_recovery, "Sender Recovery", None),
        (transaction_lookup, "Transaction Lookup", None),
        (receipts, "Receipts", Some(64)),
        (account_history, "Account History", None),
        (storage_history, "Storage History", None)
    );
}
