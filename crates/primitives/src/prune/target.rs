use crate::{PruneMode, ReceiptsLogPruneConfig};
use serde::{Deserialize, Deserializer, Serialize};

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

/// Deserializes [`Option<PruneMode>`] and validates that the value is not less than the const
/// generic parameter `MIN_BLOCKS`. This parameter represents the number of blocks that needs to be
/// left in database after the pruning.
///
/// 1. For [PruneMode::Full], it fails if `MIN_BLOCKS > 0`.
/// 2. For [PruneMode::Distance(distance)], it fails if `distance < MIN_BLOCKS + 1`. `+ 1` is needed
/// because `PruneMode::Distance(0)` means that we leave zero blocks from the latest, meaning we
/// have one block in the database.
fn deserialize_opt_prune_mode_with_min_blocks<'de, const MIN_BLOCKS: u64, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<PruneMode>, D::Error> {
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
