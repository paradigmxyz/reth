//! Commonly used types for prune usage.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]

mod checkpoint;
mod limiter;
mod mode;
mod pruner;
mod segment;
mod target;

pub use checkpoint::PruneCheckpoint;
pub use limiter::PruneLimiter;
pub use mode::PruneMode;
pub use pruner::{
    PruneInterruptReason, PruneProgress, PrunedSegmentInfo, PrunerOutput, SegmentOutput,
    SegmentOutputCheckpoint,
};
pub use segment::{PrunePurpose, PruneSegment, PruneSegmentError};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
pub use target::{PruneModes, MINIMUM_PRUNING_DISTANCE};

use alloy_primitives::{Address, BlockNumber};
use std::ops::Deref;

/// Configuration for pruning receipts not associated with logs emitted by the specified contracts.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ReceiptsLogPruneConfig(pub BTreeMap<Address, PruneMode>);

impl ReceiptsLogPruneConfig {
    /// Checks if the configuration is empty
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Given the `tip` block number, consolidates the structure so it can easily be queried for
    /// filtering across a range of blocks.
    ///
    /// Example:
    ///
    /// `{ addrA: Before(872), addrB: Before(500), addrC: Distance(128) }`
    ///
    ///    for `tip: 1000`, gets transformed to a map such as:
    ///
    /// `{ 500: [addrB], 872: [addrA, addrC] }`
    ///
    /// The [`BlockNumber`] key of the new map should be viewed as `PruneMode::Before(block)`, which
    /// makes the previous result equivalent to
    ///
    /// `{ Before(500): [addrB], Before(872): [addrA, addrC] }`
    pub fn group_by_block(
        &self,
        tip: BlockNumber,
        pruned_block: Option<BlockNumber>,
    ) -> Result<BTreeMap<BlockNumber, Vec<&Address>>, PruneSegmentError> {
        let mut map = BTreeMap::new();
        let base_block = pruned_block.unwrap_or_default() + 1;

        for (address, mode) in &self.0 {
            // Getting `None`, means that there is nothing to prune yet, so we need it to include in
            // the BTreeMap (block = 0), otherwise it will be excluded.
            // Reminder that this BTreeMap works as an inclusion list that excludes (prunes) all
            // other receipts.
            //
            // Reminder, that we increment because the [`BlockNumber`] key of the new map should be
            // viewed as `PruneMode::Before(block)`
            let block = base_block.max(
                mode.prune_target_block(tip, PruneSegment::ContractLogs, PrunePurpose::User)?
                    .map(|(block, _)| block)
                    .unwrap_or_default() +
                    1,
            );

            map.entry(block).or_insert_with(Vec::new).push(address)
        }
        Ok(map)
    }

    /// Returns the lowest block where we start filtering logs which use `PruneMode::Distance(_)`.
    pub fn lowest_block_with_distance(
        &self,
        tip: BlockNumber,
        pruned_block: Option<BlockNumber>,
    ) -> Result<Option<BlockNumber>, PruneSegmentError> {
        let pruned_block = pruned_block.unwrap_or_default();
        let mut lowest = None;

        for mode in self.values() {
            if mode.is_distance() {
                if let Some((block, _)) =
                    mode.prune_target_block(tip, PruneSegment::ContractLogs, PrunePurpose::User)?
                {
                    lowest = Some(lowest.unwrap_or(u64::MAX).min(block));
                }
            }
        }

        Ok(lowest.map(|lowest| lowest.max(pruned_block)))
    }
}

impl Deref for ReceiptsLogPruneConfig {
    type Target = BTreeMap<Address, PruneMode>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_group_by_block_empty_config() {
        let config = ReceiptsLogPruneConfig(BTreeMap::new());
        let tip = 1000;
        let pruned_block = None;

        let result = config.group_by_block(tip, pruned_block).unwrap();
        assert!(result.is_empty(), "The result should be empty when the config is empty");
    }

    #[test]
    fn test_group_by_block_single_entry() {
        let mut config_map = BTreeMap::new();
        let address = Address::new([1; 20]);
        let prune_mode = PruneMode::Before(500);
        config_map.insert(address, prune_mode);

        let config = ReceiptsLogPruneConfig(config_map);
        // Big tip to have something to prune for the target block
        let tip = 3000000;
        let pruned_block = Some(400);

        let result = config.group_by_block(tip, pruned_block).unwrap();

        // Expect one entry with block 500 and the corresponding address
        assert_eq!(result.len(), 1);
        assert_eq!(result[&500], vec![&address], "Address should be grouped under block 500");

        // Tip smaller than the target block, so that we have nothing to prune for the block
        let tip = 300;
        let pruned_block = Some(400);

        let result = config.group_by_block(tip, pruned_block).unwrap();

        // Expect one entry with block 400 and the corresponding address
        assert_eq!(result.len(), 1);
        assert_eq!(result[&401], vec![&address], "Address should be grouped under block 400");
    }

    #[test]
    fn test_group_by_block_multiple_entries() {
        let mut config_map = BTreeMap::new();
        let address1 = Address::new([1; 20]);
        let address2 = Address::new([2; 20]);
        let prune_mode1 = PruneMode::Before(600);
        let prune_mode2 = PruneMode::Before(800);
        config_map.insert(address1, prune_mode1);
        config_map.insert(address2, prune_mode2);

        let config = ReceiptsLogPruneConfig(config_map);
        let tip = 900000;
        let pruned_block = Some(400);

        let result = config.group_by_block(tip, pruned_block).unwrap();

        // Expect two entries: one for block 600 and another for block 800
        assert_eq!(result.len(), 2);
        assert_eq!(result[&600], vec![&address1], "Address1 should be grouped under block 600");
        assert_eq!(result[&800], vec![&address2], "Address2 should be grouped under block 800");
    }

