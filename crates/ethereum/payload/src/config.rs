use alloy_eips::eip1559::ETHEREUM_BLOCK_GAS_LIMIT;
use alloy_primitives::Bytes;
use reth_primitives_traits::constants::GAS_LIMIT_BOUND_DIVISOR;

/// Settings for the Ethereum builder.
#[derive(PartialEq, Eq, Clone, Debug)]
pub struct EthereumBuilderConfig {
    /// Block extra data.
    pub extra_data: Bytes,
    /// Desired gas limit.
    pub desired_gas_limit: u64,
}

impl EthereumBuilderConfig {
    /// Create new payload builder config.
    pub const fn new(extra_data: Bytes) -> Self {
        Self { extra_data, desired_gas_limit: ETHEREUM_BLOCK_GAS_LIMIT }
    }

    /// Set desired gas limit.
    pub fn with_gas_limit(mut self, desired_gas_limit: u64) -> Self {
        self.desired_gas_limit = desired_gas_limit;
        self
    }
}

impl EthereumBuilderConfig {
    /// Returns owned extra data bytes for the block.
    pub fn extra_data(&self) -> Bytes {
        self.extra_data.clone()
    }

    /// Returns the gas limit for the next block based
    /// on parent and desired gas limits.
    pub fn gas_limit(&self, parent_gas_limit: u64) -> u64 {
        calculate_block_gas_limit(parent_gas_limit, self.desired_gas_limit)
    }
}

/// Calculate the gas limit for the next block based on parent and desired gas limits.
pub fn calculate_block_gas_limit(parent_gas_limit: u64, desired_gas_limit: u64) -> u64 {
    let delta = parent_gas_limit / GAS_LIMIT_BOUND_DIVISOR;
    let min_gas_limit = parent_gas_limit + delta - 1;
    let max_gas_limit = parent_gas_limit - delta + 1;
    desired_gas_limit.clamp(min_gas_limit, max_gas_limit)
}
