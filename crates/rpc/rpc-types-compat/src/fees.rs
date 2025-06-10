use crate::error::RpcInvalidTransactionError;
use alloy_primitives::{B256, U256};
use std::cmp::min;
use thiserror::Error;

/// Helper type for representing the fees of a `TransactionRequest`
#[derive(Debug)]
pub struct CallFees {
    /// EIP-1559 priority fee
    pub max_priority_fee_per_gas: Option<U256>,
    /// Unified gas price setting
    ///
    /// Will be the configured `basefee` if unset in the request
    ///
    /// `gasPrice` for legacy,
    /// `maxFeePerGas` for EIP-1559
    pub gas_price: U256,
    /// Max Fee per Blob gas for EIP-4844 transactions
    pub max_fee_per_blob_gas: Option<U256>,
}

impl CallFees {
    /// Ensures the fields of a `TransactionRequest` are not conflicting.
    ///
    /// # EIP-4844 transactions
    ///
    /// Blob transactions have an additional fee parameter `maxFeePerBlobGas`.
    /// If the `maxFeePerBlobGas` or `blobVersionedHashes` are set we treat it as an EIP-4844
    /// transaction.
    ///
    /// Note: Due to the `Default` impl of [`BlockEnv`] (Some(0)) this assumes the `block_blob_fee`
    /// is always `Some`
    ///
    /// ## Notable design decisions
    ///
    /// For compatibility reasons, this contains several exceptions when fee values are validated:
    /// - If both `maxFeePerGas` and `maxPriorityFeePerGas` are set to `0` they are treated as
    ///   missing values, bypassing fee checks wrt. `baseFeePerGas`.
    ///
    /// This mirrors geth's behaviour when transaction requests are executed: <https://github.com/ethereum/go-ethereum/blob/380688c636a654becc8f114438c2a5d93d2db032/core/state_transition.go#L306-L306>
    ///
    /// [`BlockEnv`]: revm_context::BlockEnv
    pub fn ensure_fees(
        call_gas_price: Option<U256>,
        call_max_fee: Option<U256>,
        call_priority_fee: Option<U256>,
        block_base_fee: U256,
        blob_versioned_hashes: Option<&[B256]>,
        max_fee_per_blob_gas: Option<U256>,
        block_blob_fee: Option<U256>,
    ) -> Result<Self, CallFeesError> {
        /// Get the effective gas price of a transaction as specfified in EIP-1559 with relevant
        /// checks.
        fn get_effective_gas_price(
            max_fee_per_gas: Option<U256>,
            max_priority_fee_per_gas: Option<U256>,
            block_base_fee: U256,
        ) -> Result<U256, CallFeesError> {
            match max_fee_per_gas {
                Some(max_fee) => {
                    let max_priority_fee_per_gas = max_priority_fee_per_gas.unwrap_or(U256::ZERO);

                    // only enforce the fee cap if provided input is not zero
                    if !(max_fee.is_zero() && max_priority_fee_per_gas.is_zero()) &&
                        max_fee < block_base_fee
                    {
                        // `base_fee_per_gas` is greater than the `max_fee_per_gas`
                        return Err(RpcInvalidTransactionError::FeeCapTooLow.into())
                    }
                    if max_fee < max_priority_fee_per_gas {
                        return Err(
                            // `max_priority_fee_per_gas` is greater than the `max_fee_per_gas`
                            RpcInvalidTransactionError::TipAboveFeeCap.into(),
                        )
                    }
                    // ref <https://github.com/ethereum/go-ethereum/blob/0dd173a727dd2d2409b8e401b22e85d20c25b71f/internal/ethapi/transaction_args.go#L446-L446>
                    Ok(min(
                        max_fee,
                        block_base_fee.checked_add(max_priority_fee_per_gas).ok_or_else(|| {
                            CallFeesError::from(RpcInvalidTransactionError::TipVeryHigh)
                        })?,
                    ))
                }
                None => Ok(block_base_fee
                    .checked_add(max_priority_fee_per_gas.unwrap_or(U256::ZERO))
                    .ok_or(CallFeesError::from(RpcInvalidTransactionError::TipVeryHigh))?),
            }
        }

        let has_blob_hashes =
            blob_versioned_hashes.as_ref().map(|blobs| !blobs.is_empty()).unwrap_or(false);

        match (call_gas_price, call_max_fee, call_priority_fee, max_fee_per_blob_gas) {
            (gas_price, None, None, None) => {
                // either legacy transaction or no fee fields are specified
                // when no fields are specified, set gas price to zero
                let gas_price = gas_price.unwrap_or(U256::ZERO);
                Ok(Self {
                    gas_price,
                    max_priority_fee_per_gas: None,
                    max_fee_per_blob_gas: has_blob_hashes.then_some(block_blob_fee).flatten(),
                })
            }
            (None, max_fee_per_gas, max_priority_fee_per_gas, None) => {
                // request for eip-1559 transaction
                let effective_gas_price = get_effective_gas_price(
                    max_fee_per_gas,
                    max_priority_fee_per_gas,
                    block_base_fee,
                )?;
                let max_fee_per_blob_gas = has_blob_hashes.then_some(block_blob_fee).flatten();

                Ok(Self {
                    gas_price: effective_gas_price,
                    max_priority_fee_per_gas,
                    max_fee_per_blob_gas,
                })
            }
            (None, max_fee_per_gas, max_priority_fee_per_gas, Some(max_fee_per_blob_gas)) => {
                // request for eip-4844 transaction
                let effective_gas_price = get_effective_gas_price(
                    max_fee_per_gas,
                    max_priority_fee_per_gas,
                    block_base_fee,
                )?;
                // Ensure blob_hashes are present
                if !has_blob_hashes {
                    // Blob transaction but no blob hashes
                    return Err(RpcInvalidTransactionError::BlobTransactionMissingBlobHashes.into())
                }

                Ok(Self {
                    gas_price: effective_gas_price,
                    max_priority_fee_per_gas,
                    max_fee_per_blob_gas: Some(max_fee_per_blob_gas),
                })
            }
            _ => {
                // this fallback covers incompatible combinations of fields
                Err(CallFeesError::ConflictingFeeFieldsInRequest)
            }
        }
    }
}

/// Error coming from decoding and validating transaction request fees.
#[derive(Debug, Error)]
pub enum CallFeesError {
    /// Errors related to invalid transactions
    #[error(transparent)]
    InvalidTransaction(#[from] RpcInvalidTransactionError),
    /// Thrown when a call or transaction request (`eth_call`, `eth_estimateGas`,
    /// `eth_sendTransaction`) contains conflicting fields (legacy, EIP-1559)
    #[error("both gasPrice and (maxFeePerGas or maxPriorityFeePerGas) specified")]
    ConflictingFeeFieldsInRequest,
}
