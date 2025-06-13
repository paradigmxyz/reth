use reth_primitives::TransactionSigned;
use alloy_primitives::{Address, B256};
use serde::{Deserialize, Serialize};

/// Payload attributes for the Rollkit Reth node
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RollkitPayloadAttributes {
    /// List of transactions to be executed in the payload
    pub transactions: Vec<TransactionSigned>,
    /// Optional gas limit for the transactions
    pub gas_limit: Option<u64>,
    /// Timestamp for the block
    pub timestamp: u64,
    /// Prev randao value
    pub prev_randao: B256,
    /// Suggested fee recipient
    pub suggested_fee_recipient: Address,
}

impl RollkitPayloadAttributes {
    /// Creates a new instance of RollkitPayloadAttributes
    pub fn new(
        transactions: Vec<TransactionSigned>,
        gas_limit: Option<u64>,
        timestamp: u64,
        prev_randao: B256,
        suggested_fee_recipient: Address,
    ) -> Self {
        Self {
            transactions,
            gas_limit,
            timestamp,
            prev_randao,
            suggested_fee_recipient,
        }
    }

    /// Validates the payload attributes
    pub fn validate(&self) -> Result<(), PayloadAttributesError> {
        if self.transactions.is_empty() {
            return Err(PayloadAttributesError::EmptyTransactions);
        }

        if let Some(gas_limit) = self.gas_limit {
            if gas_limit == 0 {
                return Err(PayloadAttributesError::InvalidGasLimit);
            }
        }

        Ok(())
    }
}

/// Errors that can occur during payload attributes validation
#[derive(Debug, thiserror::Error)]
pub enum PayloadAttributesError {
    #[error("No transactions provided")]
    EmptyTransactions,
    #[error("Invalid gas limit")]
    InvalidGasLimit,
    #[error("Transaction validation failed: {0}")]
    TransactionValidation(String),
} 