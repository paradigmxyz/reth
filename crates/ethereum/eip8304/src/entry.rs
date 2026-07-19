use alloy_primitives::{Address, B256};
use core::cmp::Ordering;

use crate::constants::{
    ENTRY_TYPE_BLOCK, ENTRY_TYPE_LOG_ADDRESS, ENTRY_TYPE_LOG_TOPIC0, ENTRY_TYPE_LOG_TOPIC1,
    ENTRY_TYPE_LOG_TOPIC2, ENTRY_TYPE_LOG_TOPIC3, ENTRY_TYPE_TRANSACTION,
};

/// A single index entry representing a block, transaction, or log event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexEntry {
    pub entry_type: u8,
    pub search_content: B256,
    pub block_number: u64,
    pub tx_index: u32,
    pub log_index: u32,
}

impl IndexEntry {
    pub fn block_entry(block_number: u64, block_hash: B256) -> Self {
        Self {
            entry_type: ENTRY_TYPE_BLOCK,
            search_content: block_hash,
            block_number,
            tx_index: 0,
            log_index: 0,
        }
    }

    pub fn transaction_entry(
        block_number: u64,
        tx_index: u32,
        tx_hash: B256,
        cumulative_log_count: u32,
    ) -> Self {
        Self {
            entry_type: ENTRY_TYPE_TRANSACTION,
            search_content: tx_hash,
            block_number,
            tx_index,
            log_index: cumulative_log_count,
        }
    }

    pub fn log_address_entry(
        block_number: u64,
        tx_index: u32,
        log_index: u32,
        address: Address,
    ) -> Self {
        let mut content = B256::ZERO;
        content[12..32].copy_from_slice(address.as_slice());
        Self {
            entry_type: ENTRY_TYPE_LOG_ADDRESS,
            search_content: content,
            block_number,
            tx_index,
            log_index,
        }
    }

    pub fn log_topic_entry(
        block_number: u64,
        tx_index: u32,
        log_index: u32,
        topic_index: u8,
        topic: B256,
    ) -> Self {
        let entry_type = match topic_index {
            0 => ENTRY_TYPE_LOG_TOPIC0,
            1 => ENTRY_TYPE_LOG_TOPIC1,
            2 => ENTRY_TYPE_LOG_TOPIC2,
            3 => ENTRY_TYPE_LOG_TOPIC3,
            _ => {
                return Self {
                    entry_type: ENTRY_TYPE_LOG_TOPIC0,
                    search_content: topic,
                    block_number,
                    tx_index,
                    log_index,
                }
            }
        };
        Self { entry_type, search_content: topic, block_number, tx_index, log_index }
    }

    /// Build sorted index entries from a block's transactions and receipts.
    ///
    /// `tx_hashes` provides the transaction hash for each transaction.
    /// `receipt_logs` provides the logs for each receipt, where each log yields
    /// (address, topics iterator).
    pub fn build_from_block(
        block_number: u64,
        tx_hashes: &[B256],
        receipt_logs: &[Vec<(Address, Vec<B256>)>],
    ) -> Vec<Self> {
        let mut entries = Vec::new();
        let mut cumulative_log_count = 0u32;

        for (tx_idx, (tx_hash, logs)) in tx_hashes.iter().zip(receipt_logs.iter()).enumerate() {
            entries.push(Self::transaction_entry(
                block_number,
                tx_idx as u32,
                *tx_hash,
                cumulative_log_count,
            ));

            for (log_idx, (address, topics)) in logs.iter().enumerate() {
                entries.push(Self::log_address_entry(
                    block_number,
                    tx_idx as u32,
                    log_idx as u32,
                    *address,
                ));

                for (topic_idx, topic) in topics.iter().enumerate() {
                    if topic_idx > 3 {
                        break;
                    }
                    entries.push(Self::log_topic_entry(
                        block_number,
                        tx_idx as u32,
                        log_idx as u32,
                        topic_idx as u8,
                        *topic,
                    ));
                }
            }

            cumulative_log_count += logs.len() as u32;
        }

        entries.sort();
        entries
    }

    /// Binary encode the entry in big-endian format for lexicographic ordering.
    ///
    /// Block:       2 + 32 + 8 = 42 bytes
    /// Transaction: 2 + 32 + 8 + 4 + 4 = 50 bytes
    /// Log address: 2 + 20 + 8 + 4 + 4 = 38 bytes
    /// Log topic:   2 + 32 + 8 + 4 + 4 = 50 bytes
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.push(0u8);
        buf.push(self.entry_type);

