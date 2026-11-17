//! Commonly used code snippets

use super::{
    error::{AsEthApiError, FromEthApiError},
    EthApiError, EthResult,
};
use alloy_consensus::TxReceipt;
use alloy_rpc_types_eth::Bundle;
use evm2::{interpreter::InstrStop, TxResultExt};
use reth_evm::TransactionEnvMut;
use reth_primitives_traits::{Recovered, SignedTransaction};
use reth_rpc_server_types::constants::{DEFAULT_CALL_MANY_TIMEOUT, DEFAULT_MAX_CALL_MANY_CALLS};
use std::{future::Future, time::Instant};

/// Calculates the gas used and next log index for a transaction at the given index
pub fn calculate_gas_used_and_next_log_index(
    tx_index: u64,
    all_receipts: &[impl TxReceipt],
) -> (u64, usize) {
    let mut gas_used = 0;
    let mut next_log_index = 0;

    if tx_index > 0 {
        for receipt in all_receipts.iter().take(tx_index as usize) {
            gas_used = receipt.cumulative_gas_used();
            next_log_index += receipt.logs().len();
        }
    }

    (gas_used, next_log_index)
}

/// Recovers a [`SignedTransaction`] from an enveloped encoded byte stream.
///
/// This is a helper function that returns the appropriate RPC-specific error if the input data is
/// malformed.
///
/// This function uses [`alloy_eips::eip2718::Decodable2718::decode_2718_exact`] to ensure
/// that the entire input buffer is consumed and no trailing bytes are allowed.
///
/// See [`alloy_eips::eip2718::Decodable2718::decode_2718_exact`]
pub fn recover_raw_transaction<T: SignedTransaction>(data: &[u8]) -> EthResult<Recovered<T>> {
    SignedTransaction::try_into_recovered(decode_raw_transaction::<T>(data)?)
        .or(Err(EthApiError::InvalidTransactionSignature))
}

/// Decodes an EIP-2718 transaction without recovering its sender.
///
/// Rejects empty input, malformed transactions, and trailing bytes with RPC-specific errors.
pub fn decode_raw_transaction<T: SignedTransaction>(data: &[u8]) -> EthResult<T> {
    if data.is_empty() {
        return Err(EthApiError::EmptyRawTransactionData)
    }

    T::decode_2718_exact(data).map_err(|_| EthApiError::FailedToDecodeSignedTransaction)
}

/// Performs a binary search within a given block range to find the desired block number.
///
/// The binary search is performed by calling the provided asynchronous `check` closure on the
/// blocks of the range. The closure should return a future representing the result of performing
/// the desired logic at a given block. The future resolves to an `bool` where:
/// - `true` indicates that the condition has been matched, but we can try to find a lower block to
///   make the condition more matchable.
/// - `false` indicates that the condition not matched, so the target is not present in the current
///   block and should continue searching in a higher range.
///
/// Args:
/// - `low`: The lower bound of the block range (inclusive).
/// - `high`: The upper bound of the block range (inclusive).
/// - `check`: A closure that performs the desired logic at a given block.
pub async fn binary_search<F, Fut, E>(low: u64, high: u64, check: F) -> Result<u64, E>
where
    F: Fn(u64) -> Fut,
    Fut: Future<Output = Result<bool, E>>,
{
    let mut low = low;
    let mut high = high;
    let mut num = high;

    while low <= high {
        let mid = (low + high) / 2;
        if check(mid).await? {
            high = mid - 1;
            num = mid;
        } else {
            low = mid + 1
        }
    }

    Ok(num)
}

/// Calculates the blob gas used ratio for a block, accounting for the case where
/// `max_blob_gas_per_block` is zero.
///
/// Returns `0.0` if `max_blob_gas_per_block` is `0`, otherwise returns the ratio
/// `blob_gas_used/max_blob_gas_per_block`.
pub fn checked_blob_gas_used_ratio(blob_gas_used: u64, max_blob_gas_per_block: u64) -> f64 {
    if max_blob_gas_per_block == 0 {
        0.0
    } else {
        blob_gas_used as f64 / max_blob_gas_per_block as f64
    }
}

