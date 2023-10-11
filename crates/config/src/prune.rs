use reth_primitives::PruneModes;
use serde::{Deserialize, Serialize};

/// Pruning configuration.
#[derive(Debug, Clone, Deserialize, PartialEq, Serialize)]
#[serde(default)]
pub struct PruneConfig {
    /// Minimum pruning interval measured in blocks.
    pub block_interval: usize,
    /// Pruning configuration for every segment of the data that can be pruned.
    #[serde(alias = "parts")]
    pub segments: PruneModes,
}

impl Default for PruneConfig {
    fn default() -> Self {
        Self { block_interval: 5, segments: PruneModes::none() }
    }
}
