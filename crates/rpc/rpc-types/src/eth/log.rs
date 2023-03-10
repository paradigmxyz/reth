use reth_primitives::{Address, Bytes, H256, U256};
use serde::{Deserialize, Serialize};

/// Ethereum Log emitted by a transaction
#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Log {
    /// Address
    pub address: Address,
    /// All topics of the log
    pub topics: Vec<H256>,
    /// Additional data fields of the log
    pub data: Bytes,
    /// Hash of the block the transaction that emitted this log was mined in
    pub block_hash: Option<H256>,
    /// Number of the block the transaction that emitted this log was mined in
    pub block_number: Option<U256>,
    /// Transaction Hash
    pub transaction_hash: Option<H256>,
    /// Index of the Transaction in the block
    pub transaction_index: Option<U256>,
    /// Log Index in Block
    pub log_index: Option<U256>,
    /// Log Index in Transaction
    pub transaction_log_index: Option<U256>,
    /// Geth Compatibility Field: whether this log was removed
    #[serde(default)]
    pub removed: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serde_log() {
        let log = Log {
            address: Address::from_low_u64_be(0x1234),
            topics: vec![H256::from_low_u64_be(0x1234)],
            data: Bytes::from(vec![0x12, 0x34]),
            block_hash: Some(H256::from_low_u64_be(0x1234)),
            block_number: Some(U256::from(0x1234)),
            transaction_hash: Some(H256::from_low_u64_be(0x1234)),
            transaction_index: Some(U256::from(0x1234)),
            log_index: Some(U256::from(0x1234)),
            transaction_log_index: Some(U256::from(0x1234)),
            removed: false,
        };
        let serialized = serde_json::to_string(&log).unwrap();
        assert_eq!(
            serialized,
            r#"{"address":"0x0000000000000000000000000000000000001234","topics":["0x0000000000000000000000000000000000000000000000000000000000001234"],"data":"0x1234","blockHash":"0x0000000000000000000000000000000000000000000000000000000000001234","blockNumber":"0x1234","transactionHash":"0x0000000000000000000000000000000000000000000000000000000000001234","transactionIndex":"0x1234","logIndex":"0x1234","transactionLogIndex":"0x1234","removed":false}"#
        );

        let deserialized: Log = serde_json::from_str(&serialized).unwrap();
        assert_eq!(log, deserialized);
    }
}
