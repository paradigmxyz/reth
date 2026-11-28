use alloy_primitives::{B256, U256};
use core::cmp::min;
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
    /// `maxFeePerBlobGas` only prices blob gas, so it does not take part in the execution fee
    /// checks: a legacy `gasPrice` prices the execution gas of a blob transaction, as it does in
    /// geth's `eth_call`, while `gasPrice` together with `maxFeePerGas` or `maxPriorityFeePerGas`
    /// is rejected.
    ///
    /// Note: Due to the `Default` impl of `BlockEnv` (Some(0)) this assumes the `block_blob_fee`
    /// is always `Some`
    ///
    /// ## Notable design decisions
    ///
    /// For compatibility reasons, this contains several exceptions when fee values are validated:
    /// - If both `maxFeePerGas` and `maxPriorityFeePerGas` are set to `0` they are treated as
    ///   missing values, bypassing fee checks wrt. `baseFeePerGas`.
    ///
    /// This mirrors geth's behaviour when transaction requests are executed: <https://github.com/ethereum/go-ethereum/blob/380688c636a654becc8f114438c2a5d93d2db032/core/state_transition.go#L306-L306>
    pub fn ensure_fees(
        call_gas_price: Option<U256>,
        call_max_fee: Option<U256>,
        call_priority_fee: Option<U256>,
        block_base_fee: U256,
        blob_versioned_hashes: Option<&[B256]>,
        max_fee_per_blob_gas: Option<U256>,
        block_blob_fee: Option<U256>,
    ) -> Result<Self, CallFeesError> {
        /// Get the effective gas price of a transaction as specified in EIP-1559 with relevant
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
                        return Err(CallFeesError::FeeCapTooLow);
                    }
                    if max_fee < max_priority_fee_per_gas {
                        return Err(
                            // `max_priority_fee_per_gas` is greater than the `max_fee_per_gas`
                            CallFeesError::TipAboveFeeCap,
                        );
                    }
                    // ref <https://github.com/ethereum/go-ethereum/blob/0dd173a727dd2d2409b8e401b22e85d20c25b71f/internal/ethapi/transaction_args.go#L446-L446>
                    Ok(min(
                        max_fee,
                        block_base_fee
                            .checked_add(max_priority_fee_per_gas)
                            .ok_or(CallFeesError::TipVeryHigh)?,
                    ))
                }
                None => Ok(block_base_fee
                    .checked_add(max_priority_fee_per_gas.unwrap_or(U256::ZERO))
                    .ok_or(CallFeesError::TipVeryHigh)?),
            }
        }

        let has_blob_hashes =
            blob_versioned_hashes.as_ref().map(|blobs| !blobs.is_empty()).unwrap_or(false);

        let (gas_price, max_priority_fee_per_gas) =
            match (call_gas_price, call_max_fee, call_priority_fee) {
                (gas_price, None, None) => {
                    // either legacy transaction or no fee fields are specified
                    // when no fields are specified, set gas price to zero
                    (gas_price.unwrap_or(U256::ZERO), None)
                }
                (None, max_fee_per_gas, max_priority_fee_per_gas) => {
                    // request for eip-1559 transaction
                    let effective_gas_price = get_effective_gas_price(
                        max_fee_per_gas,
                        max_priority_fee_per_gas,
                        block_base_fee,
                    )?;
                    (effective_gas_price, max_priority_fee_per_gas)
                }
                _ => {
                    // gasPrice together with maxFeePerGas or maxPriorityFeePerGas
                    return Err(CallFeesError::ConflictingFeeFieldsInRequest);
                }
            };

        // The blob fee cap only prices blob gas, so it combines with either fee style.
        let max_fee_per_blob_gas = match max_fee_per_blob_gas {
            Some(_) if !has_blob_hashes => {
                // Blob transaction but no blob hashes
                return Err(CallFeesError::BlobTransactionMissingBlobHashes);
            }
            Some(max_fee_per_blob_gas) => Some(max_fee_per_blob_gas),
            None => has_blob_hashes.then_some(block_blob_fee).flatten(),
        };

        Ok(Self { gas_price, max_priority_fee_per_gas, max_fee_per_blob_gas })
    }
}

