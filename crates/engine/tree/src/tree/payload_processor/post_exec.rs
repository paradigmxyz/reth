//! Per-block post-execution handle for background post-execution artifact computation.
//!
//! This module provides [`PostExecHandle`], a block-scoped facade that coordinates
//! background tasks:
//!
//! 1. **Receipt root worker** — spawned at construction via [`Runtime::spawn_blocking_named`].
//!    Receipts are streamed incrementally during execution; when the channel closes the worker
//!    finalizes the receipt trie root and aggregated bloom.
//!
//! 2. **Hashed post-state task** — spawned by [`PostExecHandle::finish`] so it starts immediately
//!    after execution, running in parallel with receipt-root finalization.
//!
//! 3. **Transaction root task** — spawned by [`PostExecHandle::finish`] for payload blocks,
//!    computing the transaction trie root in parallel with the other tasks.
//!
//! Results are accessed via blocking accessors that wait for the background tasks to complete.

use alloy_eips::Encodable2718;
use alloy_primitives::{Bloom, B256};
use crossbeam_channel::Sender as CrossbeamSender;
use reth_primitives_traits::Receipt;
use reth_tasks::{LazyHandle, Runtime};
use reth_trie::HashedPostState;
use reth_trie_common::ordered_root::OrderedTrieRootEncodedBuilder;
use std::sync::{Arc, OnceLock};
use tracing::error;

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

/// Block-scoped handle for post-execution background tasks.
///
/// Created once per block via [`PostExecHandle::new`], which immediately spawns a
/// receipt-root background worker. During transaction execution, receipts are streamed
/// via [`push_receipt`](Self::push_receipt). After execution completes, call
/// [`finish`](Self::finish) to close the receipt channel and spawn hashed-post-state
/// and (optionally) transaction-root computation in parallel.
#[must_use]
pub struct PostExecHandle<R> {
    tx: Option<CrossbeamSender<IndexedReceipt<R>>>,
    receipt_root_bloom: Arc<OnceLock<Option<(B256, Bloom)>>>,
    hashed_post_state: Option<LazyHandle<HashedPostState>>,
    transaction_root: Option<LazyHandle<B256>>,
    executor: Runtime,
}

impl<R> core::fmt::Debug for PostExecHandle<R> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("PostExecHandle").field("finished", &self.tx.is_none()).finish()
    }
}

