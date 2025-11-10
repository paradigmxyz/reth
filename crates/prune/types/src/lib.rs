//! Commonly used types for prune usage.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

mod checkpoint;
mod event;
mod mode;
mod pruner;
mod segment;
mod target;

use std::collections::BTreeMap;

use alloy_primitives::{Address, BlockNumber};
pub use checkpoint::PruneCheckpoint;
use derive_more::{Deref, From};
pub use event::PrunerEvent;
pub use mode::PruneMode;
pub use pruner::{
    PruneInterruptReason, PruneProgress, PrunedSegmentInfo, PrunerOutput, SegmentOutput,
    SegmentOutputCheckpoint,
};
pub use segment::{PrunePurpose, PruneSegment, PruneSegmentError};
use serde::Deserialize;
pub use target::{PruneModes, UnwindTargetPrunedError, MINIMUM_PRUNING_DISTANCE};

/// Configuration for pruning receipts not associated with logs emitted by the specified contracts.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deref, From)]
#[cfg_attr(any(test, feature = "serde"), derive(serde::Serialize, serde::Deserialize))]
pub struct ReceiptsLogPruneConfig(pub BTreeMap<Address, PruneMode>);

impl ReceiptsLogPruneConfig {
    /// Creates an empty config.
    pub const fn empty() -> Self {
        Self(BTreeMap::new())
    }

    /// Returns `true` if the config is empty.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub(crate) fn deserialize<'de, D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let map = BTreeMap::<Address, PruneMode>::deserialize(deserializer)?;
        if let Some(address) =
            map.iter().find_map(|(address, mode)| mode.is_distance().then_some(address))
        {
            return Err(serde::de::Error::custom(format!(
                "address {} has a distance-based pruning mode which is no longer supported. Please either use `full` or `before`.",
                address
            )));
        }
        Ok(Self(map))
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
    ) -> Result<BTreeMap<BlockNumber, Vec<Address>>, PruneSegmentError> {
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

            map.entry(block).or_insert_with(Vec::new).push(*address)
        }
        Ok(map)
    }
}
