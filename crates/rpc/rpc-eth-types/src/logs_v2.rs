//! Request and response types of the `eth_getLogsV2` prototype.
//!
//! Based on <https://github.com/ethereum/execution-apis/pull/900>, with a log-level cursor so a
//! scan can resume inside a block.

use crate::logs_utils::{block_transaction_hash, ProviderOrBlock};
use alloy_consensus::{BlockHeader, TxReceipt};
use alloy_eips::BlockNumberOrTag;
use alloy_primitives::{Address, Bytes, Log, Sealable, TxHash, B256};
use alloy_rpc_types_eth::{Filter, FilterBlockOption};
use reth_errors::ProviderError;
use reth_primitives_traits::{SealedHeader, SignedTransaction};
use reth_storage_api::BlockReader;
use serde::{Deserialize, Serialize};

/// Request of `eth_getLogsV2`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RawLogsV2Filter", into = "RawLogsV2Filter")]
pub struct LogsV2Filter {
    /// Where the scan starts.
    pub start: LogsV2Start,
    /// Inclusive end of the scan.
    ///
    /// `None` means `latest` when scanning in ascending order, and the earliest available block
    /// when scanning in descending order.
    pub to_block: Option<BlockNumberOrTag>,
    /// Order in which blocks and logs are scanned.
    pub direction: Direction,
    /// Address and topic criteria.
    ///
    /// The block option is always the default, unbounded range.
    pub filter: Filter,
    /// Maximum number of logs the client wants, the server may return fewer.
    pub max_logs: Option<u64>,
    /// Maximum number of blocks the client wants scanned, the server may scan fewer.
    pub max_blocks: Option<u64>,
    /// Optional log fields to return, `None` returns all of them.
    pub fields: Option<Vec<LogField>>,
}

/// Start of an `eth_getLogsV2` scan.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogsV2Start {
    /// Resume after the cursor, which must be on the canonical chain.
    After(LogCursor),
    /// Start at this block, inclusive. Used for the first page only.
    FromBlock(BlockNumberOrTag),
}

/// Order of an `eth_getLogsV2` scan.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    /// Oldest log first.
    #[default]
    Asc,
    /// Newest log first.
    Desc,
}

/// Position in the log stream: after the log with `log_index` in the block, or after the whole
/// block if `log_index` is `None`.
///
/// "After" follows the scan's [`Direction`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogCursor {
    /// Block number.
    #[serde(with = "alloy_serde::quantity")]
    pub block_number: u64,
    /// Block hash, which lets the server detect that the client's chain was reorged.
    pub block_hash: B256,
    /// Index of the last delivered log within the block.
    #[serde(default, with = "alloy_serde::quantity::opt", skip_serializing_if = "Option::is_none")]
    pub log_index: Option<u64>,
}

/// Optional field of a [`LogV2`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LogField {
    /// [`LogV2::address`].
    Address,
    /// [`LogV2::topics`].
    Topics,
    /// [`LogV2::data`].
    Data,
    /// [`LogBlock::timestamp`].
    BlockTimestamp,
    /// [`LogV2::transaction_hash`].
    TransactionHash,
    /// [`LogV2::transaction_index`].
    TransactionIndex,
}

/// Response of `eth_getLogsV2`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogsV2Result {
    /// Matching logs, in the order of the scan's [`Direction`].
    pub logs: Vec<LogV2>,
    /// The blocks that contain [`Self::logs`], once each and in the same order.
    ///
    /// A block split across pages is listed on each of them.
    pub blocks: Vec<LogBlock>,
    /// Cursor to resume from.
    ///
    /// The scan reached the requested `toBlock` if this is a whole-block cursor at `toBlock`.
    /// Following the head is only possible in ascending order.
    /// Following the head means polling with this cursor, which returns no logs and the same
    /// cursor until a new block arrives.
    pub next_cursor: LogCursor,
    /// The canonical head when the server executed the query.
    pub head_block: BlockRef,
    /// The finalized block when the server executed the query, `None` if unknown.
    pub finalized_block: Option<BlockRef>,
}

