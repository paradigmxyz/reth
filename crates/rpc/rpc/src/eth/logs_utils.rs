use reth_primitives::{
    filter::FilteredParams, BlockNumHash, BlockNumberOrTag, ChainInfo, Receipt, TxHash, U256,
};
use reth_rpc_types::Log;

/// Returns all matching logs of a block's receipts grouped with the hash of their transaction.
pub(crate) fn matching_block_logs<I>(
    filter: &FilteredParams,
    block: BlockNumHash,
    tx_and_receipts: I,
    removed: bool,
) -> Vec<Log>
where
    I: IntoIterator<Item = (TxHash, Receipt)>,
{
    let mut all_logs = Vec::new();
    append_matching_block_logs(&mut all_logs, filter, block, tx_and_receipts, removed);
    all_logs
}

/// Appends all matching logs of a block's receipts grouped with the hash of their transaction
pub(crate) fn append_matching_block_logs<I>(
    all_logs: &mut Vec<Log>,
    filter: &FilteredParams,
    block: BlockNumHash,
    tx_and_receipts: I,
    removed: bool,
) where
    I: IntoIterator<Item = (TxHash, Receipt)>,
{
    let block_number_u256 = U256::from(block.number);
    // tracks the index of a log in the entire block
    let mut log_index: u32 = 0;
    for (transaction_idx, (transaction_hash, receipt)) in tx_and_receipts.into_iter().enumerate() {
        let logs = receipt.logs;
        for log in logs.into_iter() {
            if log_matches_filter(block, &log, filter) {
                let log = Log {
                    address: log.address,
                    topics: log.topics,
                    data: log.data,
                    block_hash: Some(block.hash),
                    block_number: Some(block_number_u256),
                    transaction_hash: Some(transaction_hash),
                    transaction_index: Some(U256::from(transaction_idx)),
                    log_index: Some(U256::from(log_index)),
                    removed,
                };
                all_logs.push(log);
            }
            log_index += 1;
        }
    }
}

/// Returns true if the log matches the filter and should be included
pub(crate) fn log_matches_filter(
    block: BlockNumHash,
    log: &reth_primitives::Log,
    params: &FilteredParams,
) -> bool {
    if params.filter.is_some() &&
        (!params.filter_block_range(block.number) ||
            !params.filter_block_hash(block.hash) ||
            !params.filter_address(log) ||
            !params.filter_topics(log))
    {
        return false
    }
    true
}

/// Computes the block range based on the filter range and current block numbers
pub(crate) fn get_filter_block_range(
    from_block: Option<BlockNumberOrTag>,
    to_block: Option<BlockNumberOrTag>,
    start_block: u64,
    info: ChainInfo,
) -> (u64, u64) {
    let mut from_block_number = start_block;
    let mut to_block_number = info.best_number;

    // if a `from_block` argument is provided then the `from_block_number` is the converted value or
    // the start block if the converted value is larger than the start block, since `from_block`
    // can't be a future block: `min(head, from_block)`
    if let Some(filter_from_block) = from_block.and_then(|num| info.convert_block_number(num)) {
        from_block_number = start_block.min(filter_from_block)
    }

    // upper end of the range is the converted `to_block` argument, restricted by the best block:
    // `min(best_number,to_block_number)`
    if let Some(filter_to_block) = to_block.and_then(|num| info.convert_block_number(num)) {
        to_block_number = info.best_number.min(filter_to_block);
    }

    (from_block_number, to_block_number)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_log_range_from_and_to() {
        let from: BlockNumberOrTag = 14000000u64.into();
        let to: BlockNumberOrTag = 14000100u64.into();
        let info = ChainInfo { best_number: 15000000, ..Default::default() };
        let range = get_filter_block_range(Some(from), Some(to), info.best_number, info);
        assert_eq!(range, (from.as_number().unwrap(), to.as_number().unwrap()));
    }

    #[test]
    fn test_log_range_higher() {
        let from: BlockNumberOrTag = 15000001u64.into();
        let to: BlockNumberOrTag = 15000002u64.into();
        let info = ChainInfo { best_number: 15000000, ..Default::default() };
        let range = get_filter_block_range(Some(from), Some(to), info.best_number, info.clone());
        assert_eq!(range, (info.best_number, info.best_number));
    }

    #[test]
    fn test_log_range_from() {
        let from: BlockNumberOrTag = 14000000u64.into();
        let info = ChainInfo { best_number: 15000000, ..Default::default() };
        let range = get_filter_block_range(Some(from), None, info.best_number, info.clone());
        assert_eq!(range, (from.as_number().unwrap(), info.best_number));
    }

    #[test]
    fn test_log_range_to() {
        let to: BlockNumberOrTag = 14000000u64.into();
        let info = ChainInfo { best_number: 15000000, ..Default::default() };
        let range = get_filter_block_range(None, Some(to), info.best_number, info.clone());
        assert_eq!(range, (info.best_number, to.as_number().unwrap()));
    }

    #[test]
    fn test_log_range_empty() {
        let info = ChainInfo { best_number: 15000000, ..Default::default() };
        let range = get_filter_block_range(None, None, info.best_number, info.clone());

        // no range given -> head
        assert_eq!(range, (info.best_number, info.best_number));
    }
}