/// Bounds the work of one `eth_callMany`, `trace_callMany` or `debug_traceCallMany` request.
///
/// The request may carry at most [`DEFAULT_MAX_CALL_MANY_CALLS`] calls, all of its calls share
/// one gas budget of the RPC gas cap, and no call starts more than [`DEFAULT_CALL_MANY_TIMEOUT`]
/// after the first one.
#[derive(Debug)]
pub struct CallManyBudget {
    /// The RPC gas cap the calls share, reported once they exceed it.
    gas_cap: u64,
    /// Gas left for the remaining calls, `None` if the RPC gas cap is disabled.
    remaining_gas: Option<u64>,
    /// No call starts after this instant. Set when the first call is admitted, so waiting for a
    /// permit or replaying the block's transactions does not count.
    deadline: Option<Instant>,
    /// Whether [`Self::admit`] lowered the gas limit of the last admitted call.
    capped: bool,
}

impl CallManyBudget {
    /// Creates the budget for a request of `calls` calls that share `gas_cap` gas.
    ///
    /// Returns an error if `calls` exceeds [`DEFAULT_MAX_CALL_MANY_CALLS`]. A `gas_cap` of `0`
    /// disables the gas budget, as it disables the RPC gas cap.
    pub fn new(calls: usize, gas_cap: u64) -> Result<Self, EthApiError> {
        if calls > DEFAULT_MAX_CALL_MANY_CALLS {
            return Err(EthApiError::InvalidParams(format!(
                "call count {calls} exceeds limit {DEFAULT_MAX_CALL_MANY_CALLS}"
            )))
        }
        Ok(Self {
            gas_cap,
            remaining_gas: (gas_cap != 0).then_some(gas_cap),
            deadline: None,
            capped: false,
        })
    }

    /// Creates the budget for a request of `bundles`, see [`Self::new`].
    ///
    /// An empty bundle counts as one call, so requests of empty bundles are bounded too.
    pub fn for_bundles<T>(bundles: &[Bundle<T>], gas_cap: u64) -> Result<Self, EthApiError> {
        Self::new(bundles.iter().map(|bundle| bundle.transactions.len().max(1)).sum(), gas_cap)
    }

    /// Admits the request's next call, lowering the gas limit of `tx_env` to the gas left for the
    /// request's remaining calls.
    ///
    /// The first admitted call starts the request's timeout. Returns
    /// [`EthApiError::ExecutionTimedOut`] once it has expired.
    pub fn admit<T: TransactionEnvMut>(
        &mut self,
        tx_env: &mut Recovered<T>,
    ) -> Result<(), EthApiError> {
        let now = Instant::now();
        if now >= *self.deadline.get_or_insert(now + DEFAULT_CALL_MANY_TIMEOUT) {
            return Err(EthApiError::ExecutionTimedOut(DEFAULT_CALL_MANY_TIMEOUT))
        }
        self.capped = false;
        if let Some(remaining) = self.remaining_gas &&
            tx_env.gas_limit() > remaining
        {
            tx_env.inner_mut().set_gas_limit(remaining);
            self.capped = true;
        }
        Ok(())
    }

    /// Returns [`EthApiError::CallManyGasCapExceeded`] in place of `err` if the last admitted call
    /// failed its intrinsic gas check because [`Self::admit`] lowered its gas limit.
    pub fn map_call_err<E: AsEthApiError + FromEthApiError>(&self, err: E) -> E {
        if self.capped && err.is_gas_too_low() {
            return E::from_eth_err(EthApiError::CallManyGasCapExceeded(self.gas_cap))
        }
        err
    }

