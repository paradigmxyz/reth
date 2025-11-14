use alloy_primitives::BlockNumber;
use derive_more::Display;
use thiserror::Error;

use crate::{PruneCheckpoint, PruneMode, PruneSegment, ReceiptsLogPruneConfig};

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

/// Default pruning mode for merkle changesets
const fn default_merkle_changesets_mode() -> PruneMode {
    PruneMode::Distance(MINIMUM_PRUNING_DISTANCE)
}

/// Pruning configuration for every segment of the data that can be pruned.
#[derive(Debug, Clone, Eq, PartialEq)]
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
    /// Merkle Changesets pruning configuration for `AccountsTrieChangeSets` and
    /// `StoragesTrieChangeSets`.
    #[cfg_attr(
        any(test, feature = "serde"),
        serde(
            default = "default_merkle_changesets_mode",
            deserialize_with = "deserialize_prune_mode_with_min_blocks::<MINIMUM_PRUNING_DISTANCE, _>"
        )
    )]
    pub merkle_changesets: PruneMode,
    /// Receipts pruning configuration by retaining only those receipts that contain logs emitted
    /// by the specified addresses, discarding others. This setting is overridden by `receipts`.
    ///
    /// The [`BlockNumber`](`crate::BlockNumber`) represents the starting block from which point
    /// onwards the receipts are preserved.
    #[cfg_attr(
        any(test, feature = "serde"),
        serde(skip_serializing_if = "ReceiptsLogPruneConfig::is_empty")
    )]
    pub receipts_log_filter: ReceiptsLogPruneConfig,
}

impl Default for PruneModes {
    fn default() -> Self {
        Self {
            sender_recovery: None,
            transaction_lookup: None,
            receipts: None,
            account_history: None,
            storage_history: None,
            bodies_history: None,
            merkle_changesets: default_merkle_changesets_mode(),
            receipts_log_filter: ReceiptsLogPruneConfig::default(),
        }
    }
}

impl PruneModes {
    /// Sets pruning to all targets.
    pub fn all() -> Self {
        Self {
            sender_recovery: Some(PruneMode::Full),
            transaction_lookup: Some(PruneMode::Full),
            receipts: Some(PruneMode::Full),
            account_history: Some(PruneMode::Full),
            storage_history: Some(PruneMode::Full),
            bodies_history: Some(PruneMode::Full),
            merkle_changesets: PruneMode::Full,
            receipts_log_filter: Default::default(),
        }
    }

    /// Returns whether there is any kind of receipt pruning configuration.
    pub fn has_receipts_pruning(&self) -> bool {
        self.receipts.is_some() || !self.receipts_log_filter.is_empty()
    }

    /// Returns an error if we can't unwind to the targeted block because the target block is
    /// outside the range.
    ///
    /// This is only relevant for certain tables that are required by other stages
    ///
    /// See also <https://github.com/paradigmxyz/reth/issues/16579>
    pub fn ensure_unwind_target_unpruned(
        &self,
        latest_block: u64,
        target_block: u64,
        checkpoints: &[(PruneSegment, PruneCheckpoint)],
    ) -> Result<(), UnwindTargetPrunedError> {
        let distance = latest_block.saturating_sub(target_block);
        for (prune_mode, history_type, checkpoint) in &[
            (
                self.account_history,
                HistoryType::AccountHistory,
                checkpoints.iter().find(|(segment, _)| segment.is_account_history()),
            ),
            (
                self.storage_history,
                HistoryType::StorageHistory,
                checkpoints.iter().find(|(segment, _)| segment.is_storage_history()),
            ),
        ] {
            if let Some(PruneMode::Distance(limit)) = prune_mode {
                // check if distance exceeds the configured limit
                if distance > *limit {
                    // but only if have haven't pruned the target yet, if we dont have a checkpoint
                    // yet, it's fully unpruned yet
                    let pruned_height = checkpoint
                        .and_then(|checkpoint| checkpoint.1.block_number)
                        .unwrap_or(latest_block);
                    if pruned_height >= target_block {
                        // we've pruned the target block already and can't unwind past it
                        return Err(UnwindTargetPrunedError::TargetBeyondHistoryLimit {
                            latest_block,
                            target_block,
                            history_type: history_type.clone(),
                            limit: *limit,
                        })
                    }
                }
            }
        }
        Ok(())
    }
}