/// Log returned by `eth_getLogsV2`.
///
/// The position fields are always set, the others only if selected in [`LogsV2Filter::fields`].
/// The block fields are stated once per block in [`LogsV2Result::blocks`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogV2 {
    /// Number of the block that contains the log.
    #[serde(with = "alloy_serde::quantity")]
    pub block_number: u64,
    /// Index of the log within the block.
    #[serde(with = "alloy_serde::quantity")]
    pub log_index: u64,
    /// Address of the contract that emitted the log.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub address: Option<Address>,
    /// Topics of the log.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub topics: Option<Vec<B256>>,
    /// Data of the log.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Bytes>,
    /// Hash of the transaction that emitted the log.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transaction_hash: Option<B256>,
    /// Index of the transaction that emitted the log within the block.
    #[serde(default, with = "alloy_serde::quantity::opt", skip_serializing_if = "Option::is_none")]
    pub transaction_index: Option<u64>,
}

/// Block that contains logs of an `eth_getLogsV2` page.
///
/// A page is checked to come from a single chain, so the block number identifies the block of a
/// log within the page.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogBlock {
    /// Block number.
    #[serde(with = "alloy_serde::quantity")]
    pub number: u64,
    /// Block hash.
    pub hash: B256,
    /// Block timestamp, only set if [`LogField::BlockTimestamp`] is selected.
    #[serde(default, with = "alloy_serde::quantity::opt", skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<u64>,
}

/// Number, hash and parent hash of a block.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BlockRef {
    /// Block number.
    #[serde(with = "alloy_serde::quantity")]
    pub number: u64,
    /// Block hash.
    pub hash: B256,
    /// Hash of the parent block.
    pub parent_hash: B256,
}

impl<H: BlockHeader + Sealable> From<&SealedHeader<H>> for BlockRef {
    fn from(header: &SealedHeader<H>) -> Self {
        Self { number: header.number(), hash: header.hash(), parent_hash: header.parent_hash() }
    }
}

/// Appends the logs of a block that match the filter and come after `after_log_index` in the given
/// direction, with only the selected fields set, and the block to `blocks` if it has any.
///
/// Stops once `logs` holds `max_logs` logs and returns the index of the last appended log in that
/// case. The transaction hash is only looked up if it is selected, which saves reading the
/// transaction.
#[expect(clippy::too_many_arguments)]
pub fn append_matching_block_logs_v2<P, H, R>(
    logs: &mut Vec<LogV2>,
    blocks: &mut Vec<LogBlock>,
    provider_or_block: &ProviderOrBlock<'_, P>,
    filter: &Filter,
    fields: Option<&[LogField]>,
    header: &SealedHeader<H>,
    receipts: &[R],
    direction: Direction,
    after_log_index: Option<u64>,
    max_logs: usize,
) -> Result<Option<u64>, ProviderError>
where
    P: BlockReader<Transaction: SignedTransaction>,
    H: BlockHeader + Sealable,
    R: TxReceipt<Log = Log>,
{
    let selected = |field| fields.is_none_or(|fields| fields.contains(&field));
    let with_address = selected(LogField::Address);
    let with_topics = selected(LogField::Topics);
    let with_data = selected(LogField::Data);
    let with_block_timestamp = selected(LogField::BlockTimestamp);
    let with_transaction_hash = selected(LogField::TransactionHash);
    let with_transaction_index = selected(LogField::TransactionIndex);

    let mut first_tx_num = None;
    // The hash of the last looked up transaction, which emitted the previous matching log.
    let mut last_transaction_hash = None::<(usize, Option<TxHash>)>;
    // Appends the log if it matches and returns whether the page is full.
    let mut append = |receipt_idx: usize, log_index: u64, log: &Log| {
        if after_log_index.is_some_and(|after| match direction {
            Direction::Asc => log_index <= after,
            Direction::Desc => log_index >= after,
        }) || !filter.matches(log)
        {
            return Ok(false);
        }
        let transaction_hash = match last_transaction_hash {
            _ if !with_transaction_hash => None,
            Some((idx, hash)) if idx == receipt_idx => hash,
            _ => {
                let hash = block_transaction_hash(
                    provider_or_block,
                    header.number(),
                    receipt_idx,
                    &mut first_tx_num,
                )?;
                last_transaction_hash = Some((receipt_idx, hash));
                hash
            }
        };
        if blocks.last().is_none_or(|block| block.number != header.number()) {
            blocks.push(LogBlock {
                number: header.number(),
                hash: header.hash(),
                timestamp: with_block_timestamp.then(|| header.timestamp()),
            });
        }
        logs.push(LogV2 {
            block_number: header.number(),
            log_index,
            address: with_address.then_some(log.address),
            topics: with_topics.then(|| log.topics().to_vec()),
            data: with_data.then(|| log.data.data.clone()),
            transaction_hash,
            // The transaction and receipt index is always the same.
            transaction_index: with_transaction_index.then_some(receipt_idx as u64),
        });
        Ok::<_, ProviderError>(logs.len() >= max_logs)
    };

    match direction {
        Direction::Asc => {
            let mut log_index = 0;
            for (receipt_idx, receipt) in receipts.iter().enumerate() {
                for log in receipt.logs() {
                    if append(receipt_idx, log_index, log)? {
                        return Ok(Some(log_index));
                    }
                    log_index += 1;
                }
            }
        }
        Direction::Desc => {
            let mut log_index = receipts.iter().map(|receipt| receipt.logs().len() as u64).sum();
            for (receipt_idx, receipt) in receipts.iter().enumerate().rev() {
                for log in receipt.logs().iter().rev() {
                    log_index -= 1;
                    if append(receipt_idx, log_index, log)? {
                        return Ok(Some(log_index));
                    }
                }
            }
        }
    }
    Ok(None)
}

