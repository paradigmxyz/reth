use serde::{Deserialize, Serialize};

/// Configuration for the Rollkit payload builder
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RollkitPayloadBuilderConfig {
    /// Maximum number of transactions to include in a payload
    pub max_transactions: usize,
    /// Maximum gas limit for a payload
    pub max_gas_limit: u64,
    /// Minimum gas price for transactions
    pub min_gas_price: u64,
    /// Whether to enable transaction validation
    pub enable_tx_validation: bool,
}

impl Default for RollkitPayloadBuilderConfig {
    fn default() -> Self {
        Self {
            max_transactions: 1000,
            max_gas_limit: 30_000_000, // 30M gas
            min_gas_price: 1_000_000_000, // 1 Gwei
            enable_tx_validation: true,
        }
    }
}

impl RollkitPayloadBuilderConfig {
    /// Creates a new instance of RollkitPayloadBuilderConfig
    pub fn new(
        max_transactions: usize,
        max_gas_limit: u64,
        min_gas_price: u64,
        enable_tx_validation: bool,
    ) -> Self {
        Self {
            max_transactions,
            max_gas_limit,
            min_gas_price,
            enable_tx_validation,
        }
    }

    /// Validates the configuration
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.max_transactions == 0 {
            return Err(ConfigError::InvalidMaxTransactions);
        }

        if self.max_gas_limit == 0 {
            return Err(ConfigError::InvalidMaxGasLimit);
        }

        if self.min_gas_price == 0 {
            return Err(ConfigError::InvalidMinGasPrice);
        }

        Ok(())
    }
}

/// Errors that can occur during configuration validation
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("Invalid max transactions value")]
    InvalidMaxTransactions,
    #[error("Invalid max gas limit value")]
    InvalidMaxGasLimit,
    #[error("Invalid min gas price value")]
    InvalidMinGasPrice,
} 