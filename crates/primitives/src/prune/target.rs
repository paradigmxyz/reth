use crate::{
    serde_helper::deserialize_opt_prune_mode_with_min_blocks, PruneMode, ReceiptsLogPruneConfig,
};
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
    /// The [BlockNumber](`crate::BlockNumber`) represents the starting block from which point
    /// onwards the receipts are preserved.
    pub receipts_log_filter: ReceiptsLogPruneConfig,
}

impl PruneModes {
    /// Sets pruning to no target.
    pub fn none() -> Self {
        PruneModes::default()
    }

    /// Sets pruning to all targets.
    pub fn all() -> Self {
        Self {
            sender_recovery: Some(PruneMode::Full),
            transaction_lookup: Some(PruneMode::Full),
            receipts: Some(PruneMode::Full),
            account_history: Some(PruneMode::Full),
            storage_history: Some(PruneMode::Full),
            receipts_log_filter: Default::default(),
        }
    }
}
