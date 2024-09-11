//! Commonly used code snippets

use alloy_primitives::Bytes;
use reth_primitives::{PooledTransactionsElement, PooledTransactionsElementEcRecovered};
use std::future::Future;

use super::{EthApiError, EthResult};

/// Recovers a [`PooledTransactionsElementEcRecovered`] from an enveloped encoded byte stream.
///
/// See [`PooledTransactionsElement::decode_enveloped`]
pub fn recover_raw_transaction(data: Bytes) -> EthResult<PooledTransactionsElementEcRecovered> {
    if data.is_empty() {
        return Err(EthApiError::EmptyRawTransactionData)
    }

    let transaction = PooledTransactionsElement::decode_enveloped(&mut data.as_ref())
        .map_err(|_| EthApiError::FailedToDecodeSignedTransaction)?;

    transaction.try_into_ecrecovered().or(Err(EthApiError::InvalidTransactionSignature))
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