impl<R: Receipt + 'static> PostExecHandle<R> {
    /// Creates a new handle and immediately spawns the receipt-root background worker.
    ///
    /// The worker begins waiting for receipts via the crossbeam channel and builds the
    /// receipt trie incrementally as they arrive. When the channel closes (via
    /// [`finish`](Self::finish) or handle drop), the worker finalizes the receipt root.
    pub fn new(executor: &Runtime, receipts_len: usize) -> Self {
        let (tx, rx) = crossbeam_channel::unbounded();
        let receipt_root_bloom = Arc::new(OnceLock::new());
        let receipt_root_bloom_worker = receipt_root_bloom.clone();

        // Use spawn_blocking_named for consistent thread naming; ignore the LazyHandle<()> return.
        let _ = executor.spawn_blocking_named("receipt-root", move || {
            run_receipt_root_worker(rx, receipt_root_bloom_worker, receipts_len);
        });

        Self {
            tx: Some(tx),
            receipt_root_bloom,
            hashed_post_state: None,
            transaction_root: None,
            executor: executor.clone(),
        }
    }

    /// Streams one receipt to the background worker.
    #[inline]
    pub fn push_receipt(&self, index: usize, receipt: R) {
        if let Some(tx) = self.tx.as_ref() &&
            tx.send(IndexedReceipt::new(index, receipt)).is_err()
        {
            error!(
                target: "engine::tree::payload_processor",
                index,
                "receipt-root worker dropped before receipt event",
            );
        }
    }

    /// Closes the receipt channel and spawns hashed-post-state and (optionally)
    /// transaction-root computation.
    ///
    /// Dropping the channel sender signals the receipt-root worker to finalize.
    /// The hashed-post-state and transaction-root closures are each spawned on
    /// separate threads, running in parallel with receipt-root finalization.
    ///
    /// Pass `None` for `tx_root_fn` when the block is not a payload (no tx root needed).
    ///
    /// Must be called after all receipts have been pushed.
    pub fn finish(
        &mut self,
        hashed_state_fn: impl FnOnce() -> HashedPostState + Send + 'static,
        tx_root_fn: Option<impl FnOnce() -> B256 + Send + 'static>,
    ) {
        // Drop receipt channel sender — signals worker to finalize receipt root.
        self.tx.take();

        // Spawn hashed-post-state computation immediately on a separate thread.
        self.hashed_post_state =
            Some(self.executor.spawn_blocking_named("hash-post-state", hashed_state_fn));

        // Spawn transaction-root computation if this is a payload block.
        self.transaction_root =
            tx_root_fn.map(|f| self.executor.spawn_blocking_named("payload-tx-root", f));
    }

    /// Returns the computed receipt root and aggregated logs bloom.
    ///
    /// Blocks until the receipt-root worker completes. Returns `None` if the receipt
    /// stream was incomplete (e.g., execution was aborted).
    pub fn receipt_root_bloom(&self) -> Option<(B256, Bloom)> {
        *self.receipt_root_bloom.wait()
    }

    /// Returns the computed transaction root, if this was a payload block.
    ///
    /// Blocks until the transaction-root task completes. Returns `None` for non-payload
    /// blocks where tx root computation was not requested.
    pub fn transaction_root(&self) -> Option<B256> {
        self.transaction_root.as_ref().map(|h| *h.get())
    }

    /// Returns a reference to the computed hashed post state.
    ///
    /// Blocks until the background task completes.
    ///
    /// # Panics
    ///
    /// Panics if [`finish`](Self::finish) was not called before this method.
    pub fn hashed_post_state(&self) -> &HashedPostState {
        self.hashed_post_state
            .as_ref()
            .expect("finish() must be called before hashed_post_state()")
            .get()
    }

    /// Extracts the [`LazyHandle<HashedPostState>`] from this handle.
    ///
    /// # Panics
    ///
    /// Panics if [`finish`](Self::finish) was not called.
    pub fn into_lazy_hashed_state(&mut self) -> LazyHandle<HashedPostState> {
        self.hashed_post_state
            .take()
            .expect("finish() must be called before into_lazy_hashed_state()")
    }
}

impl<R> Drop for PostExecHandle<R> {
    fn drop(&mut self) {
        // Drop the channel sender if finish() was never called, so the receipt-root
        // worker can observe channel closure and terminate.
        self.tx.take();
    }
}