/// Error coming from decoding and validating transaction request fees.
#[derive(Debug, Error)]
pub enum CallFeesError {
    /// Thrown when a call or transaction request (`eth_call`, `eth_estimateGas`,
    /// `eth_sendTransaction`) contains conflicting fields (legacy, EIP-1559)
    #[error("both gasPrice and (maxFeePerGas or maxPriorityFeePerGas) specified")]
    ConflictingFeeFieldsInRequest,
    /// Thrown post London if the transaction's fee is less than the base fee of the block
    #[error("max fee per gas less than block base fee")]
    FeeCapTooLow,
    /// Thrown to ensure no one is able to specify a transaction with a tip higher than the total
    /// fee cap.
    #[error("max priority fee per gas higher than max fee per gas")]
    TipAboveFeeCap,
    /// A sanity error to avoid huge numbers specified in the tip field.
    #[error("max priority fee per gas higher than 2^256-1")]
    TipVeryHigh,
    /// Blob transaction has no versioned hashes
    #[error("blob transaction missing blob hashes")]
    BlobTransactionMissingBlobHashes,
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::constants::GWEI_TO_WEI;

    #[test]
    fn test_ensure_0_fallback() {
        let CallFees { gas_price, .. } =
            CallFees::ensure_fees(None, None, None, U256::from(99), None, None, Some(U256::ZERO))
                .unwrap();
        assert!(gas_price.is_zero());
    }

    #[test]
    fn test_ensure_max_fee_0_exception() {
        let CallFees { gas_price, .. } =
            CallFees::ensure_fees(None, Some(U256::ZERO), None, U256::from(99), None, None, None)
                .unwrap();
        assert!(gas_price.is_zero());
    }

    #[test]
    fn test_blob_fees() {
        let CallFees { gas_price, max_fee_per_blob_gas, .. } =
            CallFees::ensure_fees(None, None, None, U256::from(99), None, None, Some(U256::ZERO))
                .unwrap();
        assert!(gas_price.is_zero());
        assert_eq!(max_fee_per_blob_gas, None);

        let CallFees { gas_price, max_fee_per_blob_gas, .. } = CallFees::ensure_fees(
            None,
            None,
            None,
            U256::from(99),
            Some(&[B256::from(U256::ZERO)]),
            None,
            Some(U256::from(99)),
        )
        .unwrap();
        assert!(gas_price.is_zero());
        assert_eq!(max_fee_per_blob_gas, Some(U256::from(99)));
    }

    #[test]
    fn test_legacy_blob_fees() {
        let gas_price = U256::from(2 * GWEI_TO_WEI);
        let base_fee = U256::from(GWEI_TO_WEI);
        let blob_hashes = [B256::with_last_byte(1)];

        // a blob fee cap is priced on its own, execution gas at gasPrice
        let CallFees { gas_price: price, max_priority_fee_per_gas, max_fee_per_blob_gas } =
            CallFees::ensure_fees(
                Some(gas_price),
                None,
                None,
                base_fee,
                Some(&blob_hashes),
                Some(U256::from(5)),
                Some(U256::from(1)),
            )
            .unwrap();
        assert_eq!(price, gas_price);
        assert_eq!(max_priority_fee_per_gas, None);
        assert_eq!(max_fee_per_blob_gas, Some(U256::from(5)));

        // without a blob fee cap the block's blob fee applies
        let CallFees { gas_price: price, max_fee_per_blob_gas, .. } = CallFees::ensure_fees(
            Some(gas_price),
            None,
            None,
            base_fee,
            Some(&blob_hashes),
            None,
            Some(U256::from(7)),
        )
        .unwrap();
        assert_eq!(price, gas_price);
        assert_eq!(max_fee_per_blob_gas, Some(U256::from(7)));

        // a blob fee cap still needs blob hashes
        for hashes in [None, Some(&[][..])] {
            let err = CallFees::ensure_fees(
                Some(gas_price),
                None,
                None,
                base_fee,
                hashes,
                Some(U256::from(5)),
                Some(U256::from(1)),
            )
            .unwrap_err();
            assert!(matches!(err, CallFeesError::BlobTransactionMissingBlobHashes));
        }
    }

    #[test]
    fn test_blob_fee_cap_without_execution_fees() {
        // a blob fee cap alone does not price execution gas
        let CallFees { gas_price, max_priority_fee_per_gas, max_fee_per_blob_gas } =
            CallFees::ensure_fees(
                None,
                None,
                None,
                U256::from(GWEI_TO_WEI),
                Some(&[B256::with_last_byte(1)]),
                Some(U256::from(5)),
                Some(U256::from(1)),
            )
            .unwrap();
        assert!(gas_price.is_zero());
        assert_eq!(max_priority_fee_per_gas, None);
        assert_eq!(max_fee_per_blob_gas, Some(U256::from(5)));
    }

