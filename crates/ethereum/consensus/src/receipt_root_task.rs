//! Receipt root computation in a background task.
//!
//! This module provides a streaming receipt root builder that computes the receipt trie root
//! in a background thread. Receipts are sent via a channel with their index, and for each
//! receipt received, the builder incrementally flushes leaves to the [`HashBuilder`] when
//! possible. When the channel closes, the task returns the computed root.

#[cfg(feature = "std")]
mod task {
    use alloy_eips::Encodable2718;
    use alloy_primitives::{Bloom, B256};
    use crossbeam_channel::{bounded, Receiver, Sender};
    use reth_primitives_traits::Receipt;
    use reth_trie_common::ordered_root::OrderedTrieRootEncodedBuilder;
    use std::thread::{self, JoinHandle};

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

    /// Handle returned when spawning a receipt root task.
    ///
    /// Use [`Self::sender`] to get a sender for streaming receipts to the background task.
    /// When all senders are dropped, call [`Self::wait`] to get the computed root.
    #[derive(Debug)]
    pub struct ReceiptRootTaskHandle<R> {
        /// Sender for streaming receipts to the background task.
        sender: Sender<IndexedReceipt<R>>,
        /// Handle to the background thread computing the receipt root.
        handle: JoinHandle<(B256, Bloom)>,
    }

    impl<R: Send + 'static> ReceiptRootTaskHandle<R> {
        /// Returns a clone of the sender for streaming receipts to the task.
        pub fn sender(&self) -> Sender<IndexedReceipt<R>> {
            self.sender.clone()
        }