    #[test]
    fn test_group_by_block_with_distance_prune_mode() {
        let mut config_map = BTreeMap::new();
        let address = Address::new([1; 20]);
        let prune_mode = PruneMode::Distance(100000);
        config_map.insert(address, prune_mode);

        let config = ReceiptsLogPruneConfig(config_map);
        let tip = 100100;
        // Pruned block is smaller than the target block
        let pruned_block = Some(50);

        let result = config.group_by_block(tip, pruned_block).unwrap();

        // Expect the entry to be grouped under block 100 (tip - distance)
        assert_eq!(result.len(), 1);
        assert_eq!(result[&101], vec![&address], "Address should be grouped under block 100");

        let tip = 100100;
        // Pruned block is larger than the target block
        let pruned_block = Some(800);

        let result = config.group_by_block(tip, pruned_block).unwrap();

        // Expect the entry to be grouped under block 800 which is larger than tip - distance
        assert_eq!(result.len(), 1);
        assert_eq!(result[&801], vec![&address], "Address should be grouped under block 800");
    }

    #[test]
    fn test_lowest_block_with_distance_empty_config() {
        let config = ReceiptsLogPruneConfig(BTreeMap::new());
        let tip = 1000;
        let pruned_block = None;

        let result = config.lowest_block_with_distance(tip, pruned_block).unwrap();
        assert_eq!(result, None, "The result should be None when the config is empty");
    }

    #[test]
    fn test_lowest_block_with_distance_no_distance_mode() {
        let mut config_map = BTreeMap::new();
        let address = Address::new([1; 20]);
        let prune_mode = PruneMode::Before(500);
        config_map.insert(address, prune_mode);

        let config = ReceiptsLogPruneConfig(config_map);
        let tip = 1000;
        let pruned_block = None;

        let result = config.lowest_block_with_distance(tip, pruned_block).unwrap();
        assert_eq!(result, None, "The result should be None when there are no Distance modes");
    }

    #[test]
    fn test_lowest_block_with_distance_single_entry() {
        let mut config_map = BTreeMap::new();
        let address = Address::new([1; 20]);
        let prune_mode = PruneMode::Distance(100000);
        config_map.insert(address, prune_mode);

        let config = ReceiptsLogPruneConfig(config_map);

        let tip = 100100;
        let pruned_block = Some(400);

        // Expect the lowest block to be 400 as 400 > 100100 - 100000 (tip - distance)
        assert_eq!(
            config.lowest_block_with_distance(tip, pruned_block).unwrap(),
            Some(400),
            "The lowest block should be 400"
        );

        let tip = 100100;
        let pruned_block = Some(50);

        // Expect the lowest block to be 100 as 100 > 50 (pruned block)
        assert_eq!(
            config.lowest_block_with_distance(tip, pruned_block).unwrap(),
            Some(100),
            "The lowest block should be 100"
        );
    }

    #[test]
    fn test_lowest_block_with_distance_multiple_entries_last() {
        let mut config_map = BTreeMap::new();
        let address1 = Address::new([1; 20]);
        let address2 = Address::new([2; 20]);
        let prune_mode1 = PruneMode::Distance(100100);
        let prune_mode2 = PruneMode::Distance(100300);
        config_map.insert(address1, prune_mode1);
        config_map.insert(address2, prune_mode2);

        let config = ReceiptsLogPruneConfig(config_map);
        let tip = 200300;
        let pruned_block = Some(100);

        // The lowest block should be 200300 - 100300 = 100000:
        // - First iteration will return 100200 => 200300 - 100100 = 100200
        // - Second iteration will return 100000 => 200300 - 100300 = 100000 < 100200
        // - Final result is 100000
        assert_eq!(config.lowest_block_with_distance(tip, pruned_block).unwrap(), Some(100000));
    }

    #[test]
    fn test_lowest_block_with_distance_multiple_entries_first() {
        let mut config_map = BTreeMap::new();
        let address1 = Address::new([1; 20]);
        let address2 = Address::new([2; 20]);
        let prune_mode1 = PruneMode::Distance(100400);
        let prune_mode2 = PruneMode::Distance(100300);
        config_map.insert(address1, prune_mode1);
        config_map.insert(address2, prune_mode2);

        let config = ReceiptsLogPruneConfig(config_map);
        let tip = 200300;
        let pruned_block = Some(100);

        // The lowest block should be 200300 - 100400 = 99900:
        // - First iteration, lowest block is 200300 - 100400 = 99900
        // - Second iteration, lowest block is still 99900 < 200300 - 100300 = 100000
        // - Final result is 99900
        assert_eq!(config.lowest_block_with_distance(tip, pruned_block).unwrap(), Some(99900));
    }

    #[test]
    fn test_lowest_block_with_distance_multiple_entries_pruned_block() {
        let mut config_map = BTreeMap::new();
        let address1 = Address::new([1; 20]);
        let address2 = Address::new([2; 20]);
        let prune_mode1 = PruneMode::Distance(100400);
        let prune_mode2 = PruneMode::Distance(100300);
        config_map.insert(address1, prune_mode1);
        config_map.insert(address2, prune_mode2);

        let config = ReceiptsLogPruneConfig(config_map);
        let tip = 200300;
        let pruned_block = Some(100000);

        // The lowest block should be 100000 because:
        // - Lowest is 200300 - 100400 = 99900 < 200300 - 100300 = 100000
        // - Lowest is compared to the pruned block 100000: 100000 > 99900
        // - Finally the lowest block is 100000
        assert_eq!(config.lowest_block_with_distance(tip, pruned_block).unwrap(), Some(100000));
    }
}
