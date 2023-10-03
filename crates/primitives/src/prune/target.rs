use crate::{
    prune::PruneSegmentError, serde_helper::deserialize_opt_prune_mode_with_min_blocks,
    BlockNumber, PruneMode, PruneSegment, ReceiptsLogPruneConfig,
};
use paste::paste;
use serde::{Deserialize, Serialize};

/// Minimum distance from the tip necessary for the node to work correctly:
/// 1. Minimum 2 epochs (32 blocks per epoch) required to handle any reorg according to the
///    consensus protocol.
/// 2. Another 10k blocks to have a room for maneuver in case when things go wrong and a manual
///    unwind is required.
pub const MINIMUM_PRUNING_DISTANCE: u64 = 32 * 2 + 10_000;

/// Pruning configuration for every segment of the data that can be pruned.
#[derive(Debug, Clone, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default)]
pub struct PruneModes {
    /// Sender Recovery pruning configuration.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sender_recovery: Option<PruneMode>,
    /// Transaction Lookup pruning configuration.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transaction_lookup: Option<PruneMode>,
    /// Receipts pruning configuration. This setting overrides `receipts_log_filter`
    /// and offers improved performance.
    #[serde(
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_opt_prune_mode_with_min_blocks::<MINIMUM_PRUNING_DISTANCE, _>"
    )]
    pub receipts: Option<PruneMode>,
    /// Account History pruning configuration.
    #[serde(
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_opt_prune_mode_with_min_blocks::<MINIMUM_PRUNING_DISTANCE, _>"
    )]
    pub account_history: Option<PruneMode>,
    /// Storage History pruning configuration.
    #[serde(
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_opt_prune_mode_with_min_blocks::<MINIMUM_PRUNING_DISTANCE, _>"
    )]
    pub storage_history: Option<PruneMode>,
    /// Receipts pruning configuration by retaining only those receipts that contain logs emitted
    /// by the specified addresses, discarding others. This setting is overridden by `receipts`.
    ///
    /// The [`BlockNumber`] represents the starting block from which point onwards the receipts are
    /// preserved.
    pub receipts_log_filter: ReceiptsLogPruneConfig,
}

macro_rules! impl_prune_segments {
    ($(($segment:ident, $variant:ident, $min_blocks:expr)),+) => {
        $(
            paste! {
                #[doc = concat!(
                    "Check if ",
                    stringify!($variant),
                    " should be pruned at the target block according to the provided tip."
                )]
                pub fn [<should_prune_ $segment>](&self, block: BlockNumber, tip: BlockNumber) -> bool {
                    if let Some(mode) = &self.$segment {
                        return mode.should_prune(block, tip)
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
                pub fn [<prune_target_block_ $segment>](&self, tip: BlockNumber) -> Result<Option<(BlockNumber, PruneMode)>, PruneSegmentError> {
                     match self.$segment {
                        Some(mode) => mode.prune_target_block(tip, $min_blocks.unwrap_or_default(), PruneSegment::$variant),
                        None => Ok(None)
                    }
                }
            }
        )+

        /// Sets pruning to all targets.
        pub fn all() -> Self {
            Self {
                $(
                    $segment: Some(PruneMode::Full),
                )+
                receipts_log_filter: Default::default()
            }
        }

    };
}

impl PruneModes {
    /// Sets pruning to no target.
    pub fn none() -> Self {
        PruneModes::default()
    }

    impl_prune_segments!(
        (sender_recovery, SenderRecovery, None),
        (transaction_lookup, TransactionLookup, None),
        (receipts, Receipts, Some(MINIMUM_PRUNING_DISTANCE)),
        (account_history, AccountHistory, Some(MINIMUM_PRUNING_DISTANCE)),
        (storage_history, StorageHistory, Some(MINIMUM_PRUNING_DISTANCE))
    );
}
