use alloy_primitives::{Address, Bytes, B256, U256};
use serde::{Deserialize, Serialize};

/// Ethereum Log emitted by a transaction
#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Log {
    /// Address
    pub address: Address,
    /// All topics of the log
    pub topics: Vec<B256>,
    /// Additional data fields of the log
    pub data: Bytes,
    /// Hash of the block the transaction that emitted this log was mined in
    pub block_hash: Option<B256>,
    /// Number of the block the transaction that emitted this log was mined in
    pub block_number: Option<U256>,
    /// Transaction Hash
    pub transaction_hash: Option<B256>,
    /// Index of the Transaction in the block
    pub transaction_index: Option<U256>,
    /// Log Index in Block
    pub log_index: Option<U256>,
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
            address: Address::with_last_byte(0x69),
            topics: vec![B256::with_last_byte(0x69)],
            data: Bytes::from_static(&[0x69]),
            block_hash: Some(B256::with_last_byte(0x69)),
            block_number: Some(U256::from(0x69)),
            transaction_hash: Some(B256::with_last_byte(0x69)),
            transaction_index: Some(U256::from(0x69)),
            log_index: Some(U256::from(0x69)),
            removed: false,
        };
        let serialized = serde_json::to_string(&log).unwrap();
        assert_eq!(
            serialized,
            r#"{"address":"0x0000000000000000000000000000000000000069","topics":["0x0000000000000000000000000000000000000000000000000000000000000069"],"data":"0x69","blockHash":"0x0000000000000000000000000000000000000000000000000000000000000069","blockNumber":"0x69","transactionHash":"0x0000000000000000000000000000000000000000000000000000000000000069","transactionIndex":"0x69","logIndex":"0x69","removed":false}"#
        );

        let deserialized: Log = serde_json::from_str(&serialized).unwrap();
        assert_eq!(log, deserialized);
    }
}