/// Wire format of [`LogsV2Filter`].
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawLogsV2Filter {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    after: Option<LogCursor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    from_block: Option<BlockNumberOrTag>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    to_block: Option<BlockNumberOrTag>,
    #[serde(default)]
    direction: Direction,
    // `Filter` rejects unknown fields, which keeps the object closed.
    #[serde(flatten)]
    filter: Filter,
    #[serde(default, with = "alloy_serde::quantity::opt", skip_serializing_if = "Option::is_none")]
    max_logs: Option<u64>,
    #[serde(default, with = "alloy_serde::quantity::opt", skip_serializing_if = "Option::is_none")]
    max_blocks: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    fields: Option<Vec<LogField>>,
}

impl TryFrom<RawLogsV2Filter> for LogsV2Filter {
    type Error = LogsV2FilterError;

    fn try_from(raw: RawLogsV2Filter) -> Result<Self, Self::Error> {
        if raw.filter.block_option != FilterBlockOption::default() {
            return Err(LogsV2FilterError::BlockHash);
        }
        let start = match (raw.after, raw.from_block) {
            (Some(cursor), None) => LogsV2Start::After(cursor),
            (None, Some(from_block)) => LogsV2Start::FromBlock(from_block),
            _ => return Err(LogsV2FilterError::Start),
        };
        Ok(Self {
            start,
            to_block: raw.to_block,
            direction: raw.direction,
            filter: raw.filter,
            max_logs: raw.max_logs,
            max_blocks: raw.max_blocks,
            fields: raw.fields,
        })
    }
}

impl From<LogsV2Filter> for RawLogsV2Filter {
    fn from(filter: LogsV2Filter) -> Self {
        let (after, from_block) = match filter.start {
            LogsV2Start::After(cursor) => (Some(cursor), None),
            LogsV2Start::FromBlock(from_block) => (None, Some(from_block)),
        };
        Self {
            after,
            from_block,
            to_block: filter.to_block,
            direction: filter.direction,
            filter: filter.filter,
            max_logs: filter.max_logs,
            max_blocks: filter.max_blocks,
            fields: filter.fields,
        }
    }
}

