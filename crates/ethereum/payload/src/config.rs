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
    pub const fn with_gas_limit(mut self, desired_gas_limit: u64) -> Self {
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
/// Ref: <https://github.com/ethereum/go-ethereum/blob/88cbfab332c96edfbe99d161d9df6a40721bd786/core/block_validator.go#L166>
pub fn calculate_block_gas_limit(parent_gas_limit: u64, desired_gas_limit: u64) -> u64 {
    let delta = (parent_gas_limit / GAS_LIMIT_BOUND_DIVISOR).saturating_sub(1);
    let min_gas_limit = parent_gas_limit - delta;
    let max_gas_limit = parent_gas_limit + delta;
    desired_gas_limit.clamp(min_gas_limit, max_gas_limit)
}
