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
    /// Parent block hash
    pub parent_hash: B256,
    /// Block number
    pub block_number: u64,
}

impl RollkitPayloadAttributes {
    /// Creates a new instance of RollkitPayloadAttributes
    pub fn new(
        transactions: Vec<TransactionSigned>,
        gas_limit: Option<u64>,
        timestamp: u64,
        prev_randao: B256,
        suggested_fee_recipient: Address,
        parent_hash: B256,
        block_number: u64,
    ) -> Self {
        Self {
            transactions,
            gas_limit,
            timestamp,
            prev_randao,
            suggested_fee_recipient,
            parent_hash,
            block_number,
        }
    }

    /// Validates the payload attributes
    pub fn validate(&self) -> Result<(), PayloadAttributesError> {
        // For rollkit, empty transactions are allowed (empty blocks are valid)
        
        if let Some(gas_limit) = self.gas_limit {
            if gas_limit == 0 {
                return Err(PayloadAttributesError::InvalidGasLimit);
            }
        }

        Ok(())
    }
}

/// Errors that can occur during payload attributes validation
/// 
/// This enum represents various validation errors that can occur when processing
/// payload attributes for the Rollkit payload builder. Each variant corresponds
/// to a specific validation failure scenario.
#[derive(Debug, thiserror::Error)]
pub enum PayloadAttributesError {
    /// Error when no transactions are provided in the payload attributes
    /// 
    /// This error occurs when the transaction list is empty, which is invalid
    /// since a payload must contain at least one transaction to be meaningful.
    #[error("No transactions provided")]
    EmptyTransactions,
    
    /// Error when an invalid gas limit is specified
    /// 
    /// This error occurs when the gas limit is set to zero or an otherwise
    /// invalid value that would prevent proper transaction execution.
    #[error("Invalid gas limit")]
    InvalidGasLimit,
    
    /// Error when transaction validation fails
    /// 
    /// This error occurs when individual transactions within the payload
    /// fail validation checks. The error message provides details about
    /// the specific validation failure.
    #[error("Transaction validation failed: {0}")]
    TransactionValidation(String),
} 