use crate::{
    prune::PrunePartError, serde_helper::deserialize_opt_prune_mode_with_min_blocks, BlockNumber,
    PruneMode, PrunePart,
};
use paste::paste;
use serde::{Deserialize, Serialize};

/// Pruning configuration for every part of the data that can be pruned.
#[derive(Debug, Clone, Default, Copy, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default)]
pub struct PruneModes {
    /// Sender Recovery pruning configuration.
    // TODO(alexey): removing min blocks restriction is possible if we start calculating the senders
    //  dynamically on blockchain tree unwind.
    #[serde(
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_opt_prune_mode_with_min_blocks::<64, _>"
    )]
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
    #[serde(
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_opt_prune_mode_with_min_blocks::<64, _>"
    )]
    pub account_history: Option<PruneMode>,
    /// Storage History pruning configuration.
    #[serde(
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_opt_prune_mode_with_min_blocks::<64, _>"
    )]
    pub storage_history: Option<PruneMode>,
}

macro_rules! impl_prune_parts {
    ($(($part:ident, $variant:ident, $min_blocks:expr)),+) => {
        $(
            paste! {
                #[doc = concat!(
                    "Check if ",
                    stringify!($variant),
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
                    stringify!($variant),
                    " pruning needs to be done, inclusive, according to the provided tip."
                )]
                pub fn [<prune_target_block_ $part>](&self, tip: BlockNumber) -> Result<Option<(BlockNumber, PruneMode)>, PrunePartError> {
                    let min_blocks: u64 = $min_blocks.unwrap_or_default();
                    match self.$part {
                        Some(mode) => Ok(match mode {
                            PruneMode::Full if min_blocks == 0 => Some((tip, mode)),
                            PruneMode::Distance(distance) if distance > tip => None, // Nothing to prune yet
                            PruneMode::Distance(distance) if distance >= min_blocks => Some((tip - distance, mode)),
                            PruneMode::Before(n) if n > tip => None, // Nothing to prune yet
                            PruneMode::Before(n) if tip - n >= min_blocks => Some((n - 1, mode)),
                            _ => return Err(PrunePartError::Configuration(PrunePart::$variant)),
                        }),
                        None => Ok(None)
                    }
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

    impl_prune_parts!(
        (sender_recovery, SenderRecovery, Some(64)),
        (transaction_lookup, TransactionLookup, None),
        (receipts, Receipts, Some(64)),
        (account_history, AccountHistory, Some(64)),
        (storage_history, StorageHistory, Some(64))
    );
}
