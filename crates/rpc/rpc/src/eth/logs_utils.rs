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
        for (transaction_log_idx, log) in logs.into_iter().enumerate() {
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
                    transaction_log_index: Some(U256::from(transaction_log_idx)),
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

    // from block is maximum of block from last poll or `from_block` of filter
    if let Some(filter_from_block) = from_block.and_then(|num| info.convert_block_number(num)) {
        from_block_number = start_block.max(filter_from_block)
    }

    // to block is max the best number
    if let Some(filter_to_block) = to_block.and_then(|num| info.convert_block_number(num)) {
        to_block_number = filter_to_block;
        if to_block_number > info.best_number {
            to_block_number = info.best_number;
        }
    }
    (from_block_number, to_block_number)
}
