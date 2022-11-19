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
    /// Spurious dragon ethereum update block.
    pub spurious_dragon_hard_fork_block: BlockNumber,
    /// EIP-2728 hard fork number.
    pub berlin_hard_fork_block: BlockNumber,
    /// EIP-1559 hard fork number.
    pub london_hard_fork_block: BlockNumber,
    /// The Merge/Paris hard fork block number.
    pub paris_hard_fork_block: BlockNumber,
    /// Blockchain identifier introduced in EIP-155: Simple replay attack protection.
    pub chain_id: u64,
    /// Merge terminal total dificulty after the paris hardfork got activated.
    pub merge_terminal_total_difficulty: u128,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            spurious_dragon_hard_fork_block: 2675000,
            berlin_hard_fork_block: 12244000,
            london_hard_fork_block: 12965000,
            paris_hard_fork_block: 15537394,
            merge_terminal_total_difficulty: 58750000000000000000000,
            chain_id: 1,
        }
    }
}