/// Errors of decoding a [`LogsV2Filter`].
#[derive(Debug, thiserror::Error)]
pub enum LogsV2FilterError {
    /// The filter contains `blockHash`.
    #[error("blockHash is not supported, use after")]
    BlockHash,
    /// Neither or both of `after` and `fromBlock` are set.
    #[error("exactly one of after and fromBlock must be set")]
    Start,
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{address, b256};
    use serde_json::json;

    #[test]
    fn deserialize_filter() {
        let filter: LogsV2Filter = serde_json::from_value(json!({
            "after": {
                "blockNumber": "0x233",
                "blockHash": "0xfc139f5e2edee9e9c888d8df9a2d2226133a9bd87c88ccbd9c930d3d4c9f9ef5",
                "logIndex": "0x4"
            },
            "toBlock": "0x238",
            "direction": "desc",
            "address": "0x42699a7612a82f1d9c36148af9c77354759b210b",
            "topics": [],
            "maxLogs": "0xa",
            "fields": ["topics", "transactionHash"]
        }))
        .unwrap();

        assert_eq!(
            filter,
            LogsV2Filter {
                start: LogsV2Start::After(LogCursor {
                    block_number: 0x233,
                    block_hash: b256!(
                        "0xfc139f5e2edee9e9c888d8df9a2d2226133a9bd87c88ccbd9c930d3d4c9f9ef5"
                    ),
                    log_index: Some(4),
                }),
                to_block: Some(BlockNumberOrTag::Number(0x238)),
                direction: Direction::Desc,
                filter: Filter::new()
                    .address(address!("0x42699a7612a82f1d9c36148af9c77354759b210b")),
                max_logs: Some(10),
                max_blocks: None,
                fields: Some(vec![LogField::Topics, LogField::TransactionHash]),
            }
        );
        assert_eq!(
            serde_json::from_value::<LogsV2Filter>(serde_json::to_value(&filter).unwrap()).unwrap(),
            filter
        );

        let filter: LogsV2Filter =
            serde_json::from_value(json!({ "fromBlock": "earliest" })).unwrap();
        assert_eq!(filter.start, LogsV2Start::FromBlock(BlockNumberOrTag::Earliest));
        assert_eq!(filter.direction, Direction::Asc);
    }

    #[test]
    fn reject_invalid_filters() {
        let cursor = json!({ "blockNumber": "0x1", "blockHash": B256::ZERO });
        for (value, expected) in [
            (json!({}), "exactly one of after and fromBlock must be set"),
            (
                json!({ "after": cursor, "fromBlock": "0x1" }),
                "exactly one of after and fromBlock must be set",
            ),
            (
                json!({ "fromBlock": "0x1", "blockHash": B256::ZERO }),
                "blockHash is not supported, use after",
            ),
        ] {
            let err = serde_json::from_value::<LogsV2Filter>(value).unwrap_err();
            assert_eq!(err.to_string(), expected);
        }
        let err = serde_json::from_value::<LogsV2Filter>(
            json!({ "fromBlock": "0x1", "fields": ["removed"] }),
        )
        .unwrap_err();
        assert_eq!(
            err.to_string(),
            "unknown variant `removed`, expected one of `address`, `topics`, `data`, \
             `blockTimestamp`, `transactionHash`, `transactionIndex`"
        );
    }

    #[test]
    fn serialize_selected_fields_only() {
        let log = LogV2 {
            block_number: 1,
            log_index: 2,
            address: None,
            topics: Some(vec![]),
            data: None,
            transaction_hash: None,
            transaction_index: Some(3),
        };
        assert_eq!(
            serde_json::to_value(&log).unwrap(),
            json!({
                "blockNumber": "0x1",
                "logIndex": "0x2",
                "topics": [],
                "transactionIndex": "0x3"
            })
        );
        let block = LogBlock { number: 1, hash: B256::ZERO, timestamp: None };
        assert_eq!(
            serde_json::to_value(block).unwrap(),
            json!({ "number": "0x1", "hash": B256::ZERO })
        );
    }
}
