use alloy_primitives::{Bytes, B256};

use crate::entry::IndexEntry;

/// Build sorted index entries from a block's transactions and receipts.
///
/// `tx_hashes` provides the hash for each transaction in order.
/// `receipt_logs` provides (address, topics) for each log in each receipt.
pub fn build_entries_from_block_data(
    block_number: u64,
    tx_hashes: &[B256],
    receipt_logs: &[Vec<(alloy_primitives::Address, Vec<B256>)>],
) -> Vec<IndexEntry> {
    assert_eq!(tx_hashes.len(), receipt_logs.len());

    let mut entries = Vec::new();
    let mut cumulative_log_count = 0u32;

    for (tx_idx, (tx_hash, logs)) in tx_hashes.iter().zip(receipt_logs.iter()).enumerate() {
        entries.push(IndexEntry::transaction_entry(
            block_number,
            tx_idx as u32,
            *tx_hash,
            cumulative_log_count,
        ));

        for (log_idx, (address, topics)) in logs.iter().enumerate() {
            entries.push(IndexEntry::log_address_entry(
                block_number,
                tx_idx as u32,
                log_idx as u32,
                *address,
            ));

            let topic_count = (topics.len()).min(4);
            for (topic_idx, topic) in topics.iter().take(topic_count).enumerate() {
                entries.push(IndexEntry::log_topic_entry(
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

/// Build sorted index entries for a block, including the delayed parent block entry.
pub fn build_entries_for_block(
    block_number: u64,
    parent_hash: B256,
    tx_hashes: &[B256],
    receipt_logs: &[Vec<(alloy_primitives::Address, Vec<B256>)>],
) -> Vec<IndexEntry> {
    let mut entries = build_entries_from_block_data(block_number, tx_hashes, receipt_logs);

    if block_number > 0 {
        entries.push(IndexEntry::block_entry(block_number - 1, parent_hash));
    }

    entries.sort();
    entries
}

/// Encode the calldata for calling `set(first_block, table_size, table_root)` on the index
/// contract.
///
/// Each parameter is encoded as 32 bytes (big-endian, left-padded for u64 values).
pub fn encode_set_calldata(first_block: u64, table_size: u64, table_root: B256) -> Bytes {
    let mut calldata = Vec::with_capacity(96);
    calldata.extend_from_slice(&u64_to_be32(first_block));
    calldata.extend_from_slice(&u64_to_be32(table_size));
    calldata.extend_from_slice(table_root.as_slice());
    Bytes::from(calldata)
}

/// Encode the calldata for calling `get(first_block, table_size)` on the index contract.
///
/// Each parameter is encoded as 32 bytes (big-endian, left-padded for u64 values).
pub fn encode_get_calldata(first_block: u64, table_size: u64) -> Bytes {
    let mut calldata = Vec::with_capacity(64);
    calldata.extend_from_slice(&u64_to_be32(first_block));
    calldata.extend_from_slice(&u64_to_be32(table_size));
    Bytes::from(calldata)
}

fn u64_to_be32(value: u64) -> [u8; 32] {
    let mut buf = [0u8; 32];
    buf[24..32].copy_from_slice(&value.to_be_bytes());
    buf
}
