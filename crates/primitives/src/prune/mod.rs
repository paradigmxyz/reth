mod checkpoint;
mod mode;
mod segment;
mod target;

use crate::{Address, BlockNumber, StorageKey};
pub use checkpoint::PruneCheckpoint;
pub use mode::PruneMode;
pub use segment::{PruneSegment, PruneSegmentError};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
pub use target::{PruneModes, MINIMUM_PRUNING_DISTANCE};

/// Helper struct for [StorageHistoryPruneConfig].
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct StorageHistoryPruneAddressConfig {
    /// `PruneMode` for all the slots in the contract address. If `None` it means we have to look
    /// for specific `PruneMode` inside every slot.
    pub mode: Option<PruneMode>,
    /// Mapping between a slot and its `PruneMode`. If empty it means we just have to look at this
    /// struct's `mode` field.
    pub slots: BTreeMap<StorageKey, PruneMode>,
}

/// Configuration for pruning storage history not associated with specifies address.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct StorageHistoryPruneConfig(pub BTreeMap<Address, StorageHistoryPruneAddressConfig>);

impl StorageHistoryPruneConfig {
    /// Checks if the configuration is empty
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Given the `tip` block number, consolidates the structure so it can easily be queried for
    /// filtering across a range of blocks.
    ///
    /// Example:
    ///
    /// `{
    ///     addrA: {mode: Some(Before(872)), slots: {}}, addrB: {mode: Some(Before(500)), slots:
    ///     {}},     addrC, {mode: None, slots: {slot1: Distance(128), slot2: Distance(128), slot3:
    ///     Distance(100)}}
    ///  }`  
    ///    for `tip: 1000`, gets transformed to a map such as:
    ///
    /// `{ 500: {addrB: []}, 872: {addrA: []}, {addrC: [slot1, slot2]}, 900: {addrC: [slot3]} }`
    ///
    /// The [`BlockNumber`] key of the new map should be viewed as `PruneMode::Before(block)`, which
    /// makes the previous result equivalent to
    ///
    /// `{
    ///     Before(500): {addrB: []}, Before(872): {addrA: []}, {addrC: [slot1, slot2]},
    ///     Before(900): {addrC: [slot3]}
    /// }`
    pub fn group_by_block(
        &self,
        tip: BlockNumber,
        pruned_block: Option<BlockNumber>,
    ) -> Result<BTreeMap<BlockNumber, BTreeMap<Address, Vec<StorageKey>>>, PruneSegmentError> {
        let mut map = BTreeMap::new();
        let pruned_block = pruned_block.unwrap_or_default();

        for (address, storage_history_prune_address_config) in self.0.iter() {
            // Reminder that this BTreeMap works as an inclusion list that excludes (prunes) all
            // other storage changes.
            //
            // Reminder, that we increment because the [`BlockNumber`] key of the new map should be
            // viewed as `PruneMode::Before(block)`
            match storage_history_prune_address_config.mode {
                // If there is a `PruneMode` here, we can ignore the slots, meaning that we want to
                // save all the slots associeated with this contract address.
                Some(mode) => {
                    let block = (pruned_block + 1).max(
                        mode.prune_target_block(
                            tip,
                            PruneSegment::StorageHistoryFilteredByContractAndSlots,
                        )?
                        .map(|(block, _)| block)
                        .unwrap_or_default() +
                            1,
                    );
                    map.entry(block)
                        .or_insert_with(BTreeMap::default)
                        .entry(*address)
                        .or_insert_with(Vec::new);
                }
                // Otherwise we have to look for specific `PruneMode` in every slot associated with
                // this contract address.
                None => {
                    for (slot, mode) in storage_history_prune_address_config.slots.iter() {
                        let block = (pruned_block + 1).max(
                            mode.prune_target_block(
                                tip,
                                PruneSegment::StorageHistoryFilteredByContractAndSlots,
                            )?
                            .map(|(block, _)| block)
                            .unwrap_or_default() +
                                1,
                        );
                        map.entry(block)
                            .or_insert_with(BTreeMap::default)
                            .entry(*address)
                            .or_insert_with(Vec::new)
                            .push(*slot);
                    }
                }
            }
        }
        Ok(map)
    }

    /// Returns the lowest block where we start filtering storage changes which use
    /// `PruneMode::Distance(_)`.
    pub fn lowest_block_with_distance(
        &self,
        tip: BlockNumber,
        pruned_block: Option<BlockNumber>,
    ) -> Result<Option<BlockNumber>, PruneSegmentError> {
        let pruned_block = pruned_block.unwrap_or_default();
        let mut lowest = None;

        for (_, storage_history_prune_address_config) in self.0.iter() {
            match storage_history_prune_address_config.mode {
                Some(mode) => {
                    if let PruneMode::Distance(_) = mode {
                        if let Some((block, _)) = mode.prune_target_block(
                            tip,
                            PruneSegment::StorageHistoryFilteredByContractAndSlots,
                        )? {
                            lowest = Some(lowest.unwrap_or(u64::MAX).min(block));
                        }
                    }
                }
                None => {
                    for (_, mode) in storage_history_prune_address_config.slots.iter() {
                        if let PruneMode::Distance(_) = mode {
                            if let Some((block, _)) = mode.prune_target_block(
                                tip,
                                PruneSegment::StorageHistoryFilteredByContractAndSlots,
                            )? {
                                lowest = Some(lowest.unwrap_or(u64::MAX).min(block));
                            }
                        }
                    }
                }
            }
        }

        Ok(lowest.map(|lowest| lowest.max(pruned_block)))
    }
}

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

        for (address, mode) in self.0.iter() {
            // Getting `None`, means that there is nothing to prune yet, so we need it to include in
            // the BTreeMap (block = 0), otherwise it will be excluded.
            // Reminder that this BTreeMap works as an inclusion list that excludes (prunes) all
            // other receipts.
            //
            // Reminder, that we increment because the [`BlockNumber`] key of the new map should be
            // viewed as `PruneMode::Before(block)`
            let block = (pruned_block + 1).max(
                mode.prune_target_block(tip, PruneSegment::ContractLogs)?
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

        for (_, mode) in self.0.iter() {
            if let PruneMode::Distance(_) = mode {
                if let Some((block, _)) =
                    mode.prune_target_block(tip, PruneSegment::ContractLogs)?
                {
                    lowest = Some(lowest.unwrap_or(u64::MAX).min(block));
                }
            }
        }

        Ok(lowest.map(|lowest| lowest.max(pruned_block)))
    }
}

/// Progress of pruning.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum PruneProgress {
    /// There is more data to prune.
    HasMoreData,
    /// Pruning has been finished.
    Finished,
}

impl PruneProgress {
    /// Creates new [PruneProgress] from `done` boolean value.
    ///
    /// If `done == true`, returns [PruneProgress::Finished], otherwise [PruneProgress::HasMoreData]
    /// is returned.
    pub fn from_done(done: bool) -> Self {
        if done {
            Self::Finished
        } else {
            Self::HasMoreData
        }
    }
}