/// Runs the receipt-root background worker.
///
/// Receives indexed receipts from the channel, incrementally builds the receipt trie,
/// and aggregates the logs bloom. When the channel closes, it finalizes the root and
/// stores the result in the shared [`OnceLock`].
fn run_receipt_root_worker<R: Receipt>(
    rx: crossbeam_channel::Receiver<IndexedReceipt<R>>,
    receipt_root_bloom: Arc<OnceLock<Option<(B256, Bloom)>>>,
    receipts_len: usize,
) {
    // RAII guard ensures the OnceLock is set to None if we return early / panic.
    struct AbortGuard<'a> {
        lock: &'a OnceLock<Option<(B256, Bloom)>>,
        disarmed: bool,
    }
    impl Drop for AbortGuard<'_> {
        fn drop(&mut self) {
            if !self.disarmed {
                let _ = self.lock.set(None);
            }
        }
    }

    let mut guard = AbortGuard { lock: &receipt_root_bloom, disarmed: false };
    let mut builder = OrderedTrieRootEncodedBuilder::new(receipts_len);
    let mut aggregated_bloom = Bloom::ZERO;
    let mut encode_buf = Vec::new();
    let mut received_count = 0usize;

    for indexed_receipt in &rx {
        let receipt_with_bloom = indexed_receipt.receipt.with_bloom_ref();

        encode_buf.clear();
        receipt_with_bloom.encode_2718(&mut encode_buf);

        match builder.push(indexed_receipt.index, &encode_buf) {
            Ok(()) => {
                received_count += 1;
                aggregated_bloom |= *receipt_with_bloom.bloom_ref();
            }
            Err(err) => {
                error!(
                    target: "engine::tree::payload_processor",
                    index = indexed_receipt.index,
                    ?err,
                    "Receipt root worker received invalid receipt index, skipping"
                );
            }
        }
    }

    // Finalize receipt root.
    match builder.finalize() {
        Ok(root) => {
            let _ = receipt_root_bloom.set(Some((root, aggregated_bloom)));
        }
        Err(_) => {
            error!(
                target: "engine::tree::payload_processor",
                expected = receipts_len,
                received = received_count,
                "Receipt-root worker received incomplete receipts, execution likely aborted"
            );
            let _ = receipt_root_bloom.set(None);
            return;
        }
    }

    guard.disarmed = true;
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::{proofs::calculate_receipt_root, TxReceipt};
    use alloy_primitives::{Address, Bytes, Log, B256};
    use reth_ethereum_primitives::{Receipt, TxType};

    fn test_runtime() -> Runtime {
        Runtime::test()
    }

    fn sample_receipts() -> Vec<Receipt> {
        vec![
            Receipt {
                tx_type: TxType::Legacy,
                cumulative_gas_used: 21_000,
                success: true,
                logs: vec![],
            },
            Receipt {
                tx_type: TxType::Eip1559,
                cumulative_gas_used: 42_000,
                success: true,
                logs: vec![Log {
                    address: Address::ZERO,
                    data: alloy_primitives::LogData::new_unchecked(vec![B256::ZERO], Bytes::new()),
                }],
            },
            Receipt {
                tx_type: TxType::Eip2930,
                cumulative_gas_used: 63_000,
                success: false,
                logs: vec![],
            },
        ]
    }

    fn expected_root_bloom(receipts: &[Receipt]) -> (B256, Bloom) {
        let receipts_with_bloom: Vec<_> = receipts.iter().map(|r| r.with_bloom_ref()).collect();
        let root = calculate_receipt_root(&receipts_with_bloom);
        let bloom =
            receipts_with_bloom.iter().fold(Bloom::ZERO, |acc, receipt| acc | *receipt.bloom_ref());
        (root, bloom)
    }

    #[test]
    fn post_exec_handle_computes_receipt_root_and_bloom() {
        let rt = test_runtime();

        let receipts = sample_receipts();
        let (expected_root, expected_bloom) = expected_root_bloom(&receipts);

        let mut handle = PostExecHandle::<Receipt>::new(&rt, receipts.len());
        for (index, receipt) in receipts.into_iter().enumerate() {
            handle.push_receipt(index, receipt);
        }
        handle.finish(HashedPostState::default, None::<fn() -> B256>);

        let (root, bloom) = handle.receipt_root_bloom().unwrap();
        assert_eq!(root, expected_root);
        assert_eq!(bloom, expected_bloom);
    }

    #[test]
    fn post_exec_handle_handles_out_of_order_receipts() {
        let rt = test_runtime();

        let receipts = sample_receipts();
        let (expected_root, expected_bloom) = expected_root_bloom(&receipts);

        let mut handle = PostExecHandle::<Receipt>::new(&rt, receipts.len());
        for (index, receipt) in receipts.into_iter().enumerate().rev() {
            handle.push_receipt(index, receipt);
        }
        handle.finish(HashedPostState::default, None::<fn() -> B256>);

        let (root, bloom) = handle.receipt_root_bloom().unwrap();
        assert_eq!(root, expected_root);
        assert_eq!(bloom, expected_bloom);
    }

    #[test]
    fn post_exec_handle_ignores_invalid_index_for_bloom_aggregation() {
        let rt = test_runtime();

        let valid = Receipt::default();
        let invalid = Receipt {
            tx_type: TxType::Legacy,
            cumulative_gas_used: 21_000,
            success: true,
            logs: vec![Log {
                address: Address::ZERO,
                data: alloy_primitives::LogData::new_unchecked(vec![B256::ZERO], Bytes::new()),
            }],
        };

        let expected = expected_root_bloom(core::slice::from_ref(&valid));

        let mut handle = PostExecHandle::<Receipt>::new(&rt, 1);
        handle.push_receipt(0, valid);
        handle.push_receipt(999, invalid);
        handle.finish(HashedPostState::default, None::<fn() -> B256>);

        assert_eq!(handle.receipt_root_bloom(), Some(expected));
    }

    #[test]
    fn post_exec_handle_returns_none_for_incomplete_stream() {
        let rt = test_runtime();

        let mut handle = PostExecHandle::<Receipt>::new(&rt, 2);
        handle.push_receipt(0, Receipt::default());
        // Finish with only 1 of 2 receipts — root should be None.
        handle.finish(HashedPostState::default, None::<fn() -> B256>);

        assert!(handle.receipt_root_bloom().is_none());
    }

    #[test]
    fn post_exec_handle_with_hashed_post_state() {
        let rt = test_runtime();

        let mut handle = PostExecHandle::<Receipt>::new(&rt, 0);
        let expected = HashedPostState::default();
        handle.finish(HashedPostState::default, None::<fn() -> B256>);

        assert_eq!(handle.hashed_post_state(), &expected);
    }

    #[test]
    fn post_exec_handle_with_transaction_root() {
        let rt = test_runtime();

        let expected_root = B256::repeat_byte(0x42);
        let mut handle = PostExecHandle::<Receipt>::new(&rt, 0);
        handle.finish(HashedPostState::default, Some(move || expected_root));

        assert_eq!(handle.transaction_root(), Some(expected_root));
    }

    #[test]
    fn post_exec_handle_without_transaction_root() {
        let rt = test_runtime();

        let mut handle = PostExecHandle::<Receipt>::new(&rt, 0);
        handle.finish(HashedPostState::default, None::<fn() -> B256>);

        assert_eq!(handle.transaction_root(), None);
    }

    #[test]
    fn post_exec_handle_parallel_blocks() {
        let rt = test_runtime();

        let receipts_a = sample_receipts();
        let (expected_root_a, expected_bloom_a) = expected_root_bloom(&receipts_a);

        let receipts_b = vec![Receipt::default(); 2];
        let (expected_root_b, expected_bloom_b) = expected_root_bloom(&receipts_b);

        let mut handle_a = PostExecHandle::<Receipt>::new(&rt, receipts_a.len());
        let mut handle_b = PostExecHandle::<Receipt>::new(&rt, receipts_b.len());

        for (index, receipt) in receipts_a.into_iter().enumerate() {
            handle_a.push_receipt(index, receipt);
        }
        for (index, receipt) in receipts_b.into_iter().enumerate() {
            handle_b.push_receipt(index, receipt);
        }

        handle_a.finish(HashedPostState::default, None::<fn() -> B256>);
        handle_b.finish(HashedPostState::default, None::<fn() -> B256>);

        let (root_a, bloom_a) = handle_a.receipt_root_bloom().unwrap();
        let (root_b, bloom_b) = handle_b.receipt_root_bloom().unwrap();

        assert_eq!(root_a, expected_root_a);
        assert_eq!(bloom_a, expected_bloom_a);
        assert_eq!(root_b, expected_root_b);
        assert_eq!(bloom_b, expected_bloom_b);
    }

    #[test]
    fn post_exec_handle_aborted_block_then_next_succeeds() {
        let rt = test_runtime();

        // First block: aborted (dropped without finishing all receipts)
        let handle = PostExecHandle::<Receipt>::new(&rt, 2);
        handle.push_receipt(0, Receipt::default());
        drop(handle);

        // Second block: succeeds
        let mut handle = PostExecHandle::<Receipt>::new(&rt, 1);
        handle.push_receipt(0, Receipt::default());
        handle.finish(HashedPostState::default, None::<fn() -> B256>);
        assert!(handle.receipt_root_bloom().is_some());
    }

    #[test]
    fn lazy_hashed_post_state_get_and_try_into_inner() {
        let rt = test_runtime();

        let mut handle = PostExecHandle::<Receipt>::new(&rt, 0);
        handle.finish(HashedPostState::default, None::<fn() -> B256>);

        let lazy = handle.into_lazy_hashed_state();
        // handle is partially consumed but Drop is safe (hashed_post_state is now None)
        drop(handle);
        assert_eq!(lazy.get(), &HashedPostState::default());

        let inner = lazy.try_into_inner().unwrap();
        assert_eq!(inner, HashedPostState::default());
    }

    #[test]
    fn lazy_hashed_post_state_clone_prevents_try_into_inner() {
        let rt = test_runtime();

        let mut handle = PostExecHandle::<Receipt>::new(&rt, 0);
        handle.finish(HashedPostState::default, None::<fn() -> B256>);

        let lazy = handle.into_lazy_hashed_state();
        drop(handle);
        let _clone = lazy.clone();

        // try_into_inner fails because there are multiple Arc references.
        let lazy = lazy.try_into_inner().unwrap_err();
        assert_eq!(lazy.get(), &HashedPostState::default());
    }
}
