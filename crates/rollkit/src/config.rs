use serde::{Deserialize, Serialize};

/// Configuration for the Rollkit payload builder
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RollkitPayloadBuilderConfig {
    /// Maximum number of transactions to include in a payload
    pub max_transactions: usize,
    /// Minimum gas price for transactions
    pub min_gas_price: u64,
}

impl Default for RollkitPayloadBuilderConfig {
    fn default() -> Self {
        Self {
            max_transactions: 1000,
            min_gas_price: 1_000_000_000, // 1 Gwei
        }
    }
}

impl RollkitPayloadBuilderConfig {
    /// Creates a new instance of RollkitPayloadBuilderConfig
    pub fn new(
        max_transactions: usize,
        min_gas_price: u64,
    ) -> Self {
        Self {
            max_transactions,
            min_gas_price,
        }
    }

    /// Validates the configuration
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.max_transactions == 0 {
            return Err(ConfigError::InvalidMaxTransactions);
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
    /// Invalid maximum transactions value.
    InvalidMaxTransactions,
    #[error("Invalid min gas price value")]
    /// Invalid minimum gas price value.
    InvalidMinGasPrice,
} 