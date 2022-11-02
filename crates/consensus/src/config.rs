//! Reth block execution/validation configuration and constants
use reth_primitives::BlockNumber;

/// Initial base fee as defined in: https://eips.ethereum.org/EIPS/eip-1559
pub const EIP1559_INITIAL_BASE_FEE: u64 = 1_000_000_000;
/// Base fee max change denominator as defined in: https://eips.ethereum.org/EIPS/eip-1559
pub const EIP1559_BASE_FEE_MAX_CHANGE_DENOMINATOR: u64 = 8;
/// Elasticity multiplier as defined in: https://eips.ethereum.org/EIPS/eip-1559
pub const EIP1559_ELASTICITY_MULTIPLIER: u64 = 2;

/// Configuration for consensus
#[derive(Debug, Clone)]
pub struct Config {
    /// EIP-1559 hard fork number
    pub london_hard_fork_block: BlockNumber,
    /// The Merge/Paris hard fork block number
    pub paris_hard_fork_block: BlockNumber,
}

impl Default for Config {
    fn default() -> Self {
        Self { london_hard_fork_block: 12965000, paris_hard_fork_block: 15537394 }
    }
}
