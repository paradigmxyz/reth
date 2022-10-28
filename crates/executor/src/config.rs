use reth_primitives::BlockNumber;

// https://eips.ethereum.org/EIPS/eip-1559
pub const EIP1559_INITIAL_BASE_FEE: u64 = 1_000_000_000;
pub const EIP1559_BASE_FEE_MAX_CHANGE_DENOMINATOR: u64 = 8;
pub const EIP1559_ELASTICITY_MULTIPLIER: u64 = 2;

/// Configuration for executor (TODO)
#[derive(Debug, Clone)]
pub struct Config {
    /// Example
    pub example: bool,
    /// EIP-1559 hard for number
    pub london_hard_fork_block: BlockNumber,
}
