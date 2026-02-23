//! Receipt root computation in a background task.
//!
//! This module provides a streaming receipt root builder that computes the receipt trie root
//! in a background thread. Receipts are sent via a channel with their index, and for each
//! receipt received, the builder incrementally flushes leaves to the underlying
//! [`OrderedTrieRootEncodedBuilder`] when possible. When the channel closes, the task returns the
//! computed root.

use alloy_eips::Encodable2718;
use alloy_primitives::{Bloom, B256};
use crossbeam_channel::Receiver;
use reth_primitives_traits::Receipt;
use reth_trie_common::ordered_root::OrderedTrieRootEncodedBuilder;

/// Receipt with index, ready to be sent to the background task for encoding and trie building.
#[derive(Debug, Clone)]
pub struct IndexedReceipt<R> {
    /// The transaction index within the block.
    pub index: usize,
    /// The receipt.
    pub receipt: R,
}

impl<R> IndexedReceipt<R> {
    /// Creates a new indexed receipt.
    #[inline]
    pub const fn new(index: usize, receipt: R) -> Self {
        Self { index, receipt }
    }
}

/// Handle for running the receipt root computation in a background task.
///
/// This struct holds the channel needed to receive receipts.
/// Use [`Self::run`] to execute the computation (typically in a spawned blocking task).
#[derive(Debug)]
pub struct ReceiptRootTaskHandle<R> {
    /// Receiver for indexed receipts.
    receipt_rx: Receiver<IndexedReceipt<R>>,
}

impl<R: Receipt> ReceiptRootTaskHandle<R> {
    /// Creates a new handle from the receipt receiver channel.
    pub const fn new(receipt_rx: Receiver<IndexedReceipt<R>>) -> Self {
        Self { receipt_rx }
    }

