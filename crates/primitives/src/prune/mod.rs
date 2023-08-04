mod checkpoint;
mod mode;
mod part;
mod target;

pub use checkpoint::PruneCheckpoint;
pub use mode::PruneMode;
pub use part::{PrunePart, PrunePartError};
pub use target::PruneModes;

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
            let block = mode
                .prune_target_block(tip, 128, PrunePart::ContractLogs)?
                .map(|(block, _)| block)
                .unwrap_or_default();

            map.entry(block).or_insert_with(Vec::new).push(address)
        }
        Ok(map)
    }
}
