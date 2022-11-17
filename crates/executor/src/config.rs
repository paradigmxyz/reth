//! Reth block execution/validation configuration and constants

use reth_primitives::U256;

/// Configuration for executor
#[derive(Debug, Clone)]
pub struct Config {
    /// Chain id.
    pub chain_id: U256,
}