    /// Runs the receipt root computation, consuming the handle.
    ///
    /// This method receives indexed receipts from the channel, encodes them,
    /// and builds the trie incrementally. When all receipts have been received
    /// (channel closed), it returns the computed root and aggregated bloom.
    ///
    /// This is designed to be called inside a blocking task (e.g., via
    /// `executor.spawn_blocking(move || handle.run(receipts_len))`).
    ///
    /// # Arguments
    ///
    /// * `receipts_len` - The total number of receipts expected. This is needed to correctly order
    ///   the trie keys according to RLP encoding rules.
    pub fn run(self, receipts_len: usize) -> Option<(B256, Bloom)> {
        let mut builder = OrderedTrieRootEncodedBuilder::new(receipts_len);
        let mut aggregated_bloom = Bloom::ZERO;
        let mut encode_buf = Vec::new();
        let mut received_count = 0usize;

        for indexed_receipt in self.receipt_rx {
            let receipt_with_bloom = indexed_receipt.receipt.with_bloom_ref();

            encode_buf.clear();
            receipt_with_bloom.encode_2718(&mut encode_buf);

            aggregated_bloom |= *receipt_with_bloom.bloom_ref();
            match builder.push(indexed_receipt.index, &encode_buf) {
                Ok(()) => {
                    received_count += 1;
                }
                Err(err) => {
                    // If a duplicate or out-of-bounds index is streamed, skip it and
                    // fall back to computing the receipt root from the full receipts
                    // vector later.
                    tracing::error!(
                        target: "engine::tree::payload_processor",
                        index = indexed_receipt.index,
                        ?err,
                        "Receipt root task received invalid receipt index, skipping"
                    );
                }
            }
        }

        let Ok(root) = builder.finalize() else {
            // Finalize fails if we didn't receive exactly `receipts_len` receipts. This can
            // happen if execution was aborted early (e.g., invalid transaction encountered).
            // We return `None`, allowing the caller to handle the abort.
            tracing::error!(
                target: "engine::tree::payload_processor",
                expected = receipts_len,
                received = received_count,
                "Receipt root task received incomplete receipts, execution likely aborted"
            );
            return None;
        };
        Some((root, aggregated_bloom))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::{proofs::calculate_receipt_root, TxReceipt};
    use alloy_primitives::{b256, hex, Address, Bytes, Log};
    use crossbeam_channel::bounded;
    use reth_ethereum_primitives::{Receipt, TxType};

    #[tokio::test]
    async fn test_receipt_root_task_empty() {
        let (_tx, rx) = bounded::<IndexedReceipt<Receipt>>(1);
        drop(_tx);

        let handle = ReceiptRootTaskHandle::new(rx);
        let (root, bloom) =
            tokio::task::spawn_blocking(move || handle.run(0)).await.unwrap().unwrap();

        // Empty trie root
        assert_eq!(root, reth_trie_common::EMPTY_ROOT_HASH);
        assert_eq!(bloom, Bloom::ZERO);
    }

    #[tokio::test]
    async fn test_receipt_root_task_single_receipt() {
        let receipts: Vec<Receipt> = vec![Receipt::default()];

        let (tx, rx) = bounded(1);
        let receipts_len = receipts.len();

        let handle = ReceiptRootTaskHandle::new(rx);
        let join_handle = tokio::task::spawn_blocking(move || handle.run(receipts_len));

        for (i, receipt) in receipts.clone().into_iter().enumerate() {
            tx.send(IndexedReceipt::new(i, receipt)).unwrap();
        }
        drop(tx);

        let (root, _bloom) = join_handle.await.unwrap().unwrap();

        // Verify against the standard calculation
        let receipts_with_bloom: Vec<_> = receipts.iter().map(|r| r.with_bloom_ref()).collect();
        let expected_root = calculate_receipt_root(&receipts_with_bloom);

        assert_eq!(root, expected_root);
    }

    #[tokio::test]
    async fn test_receipt_root_task_multiple_receipts() {
        let receipts: Vec<Receipt> = vec![Receipt::default(); 5];

        let (tx, rx) = bounded(4);
        let receipts_len = receipts.len();

        let handle = ReceiptRootTaskHandle::new(rx);
        let join_handle = tokio::task::spawn_blocking(move || handle.run(receipts_len));

        for (i, receipt) in receipts.into_iter().enumerate() {
            tx.send(IndexedReceipt::new(i, receipt)).unwrap();
        }
        drop(tx);

        let (root, bloom) = join_handle.await.unwrap().unwrap();

        // Verify against expected values from existing test
        assert_eq!(
            root,
            b256!("0x61353b4fb714dc1fccacbf7eafc4273e62f3d1eed716fe41b2a0cd2e12c63ebc")
        );
        assert_eq!(
            bloom,
            Bloom::from(hex!("00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000"))
        );
    }

    #[tokio::test]
    async fn test_receipt_root_matches_standard_calculation() {
        // Create some receipts with actual data
        let receipts = vec![
            Receipt {
                tx_type: TxType::Legacy,
                cumulative_gas_used: 21000,
                success: true,
                logs: vec![],
            },
            Receipt {
                tx_type: TxType::Eip1559,
                cumulative_gas_used: 42000,
                success: true,
                logs: vec![Log {
                    address: Address::ZERO,
                    data: alloy_primitives::LogData::new_unchecked(vec![B256::ZERO], Bytes::new()),
                }],
            },
            Receipt {
                tx_type: TxType::Eip2930,
                cumulative_gas_used: 63000,
                success: false,
                logs: vec![],
            },
        ];

        // Calculate expected values first (before we move receipts)
        let receipts_with_bloom: Vec<_> = receipts.iter().map(|r| r.with_bloom_ref()).collect();
        let expected_root = calculate_receipt_root(&receipts_with_bloom);
        let expected_bloom =
            receipts_with_bloom.iter().fold(Bloom::ZERO, |bloom, r| bloom | r.bloom_ref());

        // Calculate using the task
        let (tx, rx) = bounded(4);
        let receipts_len = receipts.len();

        let handle = ReceiptRootTaskHandle::new(rx);
        let join_handle = tokio::task::spawn_blocking(move || handle.run(receipts_len));

        for (i, receipt) in receipts.into_iter().enumerate() {
            tx.send(IndexedReceipt::new(i, receipt)).unwrap();
        }
        drop(tx);

        let (task_root, task_bloom) = join_handle.await.unwrap().unwrap();

        assert_eq!(task_root, expected_root);
        assert_eq!(task_bloom, expected_bloom);
    }

    #[tokio::test]
    async fn test_receipt_root_task_out_of_order() {
        let receipts: Vec<Receipt> = vec![Receipt::default(); 5];

        // Calculate expected values first (before we move receipts)
        let receipts_with_bloom: Vec<_> = receipts.iter().map(|r| r.with_bloom_ref()).collect();
        let expected_root = calculate_receipt_root(&receipts_with_bloom);

        let (tx, rx) = bounded(4);
        let receipts_len = receipts.len();

        let handle = ReceiptRootTaskHandle::new(rx);
        let join_handle = tokio::task::spawn_blocking(move || handle.run(receipts_len));

        // Send in reverse order to test out-of-order handling
        for (i, receipt) in receipts.into_iter().enumerate().rev() {
            tx.send(IndexedReceipt::new(i, receipt)).unwrap();
        }
        drop(tx);

        let (root, _bloom) = join_handle.await.unwrap().unwrap();

        assert_eq!(root, expected_root);
    }
}