/// Deserializes [`PruneMode`] and validates that the value is not less than the const
/// generic parameter `MIN_BLOCKS`. This parameter represents the number of blocks that needs to be
/// left in database after the pruning.
///
/// 1. For [`PruneMode::Full`], it fails if `MIN_BLOCKS > 0`.
/// 2. For [`PruneMode::Distance`], it fails if `distance < MIN_BLOCKS + 1`. `+ 1` is needed because
///    `PruneMode::Distance(0)` means that we leave zero blocks from the latest, meaning we have one
///    block in the database.
#[cfg(any(test, feature = "serde"))]
fn deserialize_prune_mode_with_min_blocks<
    'de,
    const MIN_BLOCKS: u64,
    D: serde::Deserializer<'de>,
>(
    deserializer: D,
) -> Result<PruneMode, D::Error> {
    use serde::Deserialize;
    let prune_mode = PruneMode::deserialize(deserializer)?;
    serde_deserialize_validate::<MIN_BLOCKS, D>(&prune_mode)?;
    Ok(prune_mode)
}

/// Deserializes [`Option<PruneMode>`] and validates that the value is not less than the const
/// generic parameter `MIN_BLOCKS`. This parameter represents the number of blocks that needs to be
/// left in database after the pruning.
///
/// 1. For [`PruneMode::Full`], it fails if `MIN_BLOCKS > 0`.
/// 2. For [`PruneMode::Distance`], it fails if `distance < MIN_BLOCKS + 1`. `+ 1` is needed because
///    `PruneMode::Distance(0)` means that we leave zero blocks from the latest, meaning we have one
///    block in the database.
#[cfg(any(test, feature = "serde"))]
fn deserialize_opt_prune_mode_with_min_blocks<
    'de,
    const MIN_BLOCKS: u64,
    D: serde::Deserializer<'de>,
>(
    deserializer: D,
) -> Result<Option<PruneMode>, D::Error> {
    use serde::Deserialize;
    let prune_mode = Option::<PruneMode>::deserialize(deserializer)?;
    if let Some(prune_mode) = prune_mode.as_ref() {
        serde_deserialize_validate::<MIN_BLOCKS, D>(prune_mode)?;
    }
    Ok(prune_mode)
}

