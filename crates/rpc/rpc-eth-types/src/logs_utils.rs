//! Helper functions for `reth_rpc_eth_api::EthFilterApiServer` implementation.
//!
//! Log parsing for building filter.

use reth_chainspec::ChainInfo;
use reth_errors::ProviderError;
use reth_primitives::{BlockNumHash, Receipt, TxHash};
use reth_rpc_server_types::result::rpc_error_with_code;
use reth_rpc_types::{FilterId, FilteredParams, Log};
use reth_storage_api::BlockReader;

use crate::EthApiError;

/// Errors that can occur in the handler implementation
#[derive(Debug, thiserror::Error)]
pub enum EthFilterError {
    /// Filter not found.
    #[error("filter not found")]
    FilterNotFound(FilterId),
    /// Invalid block range.
    #[error("invalid block range params")]
    InvalidBlockRangeParams,
    /// Query scope is too broad.
    #[error("query exceeds max block range {0}")]
    QueryExceedsMaxBlocks(u64),
    /// Query result is too large.
    #[error("query exceeds max results {0}")]
    QueryExceedsMaxResults(usize),
    /// Error serving request in `eth_` namespace.
    #[error(transparent)]
    EthAPIError(#[from] EthApiError),
    /// Error thrown when a spawned task failed to deliver a response.
    #[error("internal filter error")]
    InternalError,
}

// convert the error
impl From<EthFilterError> for jsonrpsee_types::error::ErrorObject<'static> {
    fn from(err: EthFilterError) -> Self {
        match err {
            EthFilterError::FilterNotFound(_) => {
                rpc_error_with_code(jsonrpsee_types::error::INVALID_PARAMS_CODE, "filter not found")
            }
            err @ EthFilterError::InternalError => {
                rpc_error_with_code(jsonrpsee_types::error::INTERNAL_ERROR_CODE, err.to_string())
            }
            EthFilterError::EthAPIError(err) => err.into(),
            err @ (EthFilterError::InvalidBlockRangeParams |
            EthFilterError::QueryExceedsMaxBlocks(_) |
            EthFilterError::QueryExceedsMaxResults(_)) => {
                rpc_error_with_code(jsonrpsee_types::error::INVALID_PARAMS_CODE, err.to_string())
            }
        }
    }
}

impl From<ProviderError> for EthFilterError {
    fn from(err: ProviderError) -> Self {
        Self::EthAPIError(err.into())
    }
}

