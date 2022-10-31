//! Reth block execution/validation configuration and constants

use reth_primitives::BlockNumber;
/// Configuration for executor
#[derive(Debug, Clone)]
pub struct Config {
    /// EIP-1559 hard fork number
    pub london_hard_fork_block: BlockNumber,
    /// The Merge/Paris hard fork block number
    pub paris_hard_fork_block: BlockNumber,
}

impl Default for Config {
    fn default() -> Self {
        Self { london_hard_fork_block: 1, paris_hard_fork_block: 1 }
    }
}
