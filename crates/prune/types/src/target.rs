use alloy_primitives::BlockNumber;
use derive_more::Display;
use thiserror::Error;

use crate::{PruneMode, ReceiptsLogPruneConfig};

/// Minimum distance from the tip necessary for the node to work correctly:
/// 1. Minimum 2 epochs (32 blocks per epoch) required to handle any reorg according to the
///    consensus protocol.
/// 2. Another 10k blocks to have a room for maneuver in case when things go wrong and a manual
///    unwind is required.
pub const MINIMUM_PRUNING_DISTANCE: u64 = 32 * 2 + 10_000;

/// Type of history that can be pruned
#[derive(Debug, Error, PartialEq, Eq, Clone)]
pub enum UnwindTargetPrunedError {
    /// The target block is beyond the history limit
    #[error("Cannot unwind to block {target_block} as it is beyond the {history_type} limit. Latest block: {latest_block}, History limit: {limit}")]
    TargetBeyondHistoryLimit {
        /// The latest block number
        latest_block: BlockNumber,
        /// The target block number
        target_block: BlockNumber,
        /// The type of history that is beyond the limit
        history_type: HistoryType,
        /// The limit of the history
        limit: u64,
    },
}

#[derive(Debug, Display, Clone, PartialEq, Eq)]
pub enum HistoryType {
    /// Account history
    AccountHistory,
    /// Storage history
    StorageHistory,
}

/// Pruning configuration for every segment of the data that can be pruned.
#[derive(Debug, Clone, Default, Eq, PartialEq)]
#[cfg_attr(any(test, feature = "serde"), derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(any(test, feature = "serde"), serde(default))]
pub struct PruneModes {
    /// Sender Recovery pruning configuration.
    #[cfg_attr(any(test, feature = "serde"), serde(skip_serializing_if = "Option::is_none"))]
    pub sender_recovery: Option<PruneMode>,
    /// Transaction Lookup pruning configuration.
    #[cfg_attr(any(test, feature = "serde"), serde(skip_serializing_if = "Option::is_none"))]
    pub transaction_lookup: Option<PruneMode>,
    /// Receipts pruning configuration. This setting overrides `receipts_log_filter`
    /// and offers improved performance.
    #[cfg_attr(
        any(test, feature = "serde"),
        serde(
            skip_serializing_if = "Option::is_none",
            deserialize_with = "deserialize_opt_prune_mode_with_min_blocks::<MINIMUM_PRUNING_DISTANCE, _>"
        )
    )]
    pub receipts: Option<PruneMode>,
    /// Account History pruning configuration.
    #[cfg_attr(
        any(test, feature = "serde"),
        serde(
            skip_serializing_if = "Option::is_none",
            deserialize_with = "deserialize_opt_prune_mode_with_min_blocks::<MINIMUM_PRUNING_DISTANCE, _>"
        )
    )]
    pub account_history: Option<PruneMode>,
    /// Storage History pruning configuration.
    #[cfg_attr(
        any(test, feature = "serde"),
        serde(
            skip_serializing_if = "Option::is_none",
            deserialize_with = "deserialize_opt_prune_mode_with_min_blocks::<MINIMUM_PRUNING_DISTANCE, _>"
        )
    )]
    pub storage_history: Option<PruneMode>,
    /// Bodies History pruning configuration.
    #[cfg_attr(
        any(test, feature = "serde"),
        serde(
            skip_serializing_if = "Option::is_none",
            deserialize_with = "deserialize_opt_prune_mode_with_min_blocks::<MINIMUM_PRUNING_DISTANCE, _>"
        )
    )]
    pub bodies_history: Option<PruneMode>,
    /// Receipts pruning configuration by retaining only those receipts that contain logs emitted
    /// by the specified addresses, discarding others. This setting is overridden by `receipts`.
    ///
    /// The [`BlockNumber`](`crate::BlockNumber`) represents the starting block from which point
    /// onwards the receipts are preserved.
    pub receipts_log_filter: ReceiptsLogPruneConfig,
}

impl PruneModes {
    /// Sets pruning to no target.
    pub fn none() -> Self {
        Self::default()
    }

