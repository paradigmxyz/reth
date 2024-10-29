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
    PruneInterruptReason, PruneProgress, PrunerOutput, SegmentOutput, SegmentOutputCheckpoint,
};
pub use segment::{PrunePurpose, PruneSegment, PruneSegmentError};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
pub use target::{PruneModes, MINIMUM_PRUNING_DISTANCE};

use alloy_primitives::{Address, BlockNumber};

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
        let pruned_block = pruned_block.unwrap_or_default();

        for (address, mode) in &self.0 {
            // Getting `None`, means that there is nothing to prune yet, so we need it to include in
            // the BTreeMap (block = 0), otherwise it will be excluded.
            // Reminder that this BTreeMap works as an inclusion list that excludes (prunes) all
            // other receipts.
            //
            // Reminder, that we increment because the [`BlockNumber`] key of the new map should be
            // viewed as `PruneMode::Before(block)`
            let block = (pruned_block + 1).max(
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

        for mode in self.0.values() {
            if let PruneMode::Distance(_) = mode {
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