#[cfg(any(test, feature = "serde"))]
fn serde_deserialize_validate<'a, 'de, const MIN_BLOCKS: u64, D: serde::Deserializer<'de>>(
    prune_mode: &'a PruneMode,
) -> Result<(), D::Error> {
    use alloc::format;
    match prune_mode {
        PruneMode::Full if MIN_BLOCKS > 0 => {
            Err(serde::de::Error::invalid_value(
                serde::de::Unexpected::Str("full"),
                // This message should have "expected" wording
                &format!("prune mode that leaves at least {MIN_BLOCKS} blocks in the database")
                    .as_str(),
            ))
        }
        PruneMode::Distance(distance) if *distance < MIN_BLOCKS => {
            Err(serde::de::Error::invalid_value(
                serde::de::Unexpected::Unsigned(*distance),
                // This message should have "expected" wording
                &format!("prune mode that leaves at least {MIN_BLOCKS} blocks in the database")
                    .as_str(),
            ))
        }
        _ => Ok(()),
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

    #[test]
    fn test_unwind_target_unpruned() {
        // Test case 1: No pruning configured - should always succeed
        let prune_modes = PruneModes::default();
        assert!(prune_modes.ensure_unwind_target_unpruned(1000, 500, &[]).is_ok());
        assert!(prune_modes.ensure_unwind_target_unpruned(1000, 0, &[]).is_ok());

        // Test case 2: Distance pruning within limit - should succeed
        let prune_modes = PruneModes {
            account_history: Some(PruneMode::Distance(100)),
            storage_history: Some(PruneMode::Distance(100)),
            ..Default::default()
        };
        // Distance is 50, limit is 100 - OK
        assert!(prune_modes.ensure_unwind_target_unpruned(1000, 950, &[]).is_ok());

        // Test case 3: Distance exceeds limit with no checkpoint
        // NOTE: Current implementation assumes pruned_height = latest_block when no checkpoint
        // exists This means it will fail because it assumes we've pruned up to block 1000 >
        // target 800
        let prune_modes =
            PruneModes { account_history: Some(PruneMode::Distance(100)), ..Default::default() };
        // Distance is 200 > 100, no checkpoint - current impl treats as pruned up to latest_block
        let result = prune_modes.ensure_unwind_target_unpruned(1000, 800, &[]);
        assert_matches!(
            result,
            Err(UnwindTargetPrunedError::TargetBeyondHistoryLimit {
                latest_block: 1000,
                target_block: 800,
                history_type: HistoryType::AccountHistory,
                limit: 100
            })
        );

        // Test case 4: Distance exceeds limit and target is pruned - should fail
        let prune_modes =
            PruneModes { account_history: Some(PruneMode::Distance(100)), ..Default::default() };
        let checkpoints = vec![(
            PruneSegment::AccountHistory,
            PruneCheckpoint {
                block_number: Some(850),
                tx_number: None,
                prune_mode: PruneMode::Distance(100),
            },
        )];
        // Distance is 200 > 100, and checkpoint shows we've pruned up to block 850 > target 800
        let result = prune_modes.ensure_unwind_target_unpruned(1000, 800, &checkpoints);
        assert_matches!(
            result,
            Err(UnwindTargetPrunedError::TargetBeyondHistoryLimit {
                latest_block: 1000,
                target_block: 800,
                history_type: HistoryType::AccountHistory,
                limit: 100
            })
        );

        // Test case 5: Storage history exceeds limit and is pruned - should fail
        let prune_modes =
            PruneModes { storage_history: Some(PruneMode::Distance(50)), ..Default::default() };
        let checkpoints = vec![(
            PruneSegment::StorageHistory,
            PruneCheckpoint {
                block_number: Some(960),
                tx_number: None,
                prune_mode: PruneMode::Distance(50),
            },
        )];
        // Distance is 100 > 50, and checkpoint shows we've pruned up to block 960 > target 900
        let result = prune_modes.ensure_unwind_target_unpruned(1000, 900, &checkpoints);
        assert_matches!(
            result,
            Err(UnwindTargetPrunedError::TargetBeyondHistoryLimit {
                latest_block: 1000,
                target_block: 900,
                history_type: HistoryType::StorageHistory,
                limit: 50
            })
        );

        // Test case 6: Distance exceeds limit but target block not pruned yet - should succeed
        let prune_modes =
            PruneModes { account_history: Some(PruneMode::Distance(100)), ..Default::default() };
        let checkpoints = vec![(
            PruneSegment::AccountHistory,
            PruneCheckpoint {
                block_number: Some(700),
                tx_number: None,
                prune_mode: PruneMode::Distance(100),
            },
        )];
        // Distance is 200 > 100, but checkpoint shows we've only pruned up to block 700 < target
        // 800
        assert!(prune_modes.ensure_unwind_target_unpruned(1000, 800, &checkpoints).is_ok());

        // Test case 7: Both account and storage history configured, only one fails
        let prune_modes = PruneModes {
            account_history: Some(PruneMode::Distance(200)),
            storage_history: Some(PruneMode::Distance(50)),
            ..Default::default()
        };
        let checkpoints = vec![
            (
                PruneSegment::AccountHistory,
                PruneCheckpoint {
                    block_number: Some(700),
                    tx_number: None,
                    prune_mode: PruneMode::Distance(200),
                },
            ),
            (
                PruneSegment::StorageHistory,
                PruneCheckpoint {
                    block_number: Some(960),
                    tx_number: None,
                    prune_mode: PruneMode::Distance(50),
                },
            ),
        ];
        // For target 900: account history OK (distance 100 < 200), storage history fails (distance
        // 100 > 50, pruned at 960)
        let result = prune_modes.ensure_unwind_target_unpruned(1000, 900, &checkpoints);
        assert_matches!(
            result,
            Err(UnwindTargetPrunedError::TargetBeyondHistoryLimit {
                latest_block: 1000,
                target_block: 900,
                history_type: HistoryType::StorageHistory,
                limit: 50
            })
        );

        // Test case 8: Edge case - exact boundary
        let prune_modes =
            PruneModes { account_history: Some(PruneMode::Distance(100)), ..Default::default() };
        let checkpoints = vec![(
            PruneSegment::AccountHistory,
            PruneCheckpoint {
                block_number: Some(900),
                tx_number: None,
                prune_mode: PruneMode::Distance(100),
            },
        )];
        // Distance is exactly 100, checkpoint at exactly the target block
        assert!(prune_modes.ensure_unwind_target_unpruned(1000, 900, &checkpoints).is_ok());

        // Test case 9: Full pruning mode - should succeed (no distance check)
        let prune_modes = PruneModes {
            account_history: Some(PruneMode::Full),
            storage_history: Some(PruneMode::Full),
            ..Default::default()
        };
        assert!(prune_modes.ensure_unwind_target_unpruned(1000, 0, &[]).is_ok());

        // Test case 10: Edge case - saturating subtraction (target > latest)
        let prune_modes =
            PruneModes { account_history: Some(PruneMode::Distance(100)), ..Default::default() };
        // Target block (1500) > latest block (1000) - distance should be 0
        assert!(prune_modes.ensure_unwind_target_unpruned(1000, 1500, &[]).is_ok());
    }
}
