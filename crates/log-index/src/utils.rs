use alloy_primitives::{Address, B256};
use reth_primitives_traits::Receipt;
use sha2::{Digest, Sha256};

/// Compute the log value hash of a log emitting address.
pub fn address_value(address: &Address) -> B256 {
    let mut hasher = Sha256::new();
    hasher.update(address.as_slice());
    B256::from_slice(&hasher.finalize())
}

/// Compute the log value hash of a log topic.
pub fn topic_value(topic: &B256) -> B256 {
    let mut hasher = Sha256::new();
    hasher.update(topic.as_slice());
    B256::from_slice(&hasher.finalize())
}

/// Count the log values in a block's receipts.
///
/// The log value count is the number of log values in a single block's receipts, which includes:
/// - 1 for the address of each log
/// - 1 for each topic in each log
pub fn count_log_values_in_block<R: Receipt>(receipts: &[R]) -> u64 {
    receipts.iter().fold(0, |acc, receipt| {
        acc + receipt.logs().iter().fold(0, |log_acc, log| {
            log_acc + 1 + log.topics().len() as u64 // 1 for address + topics count
        })
    })
}

/// Return an Iterator over all log values in a slice of receipts.
///
/// The log values include the address value and all topic values for each log in the block's
/// receipts.
pub fn log_values_from_receipts<'a, R: Receipt + 'a>(
    receipts: &'a [R],
) -> impl Iterator<Item = B256> + 'a {
    receipts.iter().flat_map(|receipt| {
        receipt.logs().iter().flat_map(|log| {
            std::iter::once(address_value(&log.address)).chain(log.topics().iter().map(topic_value))
        })
    })
}
