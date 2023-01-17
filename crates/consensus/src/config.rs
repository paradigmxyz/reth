//! Reth block execution/validation configuration and constants
use reth_executor::{Config as ExecutorConfig, SpecUpgrades};
use reth_primitives::{BlockNumber, U256};

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

/// Initial base fee as defined in: https://eips.ethereum.org/EIPS/eip-1559
pub const EIP1559_INITIAL_BASE_FEE: u64 = 1_000_000_000;
/// Base fee max change denominator as defined in: https://eips.ethereum.org/EIPS/eip-1559
pub const EIP1559_BASE_FEE_MAX_CHANGE_DENOMINATOR: u64 = 8;
/// Elasticity multiplier as defined in: https://eips.ethereum.org/EIPS/eip-1559
pub const EIP1559_ELASTICITY_MULTIPLIER: u64 = 2;

/// Common configuration for consensus algorithms.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "camelCase"))]
pub struct Config {
    /// Blockchain identifier introduced in EIP-155: Simple replay attack protection.
    pub chain_id: u64,

    /// Homestead switch block.
    pub homestead_block: BlockNumber,

    /// TheDAO hard-fork switch block.
    pub dao_fork_block: BlockNumber,
    /// Whether the node supports or opposes the DAO hard-fork
    pub dao_fork_support: bool,

    /// EIP150 implements gas price changes.
    pub eip_150_block: BlockNumber,

    /// EIP155 hard-fork block (Spurious Dragon)
    pub eip_155_block: BlockNumber,
    /// EIP158 hard-fork block.
    pub eip_158_block: BlockNumber,
    /// Byzantium switch block.
    pub byzantium_block: BlockNumber,
    /// Constantinople switch block.
    pub constantinople_block: BlockNumber,
    /// Petersburg switch block.
    pub petersburg_block: BlockNumber,
    /// Istanbul switch block.
    pub istanbul_block: BlockNumber,
    /// EIP-2728 switch block.
    pub berlin_block: BlockNumber,
    /// EIP-1559 switch block.
    pub london_block: BlockNumber,
    /// The Merge/Paris hard-fork block number.
    pub paris_block: BlockNumber,
    /// Terminal total difficulty after the paris hard-fork to reach before The Merge is considered
    /// activated.
    #[cfg_attr(feature = "serde", serde(rename = "terminalTotalDifficulty"))]
    pub merge_terminal_total_difficulty: u128,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            chain_id: 1,
            homestead_block: 1150000,
            dao_fork_block: 1920000,
            dao_fork_support: true,
            eip_150_block: 2463000,
            eip_155_block: 2675000,
            eip_158_block: 2675000,
            byzantium_block: 4370000,
            constantinople_block: 7280000,
            petersburg_block: 7280000,
            istanbul_block: 9069000,
            berlin_block: 12244000,
            london_block: 12965000,
            paris_block: 15537394,
            merge_terminal_total_difficulty: 58750000000000000000000,
        }
    }
}

impl From<&Config> for ExecutorConfig {
    fn from(value: &Config) -> Self {
        Self {
            chain_id: U256::from(value.chain_id),
            spec_upgrades: SpecUpgrades {
                frontier: 0,
                homestead: value.homestead_block,
                dao_fork: value.dao_fork_block,
                tangerine_whistle: value.eip_150_block,
                spurious_dragon: value.eip_158_block,
                byzantium: value.byzantium_block,
                petersburg: value.petersburg_block,
                istanbul: value.istanbul_block,
                berlin: value.berlin_block,
                london: value.london_block,
                paris: value.paris_block,
                shanghai: u64::MAX, // TODO: change once known
            },
        }
    }
}
