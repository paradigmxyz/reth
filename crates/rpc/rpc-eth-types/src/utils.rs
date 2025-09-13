//! Commonly used code snippets

use super::{EthApiError, EthResult};
use reth_primitives_traits::{Recovered, SignedTransaction};
use std::future::Future;

/// Recovers a [`SignedTransaction`] from an enveloped encoded byte stream.
///
/// This is a helper function that returns the appropriate RPC-specific error if the input data is
/// malformed.
///
/// See [`alloy_eips::eip2718::Decodable2718::decode_2718`]
pub fn recover_raw_transaction<T: SignedTransaction>(mut data: &[u8]) -> EthResult<Recovered<T>> {
    if data.is_empty() {
        return Err(EthApiError::EmptyRawTransactionData)
    }

    let transaction =
        T::decode_2718(&mut data).map_err(|_| EthApiError::FailedToDecodeSignedTransaction)?;

    SignedTransaction::try_into_recovered(transaction)
        .or(Err(EthApiError::InvalidTransactionSignature))
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
/// Returns `0.0` if `blob_gas_used` is `0`, otherwise returns the ratio
/// `blob_gas_used/max_blob_gas_per_block`.
pub fn checked_blob_gas_used_ratio(blob_gas_used: u64, max_blob_gas_per_block: u64) -> f64 {
    if blob_gas_used == 0 {
        0.0
    } else {
        blob_gas_used as f64 / max_blob_gas_per_block as f64
    }
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

    #[test]
    fn test_checked_blob_gas_used_ratio() {
        // No blob gas used, max blob gas per block is 0
        assert_eq!(checked_blob_gas_used_ratio(0, 0), 0.0);
        // Blob gas used is zero, max blob gas per block is non-zero
        assert_eq!(checked_blob_gas_used_ratio(0, 100), 0.0);
        // Blob gas used is non-zero, max blob gas per block is non-zero
        assert_eq!(checked_blob_gas_used_ratio(50, 100), 0.5);
        // Blob gas used is non-zero and equal to max blob gas per block
        assert_eq!(checked_blob_gas_used_ratio(100, 100), 1.0);
    }
}
