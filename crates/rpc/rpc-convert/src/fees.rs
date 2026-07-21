use alloy_primitives::{B256, U256};
use core::cmp::min;

/// Helper type for representing the fees of a transaction request.
#[derive(Debug)]
pub struct CallFees {
    /// EIP-1559 priority fee.
    pub max_priority_fee_per_gas: Option<U256>,
    /// Effective gas price used by the call.
    pub gas_price: U256,
    /// Maximum fee per blob gas for EIP-4844 transactions.
    pub max_fee_per_blob_gas: Option<U256>,
}

impl CallFees {
    /// Ensures transaction request fee fields do not conflict and resolves their effective values.
    pub fn ensure_fees(
        call_gas_price: Option<U256>,
        call_max_fee: Option<U256>,
        call_priority_fee: Option<U256>,
        block_base_fee: U256,
        blob_versioned_hashes: Option<&[B256]>,
        max_fee_per_blob_gas: Option<U256>,
        block_blob_fee: Option<U256>,
    ) -> Result<Self, CallFeesError> {
        fn effective_gas_price(
            max_fee_per_gas: Option<U256>,
            max_priority_fee_per_gas: Option<U256>,
            block_base_fee: U256,
        ) -> Result<U256, CallFeesError> {
            match max_fee_per_gas {
                Some(max_fee) => {
                    let priority_fee = max_priority_fee_per_gas.unwrap_or(U256::ZERO);
                    if !(max_fee.is_zero() && priority_fee.is_zero()) && max_fee < block_base_fee {
                        return Err(CallFeesError::FeeCapTooLow)
                    }
                    if max_fee < priority_fee {
                        return Err(CallFeesError::TipAboveFeeCap)
                    }
                    Ok(min(
                        max_fee,
                        block_base_fee
                            .checked_add(priority_fee)
                            .ok_or(CallFeesError::TipVeryHigh)?,
                    ))
                }
                None => block_base_fee
                    .checked_add(max_priority_fee_per_gas.unwrap_or(U256::ZERO))
                    .ok_or(CallFeesError::TipVeryHigh),
            }
        }

        let has_blob_hashes = blob_versioned_hashes.is_some_and(|hashes| !hashes.is_empty());
        match (call_gas_price, call_max_fee, call_priority_fee, max_fee_per_blob_gas) {
            (gas_price, None, None, None) => Ok(Self {
                gas_price: gas_price.unwrap_or(U256::ZERO),
                max_priority_fee_per_gas: None,
                max_fee_per_blob_gas: has_blob_hashes.then_some(block_blob_fee).flatten(),
            }),
            (None, max_fee_per_gas, max_priority_fee_per_gas, None) => Ok(Self {
                gas_price: effective_gas_price(
                    max_fee_per_gas,
                    max_priority_fee_per_gas,
                    block_base_fee,
                )?,
                max_priority_fee_per_gas,
                max_fee_per_blob_gas: has_blob_hashes.then_some(block_blob_fee).flatten(),
            }),
            (None, max_fee_per_gas, max_priority_fee_per_gas, Some(max_fee_per_blob_gas)) => {
                if !has_blob_hashes {
                    return Err(CallFeesError::BlobTransactionMissingBlobHashes)
                }
                Ok(Self {
                    gas_price: effective_gas_price(
                        max_fee_per_gas,
                        max_priority_fee_per_gas,
                        block_base_fee,
                    )?,
                    max_priority_fee_per_gas,
                    max_fee_per_blob_gas: Some(max_fee_per_blob_gas),
                })
            }
            _ => Err(CallFeesError::ConflictingFeeFieldsInRequest),
        }
    }
}

/// Error from validating transaction request fee fields.
#[derive(Debug, thiserror::Error)]
pub enum CallFeesError {
    /// Legacy and EIP-1559 fee fields were both specified.
    #[error("both gasPrice and (maxFeePerGas or maxPriorityFeePerGas) specified")]
    ConflictingFeeFieldsInRequest,
    /// The fee cap is below the block base fee.
    #[error("max fee per gas less than block base fee")]
    FeeCapTooLow,
    /// The priority fee is above the total fee cap.
    #[error("max priority fee per gas higher than max fee per gas")]
    TipAboveFeeCap,
    /// The priority fee calculation overflowed.
    #[error("max priority fee per gas higher than 2^256-1")]
    TipVeryHigh,
    /// A blob transaction has no versioned hashes.
    #[error("blob transaction missing blob hashes")]
    BlobTransactionMissingBlobHashes,
}