        match self.entry_type {
            ENTRY_TYPE_BLOCK |
            ENTRY_TYPE_TRANSACTION |
            ENTRY_TYPE_LOG_TOPIC0 |
            ENTRY_TYPE_LOG_TOPIC1 |
            ENTRY_TYPE_LOG_TOPIC2 |
            ENTRY_TYPE_LOG_TOPIC3 => {
                buf.extend_from_slice(self.search_content.as_slice());
            }
            ENTRY_TYPE_LOG_ADDRESS => {
                buf.extend_from_slice(&self.search_content[12..32]);
            }
            _ => {}
        }

        buf.extend_from_slice(&self.block_number.to_be_bytes());
        match self.entry_type {
            ENTRY_TYPE_BLOCK => {}
            _ => {
                buf.extend_from_slice(&self.tx_index.to_be_bytes());
                buf.extend_from_slice(&self.log_index.to_be_bytes());
            }
        }

        buf
    }
}

impl Ord for IndexEntry {
    fn cmp(&self, other: &Self) -> Ordering {
        self.encode().cmp(&other.encode())
    }
}

impl PartialOrd for IndexEntry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::b256;

    #[test]
    fn test_block_entry_encoding() {
        let hash = b256!("42f66a2e9f9c68e223e8d826145d7cfacb00520dba6a9555803121de29790b65");
        let entry = IndexEntry::block_entry(40, hash);
        let encoded = entry.encode();
        assert_eq!(encoded.len(), 42);
        assert_eq!(&encoded[..2], &[0u8, ENTRY_TYPE_BLOCK]);
        assert_eq!(&encoded[2..34], hash.as_slice());
        assert_eq!(&encoded[34..42], &40u64.to_be_bytes());
    }

    #[test]
    fn test_tx_entry_encoding() {
        let hash = b256!("ca2d12d1b8132de09d0d668cc87349dc70134bee3010e03ddb2d83f7160bd6e3");
        let entry = IndexEntry::transaction_entry(42, 0, hash, 0);
        let encoded = entry.encode();
        assert_eq!(encoded.len(), 50);
        assert_eq!(&encoded[..2], &[0u8, ENTRY_TYPE_TRANSACTION]);
        assert_eq!(&encoded[2..34], hash.as_slice());
        assert_eq!(&encoded[34..42], &42u64.to_be_bytes());
        assert_eq!(&encoded[42..46], &0u32.to_be_bytes());
        assert_eq!(&encoded[46..50], &0u32.to_be_bytes());
    }

    #[test]
    fn test_log_address_entry_encoding() {
        let addr = Address::repeat_byte(0x42);
        let entry = IndexEntry::log_address_entry(42, 0, 0, addr);
        let encoded = entry.encode();
        assert_eq!(encoded.len(), 38);
        assert_eq!(&encoded[..2], &[0u8, ENTRY_TYPE_LOG_ADDRESS]);
        assert_eq!(&encoded[2..22], addr.as_slice());
        assert_eq!(&encoded[22..30], &42u64.to_be_bytes());
    }

    #[test]
    fn test_log_topic_entry_encoding() {
        let topic = b256!("ddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef");
        let entry = IndexEntry::log_topic_entry(42, 0, 0, 0, topic);
        let encoded = entry.encode();
        assert_eq!(encoded.len(), 50);
        assert_eq!(&encoded[..2], &[0u8, ENTRY_TYPE_LOG_TOPIC0]);
        assert_eq!(&encoded[2..34], topic.as_slice());
        assert_eq!(&encoded[34..42], &42u64.to_be_bytes());
        assert_eq!(&encoded[42..46], &0u32.to_be_bytes());
        assert_eq!(&encoded[46..50], &0u32.to_be_bytes());
    }

    #[test]
    fn test_lexicographic_ordering() {
        let e1 = IndexEntry::block_entry(40, B256::repeat_byte(0x01));
        let e2 = IndexEntry::block_entry(40, B256::repeat_byte(0x02));
        assert!(e1 < e2);

        let e3 = IndexEntry::transaction_entry(40, 0, B256::repeat_byte(0x01), 0);
        assert!(e3 > e2);
    }

    #[test]
    fn test_topic_index_mapping() {
        let topic = B256::repeat_byte(0xaa);
        let e0 = IndexEntry::log_topic_entry(1, 0, 0, 0, topic);
        let e1 = IndexEntry::log_topic_entry(1, 0, 0, 1, topic);
        let e2 = IndexEntry::log_topic_entry(1, 0, 0, 2, topic);
        let e3 = IndexEntry::log_topic_entry(1, 0, 0, 3, topic);
        assert_eq!(e0.entry_type, ENTRY_TYPE_LOG_TOPIC0);
        assert_eq!(e1.entry_type, ENTRY_TYPE_LOG_TOPIC1);
        assert_eq!(e2.entry_type, ENTRY_TYPE_LOG_TOPIC2);
        assert_eq!(e3.entry_type, ENTRY_TYPE_LOG_TOPIC3);
    }
}