    #[test]
    fn test_eip_4844_fees() {
        let CallFees { gas_price, max_priority_fee_per_gas, max_fee_per_blob_gas } =
            CallFees::ensure_fees(
                None,
                Some(U256::from(3 * GWEI_TO_WEI)),
                Some(U256::from(GWEI_TO_WEI)),
                U256::from(GWEI_TO_WEI),
                Some(&[B256::with_last_byte(1)]),
                Some(U256::from(5)),
                Some(U256::from(1)),
            )
            .unwrap();
        assert_eq!(gas_price, U256::from(2 * GWEI_TO_WEI));
        assert_eq!(max_priority_fee_per_gas, Some(U256::from(GWEI_TO_WEI)));
        assert_eq!(max_fee_per_blob_gas, Some(U256::from(5)));
    }

    #[test]
    fn test_conflicting_fee_fields() {
        let gas_price = Some(U256::from(2 * GWEI_TO_WEI));
        let fee = Some(U256::from(3 * GWEI_TO_WEI));
        let blob_hashes = [B256::with_last_byte(1)];

        for (max_fee, priority_fee) in [(fee, None), (None, fee), (fee, fee)] {
            for (hashes, blob_fee) in [
                (None, None),
                (None, Some(U256::from(5))),
                (Some(&blob_hashes[..]), None),
                (Some(&blob_hashes[..]), Some(U256::from(5))),
            ] {
                let err = CallFees::ensure_fees(
                    gas_price,
                    max_fee,
                    priority_fee,
                    U256::from(GWEI_TO_WEI),
                    hashes,
                    blob_fee,
                    Some(U256::from(1)),
                )
                .unwrap_err();
                assert!(matches!(err, CallFeesError::ConflictingFeeFieldsInRequest));
            }
        }
    }

    #[test]
    fn test_eip_1559_fees() {
        let CallFees { gas_price, .. } = CallFees::ensure_fees(
            None,
            Some(U256::from(25 * GWEI_TO_WEI)),
            Some(U256::from(15 * GWEI_TO_WEI)),
            U256::from(15 * GWEI_TO_WEI),
            None,
            None,
            Some(U256::ZERO),
        )
        .unwrap();
        assert_eq!(gas_price, U256::from(25 * GWEI_TO_WEI));

        let CallFees { gas_price, .. } = CallFees::ensure_fees(
            None,
            Some(U256::from(25 * GWEI_TO_WEI)),
            Some(U256::from(5 * GWEI_TO_WEI)),
            U256::from(15 * GWEI_TO_WEI),
            None,
            None,
            Some(U256::ZERO),
        )
        .unwrap();
        assert_eq!(gas_price, U256::from(20 * GWEI_TO_WEI));

        let CallFees { gas_price, .. } = CallFees::ensure_fees(
            None,
            Some(U256::from(30 * GWEI_TO_WEI)),
            Some(U256::from(30 * GWEI_TO_WEI)),
            U256::from(15 * GWEI_TO_WEI),
            None,
            None,
            Some(U256::ZERO),
        )
        .unwrap();
        assert_eq!(gas_price, U256::from(30 * GWEI_TO_WEI));

        let call_fees = CallFees::ensure_fees(
            None,
            Some(U256::from(30 * GWEI_TO_WEI)),
            Some(U256::from(31 * GWEI_TO_WEI)),
            U256::from(15 * GWEI_TO_WEI),
            None,
            None,
            Some(U256::ZERO),
        );
        assert!(call_fees.is_err());

        let call_fees = CallFees::ensure_fees(
            None,
            Some(U256::from(5 * GWEI_TO_WEI)),
            Some(U256::from(GWEI_TO_WEI)),
            U256::from(15 * GWEI_TO_WEI),
            None,
            None,
            Some(U256::ZERO),
        );
        assert!(call_fees.is_err());

        let call_fees = CallFees::ensure_fees(
            None,
            Some(U256::MAX),
            Some(U256::MAX),
            U256::from(5 * GWEI_TO_WEI),
            None,
            None,
            Some(U256::ZERO),
        );
        assert!(call_fees.is_err());
    }
}
