use alloy_primitives::{B256, U256};

/// Helper type for representing the fees of a transaction request.
#[derive(Debug)]
pub struct CallFees {
    /// EIP-1559 priority fee, `None` for a flat `gasPrice`.
    pub max_priority_fee_per_gas: Option<U256>,
    /// `gasPrice` for flat pricing, or the EIP-1559 fee cap, which the EVM combines with the
    /// priority fee into the effective gas price and uses to check that the sender can fund the
    /// call.
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
        fn dynamic_fees(
            max_fee_per_gas: Option<U256>,
            max_priority_fee_per_gas: Option<U256>,
            block_base_fee: U256,
        ) -> Result<(U256, U256), CallFeesError> {
            // An omitted fee cap defaults to zero, as in geth's `CallDefaults`.
            let max_fee = max_fee_per_gas.unwrap_or(U256::ZERO);
            let priority_fee = max_priority_fee_per_gas.unwrap_or(U256::ZERO);
            if max_fee < priority_fee {
                return Err(CallFeesError::TipAboveFeeCap)
            }
            if !(max_fee.is_zero() && priority_fee.is_zero()) && max_fee < block_base_fee {
                return Err(CallFeesError::FeeCapTooLow)
            }
            // The effective gas price `min(max_fee, base_fee + priority_fee)` must not overflow.
            block_base_fee.checked_add(priority_fee).ok_or(CallFeesError::TipVeryHigh)?;
            Ok((max_fee, priority_fee))
        }

        let has_blob_hashes = blob_versioned_hashes.is_some_and(|hashes| !hashes.is_empty());
        match (call_gas_price, call_max_fee, call_priority_fee, max_fee_per_blob_gas) {
            (gas_price, None, None, None) => Ok(Self {
                gas_price: gas_price.unwrap_or(U256::ZERO),
                max_priority_fee_per_gas: None,
                max_fee_per_blob_gas: has_blob_hashes.then_some(block_blob_fee).flatten(),
            }),
            (None, max_fee_per_gas, max_priority_fee_per_gas, None) => {
                let (max_fee_per_gas, max_priority_fee_per_gas) =
                    dynamic_fees(max_fee_per_gas, max_priority_fee_per_gas, block_base_fee)?;
                Ok(Self {
                    gas_price: max_fee_per_gas,
                    max_priority_fee_per_gas: Some(max_priority_fee_per_gas),
                    max_fee_per_blob_gas: has_blob_hashes.then_some(block_blob_fee).flatten(),
                })
            }
            (None, max_fee_per_gas, max_priority_fee_per_gas, Some(max_fee_per_blob_gas)) => {
                if !has_blob_hashes {
                    return Err(CallFeesError::BlobTransactionMissingBlobHashes)
                }
                let (max_fee_per_gas, max_priority_fee_per_gas) =
                    dynamic_fees(max_fee_per_gas, max_priority_fee_per_gas, block_base_fee)?;
                Ok(Self {
                    gas_price: max_fee_per_gas,
                    max_priority_fee_per_gas: Some(max_priority_fee_per_gas),
                    max_fee_per_blob_gas: Some(max_fee_per_blob_gas),
                })
            }
            (Some(gas_price), None, None, Some(max_fee_per_blob_gas)) => {
                if !has_blob_hashes {
                    return Err(CallFeesError::BlobTransactionMissingBlobHashes)
                }
                Ok(Self {
                    gas_price,
                    max_priority_fee_per_gas: None,
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

#[cfg(test)]
mod tests {
    use super::*;

    const GWEI: u64 = 1_000_000_000;

    #[test]
    fn omitted_fee_cap_defaults_to_zero() {
        let base_fee = U256::from(15 * GWEI);

        let CallFees { gas_price, .. } =
            CallFees::ensure_fees(None, None, Some(U256::ZERO), base_fee, None, None, None)
                .unwrap();
        assert!(gas_price.is_zero());

        let call_fees =
            CallFees::ensure_fees(None, None, Some(U256::from(GWEI)), base_fee, None, None, None);
        assert!(matches!(call_fees, Err(CallFeesError::TipAboveFeeCap)));

        // A tip above a fee cap that is also below the base fee reports the tip.
        let call_fees = CallFees::ensure_fees(
            None,
            Some(U256::ZERO),
            Some(U256::from(GWEI)),
            base_fee,
            None,
            None,
            None,
        );
        assert!(matches!(call_fees, Err(CallFeesError::TipAboveFeeCap)));
    }

    #[test]
    fn blob_fees_with_legacy_gas_price() {
        let hashes = [B256::ZERO];

        let CallFees { gas_price, max_fee_per_blob_gas, .. } = CallFees::ensure_fees(
            Some(U256::from(2 * GWEI)),
            None,
            None,
            U256::from(GWEI),
            Some(&hashes),
            Some(U256::from(1)),
            Some(U256::from(1)),
        )
        .unwrap();
        assert_eq!(gas_price, U256::from(2 * GWEI));
        assert_eq!(max_fee_per_blob_gas, Some(U256::from(1)));

        // Only the blob fee is set, so execution runs free.
        let CallFees { gas_price, .. } = CallFees::ensure_fees(
            None,
            None,
            None,
            U256::from(GWEI),
            Some(&hashes),
            Some(U256::from(1)),
            Some(U256::from(1)),
        )
        .unwrap();
        assert!(gas_price.is_zero());

        let call_fees = CallFees::ensure_fees(
            Some(U256::from(2 * GWEI)),
            None,
            None,
            U256::from(GWEI),
            None,
            Some(U256::from(1)),
            Some(U256::from(1)),
        );
        assert!(matches!(call_fees, Err(CallFeesError::BlobTransactionMissingBlobHashes)));
    }
}
