//! utilities for working with revm

use reth_primitives::{Address, B256, U256};
use reth_rpc_types::{
    state::{AccountOverride, StateOverride},
    BlockOverrides,
};
use revm::{
    db::CacheDB,
    precompile::{PrecompileSpecId, Precompiles},
    primitives::{db::DatabaseRef, Bytecode, SpecId, TxEnv},
    Database,
};
use revm_primitives::BlockEnv;
use std::cmp::min;

use super::{EthApiError, EthResult, RpcInvalidTransactionError};

/// Returns the addresses of the precompiles corresponding to the `SpecId`.
#[inline]
pub fn get_precompiles(spec_id: SpecId) -> impl IntoIterator<Item = Address> {
    let spec = PrecompileSpecId::from_spec_id(spec_id);
    Precompiles::new(spec).addresses().copied().map(Address::from)
}

/// Caps the configured [`TxEnv`] `gas_limit` with the allowance of the caller.
pub fn cap_tx_gas_limit_with_caller_allowance<DB>(db: &mut DB, env: &mut TxEnv) -> EthResult<()>
where
    DB: Database,
    EthApiError: From<<DB as Database>::Error>,
{
    if let Ok(gas_limit) = caller_gas_allowance(db, env)?.try_into() {
        env.gas_limit = gas_limit;
    }

    Ok(())
}

/// Calculates the caller gas allowance.
///
/// `allowance = (account.balance - tx.value) / tx.gas_price`
///
/// Returns an error if the caller has insufficient funds.
/// Caution: This assumes non-zero `env.gas_price`. Otherwise, zero allowance will be returned.
pub fn caller_gas_allowance<DB>(db: &mut DB, env: &TxEnv) -> EthResult<U256>
where
    DB: Database,
    EthApiError: From<<DB as Database>::Error>,
{
    Ok(db
        // Get the caller account.
        .basic(env.caller)?
        // Get the caller balance.
        .map(|acc| acc.balance)
        .unwrap_or_default()
        // Subtract transferred value from the caller balance.
        .checked_sub(env.value)
        // Return error if the caller has insufficient funds.
        .ok_or_else(|| RpcInvalidTransactionError::InsufficientFunds)?
        // Calculate the amount of gas the caller can afford with the specified gas price.
        .checked_div(env.gas_price)
        // This will be 0 if gas price is 0. It is fine, because we check it before.
        .unwrap_or_default())
}

/// Helper type for representing the fees of a [`reth_rpc_types::TransactionRequest`]
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

// === impl CallFees ===

impl CallFees {
    /// Ensures the fields of a [`reth_rpc_types::TransactionRequest`] are not conflicting.
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
    pub fn ensure_fees(
        call_gas_price: Option<U256>,
        call_max_fee: Option<U256>,
        call_priority_fee: Option<U256>,
        block_base_fee: U256,
        blob_versioned_hashes: Option<&[B256]>,
        max_fee_per_blob_gas: Option<U256>,
        block_blob_fee: Option<U256>,
    ) -> EthResult<Self> {
        /// Get the effective gas price of a transaction as specfified in EIP-1559 with relevant
        /// checks.
        fn get_effective_gas_price(
            max_fee_per_gas: Option<U256>,
            max_priority_fee_per_gas: Option<U256>,
            block_base_fee: U256,
        ) -> EthResult<U256> {
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
                    Ok(min(
                        max_fee,
                        block_base_fee.checked_add(max_priority_fee_per_gas).ok_or_else(|| {
                            EthApiError::from(RpcInvalidTransactionError::TipVeryHigh)
                        })?,
                    ))
                }
                None => Ok(block_base_fee
                    .checked_add(max_priority_fee_per_gas.unwrap_or(U256::ZERO))
                    .ok_or_else(|| EthApiError::from(RpcInvalidTransactionError::TipVeryHigh))?),
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
                Err(EthApiError::ConflictingFeeFieldsInRequest)
            }
        }
    }
}

/// Applies the given block overrides to the env
pub fn apply_block_overrides(overrides: BlockOverrides, env: &mut BlockEnv) {
    let BlockOverrides {
        number,
        difficulty,
        time,
        gas_limit,
        coinbase,
        random,
        base_fee,
        block_hash: _,
    } = overrides;

    if let Some(number) = number {
        env.number = number;
    }
    if let Some(difficulty) = difficulty {
        env.difficulty = difficulty;
    }
    if let Some(time) = time {
        env.timestamp = U256::from(time);
    }
    if let Some(gas_limit) = gas_limit {
        env.gas_limit = U256::from(gas_limit);
    }
    if let Some(coinbase) = coinbase {
        env.coinbase = coinbase;
    }
    if let Some(random) = random {
        env.prevrandao = Some(random);
    }
    if let Some(base_fee) = base_fee {
        env.basefee = base_fee;
    }
}

/// Applies the given state overrides (a set of [`AccountOverride`]) to the [`CacheDB`].
pub fn apply_state_overrides<DB>(overrides: StateOverride, db: &mut CacheDB<DB>) -> EthResult<()>
where
    DB: DatabaseRef,
    EthApiError: From<<DB as DatabaseRef>::Error>,
{
    for (account, account_overrides) in overrides {
        apply_account_override(account, account_overrides, db)?;
    }
    Ok(())
}

/// Applies a single [`AccountOverride`] to the [`CacheDB`].
fn apply_account_override<DB>(
    account: Address,
    account_override: AccountOverride,
    db: &mut CacheDB<DB>,
) -> EthResult<()>
where
    DB: DatabaseRef,
    EthApiError: From<<DB as DatabaseRef>::Error>,
{
    // we need to fetch the account via the `DatabaseRef` to not update the state of the account,
    // which is modified via `Database::basic_ref`
    let mut account_info = DatabaseRef::basic_ref(db, account)?.unwrap_or_default();

    if let Some(nonce) = account_override.nonce {
        account_info.nonce = nonce;
    }
    if let Some(code) = account_override.code {
        account_info.code = Some(Bytecode::new_raw(code));
    }
    if let Some(balance) = account_override.balance {
        account_info.balance = balance;
    }

    db.insert_account_info(account, account_info);

    // We ensure that not both state and state_diff are set.
    // If state is set, we must mark the account as "NewlyCreated", so that the old storage
    // isn't read from
    match (account_override.state, account_override.state_diff) {
        (Some(_), Some(_)) => return Err(EthApiError::BothStateAndStateDiffInOverride(account)),
        (None, None) => {
            // nothing to do
        }
        (Some(new_account_state), None) => {
            db.replace_account_storage(
                account,
                new_account_state
                    .into_iter()
                    .map(|(slot, value)| {
                        (U256::from_be_bytes(slot.0), U256::from_be_bytes(value.0))
                    })
                    .collect(),
            )?;
        }
        (None, Some(account_state_diff)) => {
            for (slot, value) in account_state_diff {
                db.insert_account_storage(
                    account,
                    U256::from_be_bytes(slot.0),
                    U256::from_be_bytes(value.0),
                )?;
            }
        }
    };

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use reth_primitives::constants::GWEI_TO_WEI;

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
