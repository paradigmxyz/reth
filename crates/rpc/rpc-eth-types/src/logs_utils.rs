//! Helper functions for `reth_rpc_eth_api::EthFilterApiServer` implementation.
//!
//! Log parsing for building filter.

use alloy_consensus::TxReceipt;
use alloy_eips::{eip2718::Encodable2718, BlockNumHash, BlockNumberOrTag};
use alloy_primitives::{keccak256, TxHash};
use alloy_rpc_types_eth::{Filter, FilterBlockOption, Log};
use data_encoding::BASE64URL_NOPAD;
use reth_chainspec::ChainInfo;
use reth_errors::ProviderError;
use reth_primitives_traits::{BlockBody, RecoveredBlock, SignedTransaction};
use reth_storage_api::{BlockReader, ProviderBlock};
use std::sync::Arc;
use thiserror::Error;

/// Returns all matching of a block's receipts when the transaction hashes are known.
pub fn matching_block_logs_with_tx_hashes<'a, I, R>(
    filter: &Filter,
    block_num_hash: BlockNumHash,
    block_timestamp: u64,
    tx_hashes_and_receipts: I,
    removed: bool,
) -> Vec<Log>
where
    I: IntoIterator<Item = (TxHash, &'a R)>,
    R: TxReceipt<Log = alloy_primitives::Log> + 'a,
{
    if !filter.matches_block(&block_num_hash) {
        return vec![];
    }

    let mut all_logs = Vec::new();
    // Tracks the index of a log in the entire block.
    let mut log_index: u64 = 0;

    // Iterate over transaction hashes and receipts and append matching logs.
    for (receipt_idx, (tx_hash, receipt)) in tx_hashes_and_receipts.into_iter().enumerate() {
        for log in receipt.logs() {
            if filter.matches(log) {
                let log = Log {
                    inner: log.clone(),
                    block_hash: Some(block_num_hash.hash),
                    block_number: Some(block_num_hash.number),
                    transaction_hash: Some(tx_hash),
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
    all_logs
}

/// Helper enum to fetch a transaction either from a block or from the provider.
#[derive(Debug)]
pub enum ProviderOrBlock<'a, P: BlockReader> {
    /// Provider
    Provider(&'a P),
    /// [`RecoveredBlock`]
    Block(Arc<RecoveredBlock<ProviderBlock<P>>>),
}

/// Appends all matching logs of a block's receipts.
/// If the log matches, look up the corresponding transaction hash.
pub fn append_matching_block_logs<P>(
    all_logs: &mut Vec<Log>,
    provider_or_block: ProviderOrBlock<'_, P>,
    filter: &Filter,
    block_num_hash: BlockNumHash,
    receipts: &[P::Receipt],
    removed: bool,
    block_timestamp: u64,
) -> Result<(), ProviderError>
where
    P: BlockReader<Transaction: SignedTransaction>,
{
    if !filter.matches_block(&block_num_hash) {
        return Ok(());
    }

    // Tracks the index of a log in the entire block.
    let mut log_index: u64 = 0;

    // Lazy loaded number of the first transaction in the block.
    // This is useful for blocks with multiple matching logs because it
    // prevents re-querying the block body indices.
    let mut loaded_first_tx_num = None;

    // Iterate over receipts and append matching logs.
    for (receipt_idx, receipt) in receipts.iter().enumerate() {
        // The transaction hash of the current receipt.
        let mut transaction_hash = None;

        for log in receipt.logs() {
            if filter.matches(log) {
                // if this is the first match in the receipt's logs, look up the transaction hash
                if transaction_hash.is_none() {
                    transaction_hash = match &provider_or_block {
                        ProviderOrBlock::Block(block) => {
                            block.body().transactions().get(receipt_idx).map(|t| t.trie_hash())
                        }
                        ProviderOrBlock::Provider(provider) => {
                            let first_tx_num = match loaded_first_tx_num {
                                Some(num) => num,
                                None => {
                                    let block_body_indices = provider
                                        .block_body_indices(block_num_hash.number)?
                                        .ok_or(ProviderError::BlockBodyIndicesNotFound(
                                            block_num_hash.number,
                                        ))?;
                                    loaded_first_tx_num = Some(block_body_indices.first_tx_num);
                                    block_body_indices.first_tx_num
                                }
                            };

                            // This is safe because Transactions and Receipts have the same
                            // keys.
                            let transaction_id = first_tx_num + receipt_idx as u64;
                            let transaction =
                                provider.transaction_by_id(transaction_id)?.ok_or_else(|| {
                                    ProviderError::TransactionNotFound(transaction_id.into())
                                })?;

                            Some(transaction.trie_hash())
                        }
                    };
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

/// Computes the block range based on the filter range and current block numbers.
///
/// Returns an error for invalid ranges rather than silently clamping values.
pub fn get_filter_block_range(
    from_block: Option<u64>,
    to_block: Option<u64>,
    start_block: u64,
    info: ChainInfo,
) -> Result<(u64, u64), FilterBlockRangeError> {
    let from_block_number = from_block.unwrap_or(start_block);
    let to_block_number = to_block.unwrap_or(info.best_number);

    // from > to is an invalid range
    if from_block_number > to_block_number {
        return Err(FilterBlockRangeError::InvalidBlockRange);
    }

    // we cannot query blocks that don't exist yet
    if to_block_number > info.best_number {
        return Err(FilterBlockRangeError::BlockRangeExceedsHead {
            requested: to_block_number,
            head: info.best_number,
        });
    }

    Ok((from_block_number, to_block_number))
}

/// Opaque cursor returned by `eth_getLogsWithCursor` to resume paginated log queries.
///
/// Encodes (version, filter hash, next block to resume from) into a base64url-no-pad
/// string. Opaque to clients — they should treat it as a string token to pass back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogsCursor {
    version: u8,
    filter_hash: [u8; 16],
    next_block: u64,
}

/// Current cursor format version. Incremented on wire-format changes.
const LOGS_CURSOR_VERSION: u8 = 1;

/// Decoded cursor length in bytes: 1 (version) + 16 (filter hash) + 8 (next block BE).
const LOGS_CURSOR_BYTES: usize = 25;

impl LogsCursor {
    /// Build a cursor for the current format version.
    pub const fn new(filter_hash: [u8; 16], next_block: u64) -> Self {
        Self { version: LOGS_CURSOR_VERSION, filter_hash, next_block }
    }

    /// Block number the next paginated call should resume from.
    pub const fn next_block(&self) -> u64 {
        self.next_block
    }

    /// Hash of the filter this cursor was issued for; used by the server to detect
    /// callers reusing a cursor against a different filter.
    pub const fn filter_hash(&self) -> &[u8; 16] {
        &self.filter_hash
    }

    /// Encode to the wire-format base64url-no-pad string.
    pub fn encode(&self) -> String {
        let mut bytes = [0u8; LOGS_CURSOR_BYTES];
        bytes[0] = self.version;
        bytes[1..17].copy_from_slice(&self.filter_hash);
        bytes[17..25].copy_from_slice(&self.next_block.to_be_bytes());
        BASE64URL_NOPAD.encode(&bytes)
    }

    /// Decode from the wire-format base64url-no-pad string.
    pub fn decode(s: &str) -> Result<Self, LogsCursorError> {
        let bytes = BASE64URL_NOPAD.decode(s.as_bytes()).map_err(|_| LogsCursorError::Invalid)?;
        if bytes.len() != LOGS_CURSOR_BYTES {
            return Err(LogsCursorError::Invalid);
        }
        let version = bytes[0];
        if version != LOGS_CURSOR_VERSION {
            return Err(LogsCursorError::UnsupportedVersion(version));
        }
        let mut filter_hash = [0u8; 16];
        filter_hash.copy_from_slice(&bytes[1..17]);
        let next_block = u64::from_be_bytes(bytes[17..25].try_into().expect("checked length"));
        Ok(Self { version, filter_hash, next_block })
    }
}

/// Hashes a [`Filter`] into a deterministic 16-byte fingerprint used for cursor binding.
///
/// Filters that differ only in the order of addresses or topics produce the same fingerprint.
/// Cursors carry this fingerprint so the server can detect callers reusing a cursor against
/// a different filter and reject early instead of returning silently wrong results.
pub fn canonical_filter_hash(filter: &Filter) -> [u8; 16] {
    let mut buf = Vec::with_capacity(128);

    match &filter.block_option {
        FilterBlockOption::Range { from_block, to_block } => {
            buf.push(0);
            serialize_block_number_or_tag(&mut buf, from_block.as_ref());
            serialize_block_number_or_tag(&mut buf, to_block.as_ref());
        }
        FilterBlockOption::AtBlockHash(hash) => {
            buf.push(1);
            buf.extend_from_slice(hash.as_slice());
        }
    }

    let mut addrs: Vec<_> = filter.address.iter().collect();
    addrs.sort();
    buf.extend_from_slice(&(addrs.len() as u32).to_be_bytes());
    for addr in addrs {
        buf.extend_from_slice(addr.as_slice());
    }

    for topic in &filter.topics {
        let mut topics: Vec<_> = topic.iter().collect();
        topics.sort();
        buf.extend_from_slice(&(topics.len() as u32).to_be_bytes());
        for t in topics {
            buf.extend_from_slice(t.as_slice());
        }
    }

    let hash = keccak256(&buf);
    hash[..16].try_into().expect("keccak256 returns 32 bytes")
}

fn serialize_block_number_or_tag(buf: &mut Vec<u8>, opt: Option<&BlockNumberOrTag>) {
    match opt {
        Some(BlockNumberOrTag::Number(n)) => {
            buf.push(0);
            buf.extend_from_slice(&n.to_be_bytes());
        }
        Some(BlockNumberOrTag::Latest) => buf.push(1),
        Some(BlockNumberOrTag::Earliest) => buf.push(2),
        Some(BlockNumberOrTag::Pending) => buf.push(3),
        Some(BlockNumberOrTag::Safe) => buf.push(4),
        Some(BlockNumberOrTag::Finalized) => buf.push(5),
        None => buf.push(0xff),
    }
}

/// Errors decoding a [`LogsCursor`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum LogsCursorError {
    /// Cursor string was not valid base64url or had the wrong length.
    #[error("invalid cursor")]
    Invalid,
    /// Cursor was encoded with a version this server does not support.
    #[error("unsupported cursor version: {0}")]
    UnsupportedVersion(u8),
}

/// Errors for filter block range validation.
///
/// See also <https://github.com/ethereum/go-ethereum/blob/master/eth/filters/filter.go#L224-L230>.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum FilterBlockRangeError {
    /// `from_block > to_block`
    #[error("invalid block range params")]
    InvalidBlockRange,
    /// Block range extends beyond current head
    #[error("block range extends beyond current head block: requested {requested}, head {head}")]
    BlockRangeExceedsHead {
        /// The requested `toBlock` number
        requested: u64,
        /// The current head block number
        head: u64,
    },
}

#[cfg(test)]
mod tests {
    use alloy_rpc_types_eth::Filter;

    use super::*;

    #[test]
    fn test_log_range_from_and_to() {
        let from = 14000000u64;
        let to = 14000100u64;
        let info = ChainInfo { best_number: 15000000, ..Default::default() };
        let range = get_filter_block_range(Some(from), Some(to), info.best_number, info).unwrap();
        assert_eq!(range, (from, to));
    }

    #[test]
    fn test_log_range_from() {
        let from = 14000000u64;
        let info = ChainInfo { best_number: 15000000, ..Default::default() };
        let range = get_filter_block_range(Some(from), None, 0, info).unwrap();
        assert_eq!(range, (from, info.best_number));
    }

    #[test]
    fn test_log_range_to() {
        let to = 14000000u64;
        let start_block = 0u64;
        let info = ChainInfo { best_number: 15000000, ..Default::default() };
        let range = get_filter_block_range(None, Some(to), start_block, info).unwrap();
        assert_eq!(range, (start_block, to));
    }

    #[test]
    fn test_log_range_higher_error() {
        // Range extends beyond head -> should error instead of clamping
        let from = 15000001u64;
        let to = 15000002u64;
        let info = ChainInfo { best_number: 15000000, ..Default::default() };
        let err = get_filter_block_range(Some(from), Some(to), info.best_number, info).unwrap_err();
        assert_eq!(
            err,
            FilterBlockRangeError::BlockRangeExceedsHead { requested: to, head: info.best_number }
        );
    }

    #[test]
    fn test_log_range_to_below_start_error() {
        // to_block < start_block, default from -> invalid range
        let to = 14000000u64;
        let info = ChainInfo { best_number: 15000000, ..Default::default() };
        let err = get_filter_block_range(None, Some(to), info.best_number, info).unwrap_err();
        assert_eq!(err, FilterBlockRangeError::InvalidBlockRange);
    }

    #[test]
    fn test_log_range_empty() {
        let info = ChainInfo { best_number: 15000000, ..Default::default() };
        let range = get_filter_block_range(None, None, info.best_number, info).unwrap();

        // no range given -> head
        assert_eq!(range, (info.best_number, info.best_number));
    }

    #[test]
    fn test_invalid_block_range_error() {
        let from = 100;
        let to = 50;
        let info = ChainInfo { best_number: 150, ..Default::default() };
        let err = get_filter_block_range(Some(from), Some(to), 0, info).unwrap_err();
        assert_eq!(err, FilterBlockRangeError::InvalidBlockRange);
    }

    #[test]
    fn test_block_range_exceeds_head_error() {
        let from = 100;
        let to = 200;
        let info = ChainInfo { best_number: 150, ..Default::default() };
        let err = get_filter_block_range(Some(from), Some(to), 0, info).unwrap_err();
        assert_eq!(
            err,
            FilterBlockRangeError::BlockRangeExceedsHead { requested: to, head: info.best_number }
        );
    }

    #[test]
    fn logs_cursor_roundtrip() {
        let filter_hash = [0xab; 16];
        let cursor = LogsCursor::new(filter_hash, 12345);
        let encoded = cursor.encode();
        let decoded = LogsCursor::decode(&encoded).unwrap();
        assert_eq!(decoded.next_block(), 12345);
        assert_eq!(decoded.filter_hash(), &filter_hash);
    }

    #[test]
    fn logs_cursor_rejects_unsupported_version() {
        let mut bytes = [0u8; LOGS_CURSOR_BYTES];
        bytes[0] = 99;
        let encoded = BASE64URL_NOPAD.encode(&bytes);
        let err = LogsCursor::decode(&encoded).unwrap_err();
        assert_eq!(err, LogsCursorError::UnsupportedVersion(99));
    }

    #[test]
    fn logs_cursor_rejects_wrong_length() {
        let too_short = BASE64URL_NOPAD.encode(&[1u8; 10]);
        assert_eq!(LogsCursor::decode(&too_short).unwrap_err(), LogsCursorError::Invalid);
        let too_long = BASE64URL_NOPAD.encode(&[1u8; 100]);
        assert_eq!(LogsCursor::decode(&too_long).unwrap_err(), LogsCursorError::Invalid);
    }

    #[test]
    fn logs_cursor_rejects_invalid_base64() {
        assert_eq!(LogsCursor::decode("not!valid!base64!").unwrap_err(), LogsCursorError::Invalid);
    }

    #[test]
    fn canonical_filter_hash_deterministic() {
        let filter = Filter::new()
            .address(alloy_primitives::address!("c02aaa39b223fe8d0a0e5c4f27ead9083c756cc2"))
            .from_block(100u64)
            .to_block(200u64);
        let h1 = canonical_filter_hash(&filter);
        let h2 = canonical_filter_hash(&filter);
        assert_eq!(h1, h2);
    }

    #[test]
    fn canonical_filter_hash_differs_per_filter() {
        let a = Filter::new().from_block(100u64).to_block(200u64);
        let b = Filter::new().from_block(100u64).to_block(300u64);
        assert_ne!(canonical_filter_hash(&a), canonical_filter_hash(&b));
    }

    #[test]
    fn canonical_filter_hash_address_order_independent() {
        let addr1 = alloy_primitives::address!("c02aaa39b223fe8d0a0e5c4f27ead9083c756cc2");
        let addr2 = alloy_primitives::address!("a0b86991c6218b36c1d19d4a2e9eb0ce3606eb48");
        let a = Filter::new().address(vec![addr1, addr2]);
        let b = Filter::new().address(vec![addr2, addr1]);
        assert_eq!(canonical_filter_hash(&a), canonical_filter_hash(&b));
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
            from_block.and_then(alloy_rpc_types_eth::BlockNumberOrTag::as_number),
            to_block.and_then(alloy_rpc_types_eth::BlockNumberOrTag::as_number),
            start_block,
            info,
        )
        .unwrap();
        assert_eq!(from_block_number, 16022082);
        assert_eq!(to_block_number, best_number);
    }
}