    /// Charges the gas the last admitted call used against the budget.
    ///
    /// Returns [`EthApiError::CallManyGasCapExceeded`] if the call ran out of gas after
    /// [`Self::admit`] lowered its gas limit, and an internal error if it used more gas than is
    /// left, which an admitted call cannot do.
    pub fn charge<E>(&mut self, result: &TxResultExt<E>) -> Result<(), EthApiError> {
        if self.capped && result.stop.is_out_of_gas() && result.stop != InstrStop::MemoryLimitOOG {
            return Err(EthApiError::CallManyGasCapExceeded(self.gas_cap))
        }
        if let Some(remaining) = &mut self.remaining_gas {
            *remaining =
                remaining.checked_sub(result.tx_gas_used()).ok_or(EthApiError::InternalEthError)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::{Transaction, TxLegacy};
    use alloy_primitives::Address;
    use evm2::ethereum::TxEnvelope;

    #[test]
    fn call_many_budget_limits_calls() {
        assert!(CallManyBudget::new(DEFAULT_MAX_CALL_MANY_CALLS, 0).is_ok());

        let empty = Bundle::<()> { transactions: Vec::new(), block_override: None };
        let full =
            Bundle { transactions: vec![(); DEFAULT_MAX_CALL_MANY_CALLS], block_override: None };
        assert!(CallManyBudget::for_bundles(&vec![empty.clone(); DEFAULT_MAX_CALL_MANY_CALLS], 0)
            .is_ok());
        assert_eq!(
            CallManyBudget::for_bundles(&vec![empty.clone(); DEFAULT_MAX_CALL_MANY_CALLS + 1], 0)
                .unwrap_err()
                .to_string(),
            "call count 1025 exceeds limit 1024"
        );
        assert_eq!(
            CallManyBudget::for_bundles(&[full, empty], 0).unwrap_err().to_string(),
            "call count 1025 exceeds limit 1024"
        );
    }

    fn tx_env(gas_limit: u64) -> Recovered<TxEnvelope> {
        Recovered::new_unchecked(
            TxEnvelope::Legacy(TxLegacy { gas_limit, ..Default::default() }),
            Address::ZERO,
        )
    }

    fn tx_result(gas_used: u64, stop: InstrStop) -> TxResultExt {
        TxResultExt {
            status: stop.is_success(),
            total_gas_spent: gas_used,
            stop,
            ..Default::default()
        }
    }

    #[test]
    fn call_many_budget_shares_gas_cap() {
        let admitted = |budget: &mut CallManyBudget, gas_limit| {
            let mut tx_env = tx_env(gas_limit);
            budget.admit(&mut tx_env).unwrap();
            tx_env.gas_limit()
        };

        let mut budget = CallManyBudget::new(3, 50_000).unwrap();
        assert_eq!(admitted(&mut budget, 30_000), 30_000);
        // A call that was not capped keeps its own out-of-gas result.
        budget.charge(&tx_result(30_000, InstrStop::OutOfGas)).unwrap();
        assert_eq!(admitted(&mut budget, 30_000_000), 20_000);
        assert!(matches!(
            budget.charge(&tx_result(20_000, InstrStop::OutOfGas)),
            Err(EthApiError::CallManyGasCapExceeded(50_000))
        ));
        assert_eq!(admitted(&mut budget, 10_000), 10_000);
        assert!(matches!(
            budget.charge(&tx_result(20_001, InstrStop::Stop)),
            Err(EthApiError::InternalEthError)
        ));

        let mut unlimited = CallManyBudget::new(3, 0).unwrap();
        unlimited.charge(&tx_result(u64::MAX, InstrStop::Stop)).unwrap();
        assert_eq!(admitted(&mut unlimited, u64::MAX), u64::MAX);
    }

    #[test]
    fn call_many_budget_times_out() {
        let mut budget = CallManyBudget::new(2, 0).unwrap();
        assert_eq!(budget.deadline, None);
        budget.admit(&mut tx_env(21_000)).unwrap();
        assert!(budget.deadline.is_some());

        budget.deadline = Some(Instant::now());
        assert!(matches!(
            budget.admit(&mut tx_env(21_000)),
            Err(EthApiError::ExecutionTimedOut(timeout)) if timeout == DEFAULT_CALL_MANY_TIMEOUT
        ));
    }

    #[tokio::test]
    async fn test_binary_search() {
        // in the middle
        let num: Result<_, ()> =
            binary_search(1, 10, |mid| Box::pin(async move { Ok(mid >= 5) })).await;
        assert_eq!(num, Ok(5));

        // in the upper
        let num: Result<_, ()> =
            binary_search(1, 10, |mid| Box::pin(async move { Ok(mid >= 7) })).await;
        assert_eq!(num, Ok(7));

        // in the lower
        let num: Result<_, ()> =
            binary_search(1, 10, |mid| Box::pin(async move { Ok(mid >= 1) })).await;
        assert_eq!(num, Ok(1));

        // higher than the upper
        let num: Result<_, ()> =
            binary_search(1, 10, |mid| Box::pin(async move { Ok(mid >= 11) })).await;
        assert_eq!(num, Ok(10));
    }

    #[test]
    fn test_checked_blob_gas_used_ratio() {
        // No blob gas used, max blob gas per block is 0
        assert_eq!(checked_blob_gas_used_ratio(0, 0), 0.0);
        // Blob gas used is non-zero, max blob gas per block is 0 (division by zero protection)
        assert_eq!(checked_blob_gas_used_ratio(50, 0), 0.0);
        // Blob gas used is zero, max blob gas per block is non-zero
        assert_eq!(checked_blob_gas_used_ratio(0, 100), 0.0);
        // Blob gas used is non-zero, max blob gas per block is non-zero
        assert_eq!(checked_blob_gas_used_ratio(50, 100), 0.5);
        // Blob gas used is non-zero and equal to max blob gas per block
        assert_eq!(checked_blob_gas_used_ratio(100, 100), 1.0);
    }
}