        /// Waits for the task to complete and returns the computed receipt root and logs bloom.
        ///
        /// This should be called after all receipts have been sent and the sender has been dropped.
        pub fn wait(self) -> (B256, Bloom) {
            drop(self.sender);
            self.handle.join().expect("receipt root task panicked")
        }
    }

    /// Spawns a background thread that computes the receipt trie root from receipts received
    /// via channel.
    ///
    /// The task uses an [`OrderedTrieRootEncodedBuilder`] to incrementally build the receipt
    /// trie root. Items can be pushed in any order by index, and the builder flushes leaves
    /// to the underlying [`HashBuilder`] as they become available in the correct RLP key order.
    ///
    /// Receipts are encoded in the background thread, moving the encoding work off the main
    /// execution thread.
    ///
    /// # Arguments
    ///
    /// * `receipts_len` - The total number of receipts expected. This is needed to correctly order
    ///   the trie keys according to RLP encoding rules.
    /// * `channel_capacity` - The capacity of the bounded channel for receiving receipts.
    ///
    /// # Returns
    ///
    /// A handle that provides a sender for streaming receipts and can be waited on for the result.
    pub fn spawn_receipt_root_task<R>(
        receipts_len: usize,
        channel_capacity: usize,
    ) -> ReceiptRootTaskHandle<R>
    where
        R: Receipt + Send + 'static,
    {
        let (tx, rx): (Sender<IndexedReceipt<R>>, Receiver<IndexedReceipt<R>>) =
            bounded(channel_capacity);

        let handle = thread::spawn(move || compute_receipt_root(rx, receipts_len));

        ReceiptRootTaskHandle { sender: tx, handle }
    }

    /// Computes the receipt root by receiving indexed receipts from a channel, encoding them,
    /// and building the trie incrementally using [`OrderedTrieRootEncodedBuilder`].
    fn compute_receipt_root<R: Receipt>(
        rx: Receiver<IndexedReceipt<R>>,
        receipts_len: usize,
    ) -> (B256, Bloom) {
        let mut builder = OrderedTrieRootEncodedBuilder::new(receipts_len);
        let mut aggregated_bloom = Bloom::ZERO;
        let mut encode_buf = Vec::new();

        for indexed_receipt in rx {
            let receipt_with_bloom = indexed_receipt.receipt.with_bloom_ref();

            encode_buf.clear();
            receipt_with_bloom.encode_2718(&mut encode_buf);

            aggregated_bloom |= *receipt_with_bloom.bloom_ref();
            builder.push_unchecked(indexed_receipt.index, &encode_buf);
        }

        let root = builder.finalize().expect("receipt root builder incomplete");
        (root, aggregated_bloom)
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use alloy_primitives::{b256, hex};
        use reth_ethereum_primitives::Receipt;

        #[test]
        fn test_receipt_root_task_empty() {
            let handle = spawn_receipt_root_task::<Receipt>(0, 1);
            let (root, bloom) = handle.wait();

            // Empty trie root
            assert_eq!(root, reth_trie_common::EMPTY_ROOT_HASH);
            assert_eq!(bloom, Bloom::ZERO);
        }

        #[test]
        fn test_receipt_root_task_single_receipt() {
            let receipts: Vec<Receipt> = vec![Receipt::default()];

            let handle = spawn_receipt_root_task(receipts.len(), 1);
            let sender = handle.sender();

            for (i, receipt) in receipts.clone().into_iter().enumerate() {
                sender.send(IndexedReceipt::new(i, receipt)).unwrap();
            }
            drop(sender);

            let (root, _bloom) = handle.wait();

            // Verify against the standard calculation
            use alloy_consensus::{proofs::calculate_receipt_root, TxReceipt};
            let receipts_with_bloom: Vec<_> = receipts.iter().map(|r| r.with_bloom_ref()).collect();
            let expected_root = calculate_receipt_root(&receipts_with_bloom);

            assert_eq!(root, expected_root);
        }

        #[test]
        fn test_receipt_root_task_multiple_receipts() {
            let receipts: Vec<Receipt> = vec![Receipt::default(); 5];

            let handle = spawn_receipt_root_task(receipts.len(), 4);
            let sender = handle.sender();

            for (i, receipt) in receipts.into_iter().enumerate() {
                sender.send(IndexedReceipt::new(i, receipt)).unwrap();
            }
            drop(sender);

            let (root, bloom) = handle.wait();

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

        #[test]
        fn test_receipt_root_matches_standard_calculation() {
            use alloy_consensus::{proofs::calculate_receipt_root, TxReceipt};
            use alloy_primitives::{Address, Bytes, Log};
            use reth_ethereum_primitives::{Receipt, TxType};

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
                        data: alloy_primitives::LogData::new_unchecked(
                            vec![B256::ZERO],
                            Bytes::new(),
                        ),
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
            let handle = spawn_receipt_root_task(receipts.len(), 4);
            let sender = handle.sender();
            for (i, receipt) in receipts.into_iter().enumerate() {
                sender.send(IndexedReceipt::new(i, receipt)).unwrap();
            }
            drop(sender);
            let (task_root, task_bloom) = handle.wait();

            assert_eq!(task_root, expected_root);
            assert_eq!(task_bloom, expected_bloom);
        }

        #[test]
        fn test_receipt_root_task_out_of_order() {
            use alloy_consensus::{proofs::calculate_receipt_root, TxReceipt};

            let receipts: Vec<Receipt> = vec![Receipt::default(); 5];

            // Calculate expected values first (before we move receipts)
            let receipts_with_bloom: Vec<_> = receipts.iter().map(|r| r.with_bloom_ref()).collect();
            let expected_root = calculate_receipt_root(&receipts_with_bloom);

            let handle = spawn_receipt_root_task(receipts.len(), 4);
            let sender = handle.sender();

            // Send in reverse order to test out-of-order handling
            for (i, receipt) in receipts.into_iter().enumerate().rev() {
                sender.send(IndexedReceipt::new(i, receipt)).unwrap();
            }
            drop(sender);

            let (root, _bloom) = handle.wait();

            assert_eq!(root, expected_root);
        }
    }
}

#[cfg(feature = "std")]
pub use task::*;
