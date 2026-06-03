use alloy_consensus::constants::ETH_TO_WEI;
use alloy_primitives::{Address, BlockNumber};
use reth_ethereum_forks::EthereumHardforks;

/// Chain-specific execution spec values.
#[auto_impl::auto_impl(&, alloc::sync::Arc)]
pub trait EthExecutorSpec: EthereumHardforks {
    /// Returns the deposit contract address.
    fn deposit_contract_address(&self) -> Option<Address>;
}

/// EVM limit parameters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EvmLimitParams {
    /// Maximum contract code size.
    pub max_code_size: usize,
    /// Maximum initcode size.
    pub max_initcode_size: usize,
    /// Transaction gas limit cap.
    pub tx_gas_limit_cap: Option<u64>,
}

impl EvmLimitParams {
    /// Returns Osaka EVM limit parameters.
    pub const fn osaka() -> Self {
        Self {
            max_code_size: 0x6000,
            max_initcode_size: 0xC000,
            tx_gas_limit_cap: Some(16_777_216),
        }
    }
}

/// Returns the base block reward for the given block.
pub fn base_block_reward(spec: impl EthereumHardforks, block_number: BlockNumber) -> Option<u128> {
    if spec.is_paris_active_at_block(block_number) {
        None
    } else {
        Some(base_block_reward_pre_merge(spec, block_number))
    }
}

/// Returns the pre-merge base block reward for the given block.
pub fn base_block_reward_pre_merge(
    spec: impl EthereumHardforks,
    block_number: BlockNumber,
) -> u128 {
    if spec.is_constantinople_active_at_block(block_number) {
        ETH_TO_WEI * 2
    } else if spec.is_byzantium_active_at_block(block_number) {
        ETH_TO_WEI * 3
    } else {
        ETH_TO_WEI * 5
    }
}

/// Returns the block reward including ommer rewards.
pub const fn block_reward(base_block_reward: u128, ommers: usize) -> u128 {
    base_block_reward + (base_block_reward >> 5) * ommers as u128
}

/// Returns the ommer reward for the given block and ommer.
pub const fn ommer_reward(
    base_block_reward: u128,
    block_number: BlockNumber,
    ommer_block_number: BlockNumber,
) -> u128 {
    ((8 + ommer_block_number - block_number) as u128 * base_block_reward) >> 3
}