    /// Sets pruning to all targets.
    pub fn all() -> Self {
        Self {
            sender_recovery: Some(PruneMode::Full),
            transaction_lookup: Some(PruneMode::Full),
            receipts: Some(PruneMode::Full),
            account_history: Some(PruneMode::Full),
            storage_history: Some(PruneMode::Full),
            bodies_history: Some(PruneMode::Full),
            receipts_log_filter: Default::default(),
        }
    }

    /// Returns whether there is any kind of receipt pruning configuration.
    pub fn has_receipts_pruning(&self) -> bool {
        self.receipts.is_some() || !self.receipts_log_filter.is_empty()
    }

    /// Returns true if all prune modes are set to [`None`].
    pub fn is_empty(&self) -> bool {
        self == &Self::none()
    }

    /// Returns true if target block is within history limit
    pub fn ensure_unwind_target_unpruned(
        &self,
        latest_block: u64,
        target_block: u64,
    ) -> Result<(), UnwindTargetPrunedError> {
        let distance = latest_block.saturating_sub(target_block);
        [
            (self.account_history, HistoryType::AccountHistory),
            (self.storage_history, HistoryType::StorageHistory),
        ]
        .iter()
        .find_map(|(prune_mode, history_type)| {
            if let Some(PruneMode::Distance(limit)) = prune_mode {
                (distance > *limit).then_some(Err(
                    UnwindTargetPrunedError::TargetBeyondHistoryLimit {
                        latest_block,
                        target_block,
                        history_type: history_type.clone(),
                        limit: *limit,
                    },
                ))
            } else {
                None
            }
        })
        .unwrap_or(Ok(()))
    }
}

/// Deserializes [`Option<PruneMode>`] and validates that the value is not less than the const
/// generic parameter `MIN_BLOCKS`. This parameter represents the number of blocks that needs to be
/// left in database after the pruning.
///
/// 1. For [`PruneMode::Full`], it fails if `MIN_BLOCKS > 0`.
/// 2. For [`PruneMode::Distance(distance`)], it fails if `distance < MIN_BLOCKS + 1`. `+ 1` is
///    needed because `PruneMode::Distance(0)` means that we leave zero blocks from the latest,
///    meaning we have one block in the database.
#[cfg(any(test, feature = "serde"))]
fn deserialize_opt_prune_mode_with_min_blocks<
    'de,
    const MIN_BLOCKS: u64,
    D: serde::Deserializer<'de>,
>(
    deserializer: D,
) -> Result<Option<PruneMode>, D::Error> {
    use alloc::format;
    use serde::Deserialize;
    let prune_mode = Option::<PruneMode>::deserialize(deserializer)?;

    match prune_mode {
        Some(PruneMode::Full) if MIN_BLOCKS > 0 => {
            Err(serde::de::Error::invalid_value(
                serde::de::Unexpected::Str("full"),
                // This message should have "expected" wording
                &format!("prune mode that leaves at least {MIN_BLOCKS} blocks in the database")
                    .as_str(),
            ))
        }
        Some(PruneMode::Distance(distance)) if distance < MIN_BLOCKS => {
            Err(serde::de::Error::invalid_value(
                serde::de::Unexpected::Unsigned(distance),
                // This message should have "expected" wording
                &format!("prune mode that leaves at least {MIN_BLOCKS} blocks in the database")
                    .as_str(),
            ))
        }
        _ => Ok(prune_mode),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use serde::Deserialize;

    #[test]
    fn test_deserialize_opt_prune_mode_with_min_blocks() {
        #[derive(Debug, Deserialize, PartialEq, Eq)]
        struct V(
            #[serde(deserialize_with = "deserialize_opt_prune_mode_with_min_blocks::<10, _>")]
            Option<PruneMode>,
        );

        assert!(serde_json::from_str::<V>(r#"{"distance": 10}"#).is_ok());
        assert_matches!(
            serde_json::from_str::<V>(r#"{"distance": 9}"#),
            Err(err) if err.to_string() == "invalid value: integer `9`, expected prune mode that leaves at least 10 blocks in the database"
        );

        assert_matches!(
            serde_json::from_str::<V>(r#""full""#),
            Err(err) if err.to_string() == "invalid value: string \"full\", expected prune mode that leaves at least 10 blocks in the database"
        );
    }
}