/// Returns all matching of a block's receipts when the transaction hashes are known.
pub fn matching_block_logs_with_tx_hashes<'a, I>(
    filter: &FilteredParams,
    block_num_hash: BlockNumHash,
    tx_hashes_and_receipts: I,
    removed: bool,
) -> Vec<Log>
where
    I: IntoIterator<Item = (TxHash, &'a Receipt)>,
{
    let mut all_logs = Vec::new();
    // Tracks the index of a log in the entire block.
    let mut log_index: u64 = 0;
    // Iterate over transaction hashes and receipts and append matching logs.
    for (receipt_idx, (tx_hash, receipt)) in tx_hashes_and_receipts.into_iter().enumerate() {
        for log in &receipt.logs {
            if log_matches_filter(block_num_hash, log, filter) {
                let log = Log {
                    inner: log.clone(),
                    block_hash: Some(block_num_hash.hash),
                    block_number: Some(block_num_hash.number),
                    transaction_hash: Some(tx_hash),
                    // The transaction and receipt index is always the same.
                    transaction_index: Some(receipt_idx as u64),
                    log_index: Some(log_index),
                    removed,
                    block_timestamp: None,
                };
                all_logs.push(log);
            }
            log_index += 1;
        }
    }
    all_logs
}

/// Appends all matching logs of a block's receipts.
/// If the log matches, look up the corresponding transaction hash.
pub fn append_matching_block_logs(
    all_logs: &mut Vec<Log>,
    provider: impl BlockReader,
    filter: &FilteredParams,
    block_num_hash: BlockNumHash,
    receipts: &[Receipt],
    removed: bool,
    block_timestamp: u64,
) -> Result<(), EthFilterError> {
    // Tracks the index of a log in the entire block.
    let mut log_index: u64 = 0;

    // Lazy loaded number of the first transaction in the block.
    // This is useful for blocks with multiple matching logs because it prevents
    // re-querying the block body indices.
    let mut loaded_first_tx_num = None;

    // Iterate over receipts and append matching logs.
    for (receipt_idx, receipt) in receipts.iter().enumerate() {
        // The transaction hash of the current receipt.
        let mut transaction_hash = None;

        for log in &receipt.logs {
            if log_matches_filter(block_num_hash, log, filter) {
                let first_tx_num = match loaded_first_tx_num {
                    Some(num) => num,
                    None => {
                        let block_body_indices =
                            provider.block_body_indices(block_num_hash.number)?.ok_or(
                                ProviderError::BlockBodyIndicesNotFound(block_num_hash.number),
                            )?;
                        loaded_first_tx_num = Some(block_body_indices.first_tx_num);
                        block_body_indices.first_tx_num
                    }
                };

                // if this is the first match in the receipt's logs, look up the transaction hash
                if transaction_hash.is_none() {
                    // This is safe because Transactions and Receipts have the same keys.
                    let transaction_id = first_tx_num + receipt_idx as u64;
                    let transaction = provider
                        .transaction_by_id(transaction_id)?
                        .ok_or(ProviderError::TransactionNotFound(transaction_id.into()))?;

                    transaction_hash = Some(transaction.hash());
                }

                let log = Log {
                    inner: log.clone(),
                    block_hash: Some(block_num_hash.hash),
                    block_number: Some(block_num_hash.number),
                    transaction_hash,
                    // The transaction and receipt index is always the same.
                    transaction_index: Some(receipt_idx as u64),
                    log_index: Some(log_index),
                    removed,
                    block_timestamp: Some(block_timestamp),
                };
                all_logs.push(log);
            }
            log_index += 1;
        }
    }
    Ok(())
}

/// Returns true if the log matches the filter and should be included
pub fn log_matches_filter(
    block: BlockNumHash,
    log: &reth_primitives::Log,
    params: &FilteredParams,
) -> bool {
    if params.filter.is_some() &&
        (!params.filter_block_range(block.number) ||
            !params.filter_block_hash(block.hash) ||
            !params.filter_address(&log.address) ||
            !params.filter_topics(log.topics()))
    {
        return false
    }
    true
}

/// Computes the block range based on the filter range and current block numbers
pub fn get_filter_block_range(
    from_block: Option<u64>,
    to_block: Option<u64>,
    start_block: u64,
    info: ChainInfo,
) -> (u64, u64) {
    let mut from_block_number = start_block;
    let mut to_block_number = info.best_number;

    // if a `from_block` argument is provided then the `from_block_number` is the converted value or
    // the start block if the converted value is larger than the start block, since `from_block`
    // can't be a future block: `min(head, from_block)`
    if let Some(filter_from_block) = from_block {
        from_block_number = start_block.min(filter_from_block)
    }

    // upper end of the range is the converted `to_block` argument, restricted by the best block:
    // `min(best_number,to_block_number)`
    if let Some(filter_to_block) = to_block {
        to_block_number = info.best_number.min(filter_to_block);
    }

    (from_block_number, to_block_number)
}

#[cfg(test)]
mod tests {
    use reth_rpc_types::Filter;

    use super::*;

    #[test]
    fn test_log_range_from_and_to() {
        let from = 14000000u64;
        let to = 14000100u64;
        let info = ChainInfo { best_number: 15000000, ..Default::default() };
        let range = get_filter_block_range(Some(from), Some(to), info.best_number, info);
        assert_eq!(range, (from, to));
    }

    #[test]
    fn test_log_range_higher() {
        let from = 15000001u64;
        let to = 15000002u64;
        let info = ChainInfo { best_number: 15000000, ..Default::default() };
        let range = get_filter_block_range(Some(from), Some(to), info.best_number, info);
        assert_eq!(range, (info.best_number, info.best_number));
    }

    #[test]
    fn test_log_range_from() {
        let from = 14000000u64;
        let info = ChainInfo { best_number: 15000000, ..Default::default() };
        let range = get_filter_block_range(Some(from), None, info.best_number, info);
        assert_eq!(range, (from, info.best_number));
    }

    #[test]
    fn test_log_range_to() {
        let to = 14000000u64;
        let info = ChainInfo { best_number: 15000000, ..Default::default() };
        let range = get_filter_block_range(None, Some(to), info.best_number, info);
        assert_eq!(range, (info.best_number, to));
    }

    #[test]
    fn test_log_range_empty() {
        let info = ChainInfo { best_number: 15000000, ..Default::default() };
        let range = get_filter_block_range(None, None, info.best_number, info);

        // no range given -> head
        assert_eq!(range, (info.best_number, info.best_number));
    }

    #[test]
    fn parse_log_from_only() {
        let s = r#"{"fromBlock":"0xf47a42","address":["0x7de93682b9b5d80d45cd371f7a14f74d49b0914c","0x0f00392fcb466c0e4e4310d81b941e07b4d5a079","0xebf67ab8cff336d3f609127e8bbf8bd6dd93cd81"],"topics":["0x0559884fd3a460db3073b7fc896cc77986f16e378210ded43186175bf646fc5f"]}"#;
        let filter: Filter = serde_json::from_str(s).unwrap();

        assert_eq!(filter.get_from_block(), Some(16022082));
        assert!(filter.get_to_block().is_none());

        let best_number = 17229427;
        let info = ChainInfo { best_number, ..Default::default() };

        let (from_block, to_block) = filter.block_option.as_range();

        let start_block = info.best_number;

        let (from_block_number, to_block_number) = get_filter_block_range(
            from_block.and_then(reth_rpc_types::BlockNumberOrTag::as_number),
            to_block.and_then(reth_rpc_types::BlockNumberOrTag::as_number),
            start_block,
            info,
        );
        assert_eq!(from_block_number, 16022082);
        assert_eq!(to_block_number, best_number);
    }
}
