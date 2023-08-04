mod checkpoint;
mod mode;
mod part;
mod target;

pub use checkpoint::PruneCheckpoint;
pub use mode::PruneMode;
pub use part::{PrunePart, PrunePartError};
pub use target::{PruneModes, MINIMUM_PRUNING_DISTANCE};

use crate::{Address, BlockNumber};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Configuration for pruning receipts not associated with logs emitted by the specified contracts.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ContractLogsPruneConfig(pub Vec<(PruneMode, Address)>);

impl ContractLogsPruneConfig {
    /// Checks if the configuration is empty
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Given the `tip` block number, flatten the struct so it can easily be queried for filtering
    /// across a range of blocks.
    ///
    /// The [`BlockNumber`] key of the map should be viewed as `PruneMode::Before(block)`.
    pub fn flatten(
        &self,
        tip: BlockNumber,
    ) -> Result<BTreeMap<BlockNumber, Vec<&Address>>, PrunePartError> {
        let mut map = BTreeMap::new();
        for (mode, address) in self.0.iter() {
            // Getting `None`, means that there is nothing to prune yet, so we need it to include in
            // the BTreeMap (block = 0), otherwise it will be excluded.
            // Reminder that this BTreeMap works as an inclusion list that excludes (prunes) all
            // other receipts.
            let block = mode
                .prune_target_block(tip, MINIMUM_PRUNING_DISTANCE, PrunePart::ContractLogs)?
                .map(|(block, _)| block)
                .unwrap_or_default();

            map.entry(block).or_insert_with(Vec::new).push(address)
        }
        Ok(map)
    }
}
