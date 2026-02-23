//! Block-scoped post-execution handle for receipt-root, withdrawals-root and hashed-post-state.
//!
//! This module introduces [`PostExecOutput`], a per-block output container backed by
//! [`OnceLock`](reth_primitives_traits::sync::OnceLock). Each artifact can be awaited
//! independently via `wait`-backed getters.

use super::receipt_root_task::{IndexedReceipt, ReceiptRootTaskHandle};
use alloy_eips::eip4895::Withdrawal;
use alloy_primitives::{Bloom, B256};
use crossbeam_channel::{self, Sender as CrossbeamSender};
use reth_primitives_traits::{proofs::calculate_withdrawals_root, sync::OnceLock, Receipt};
use reth_tasks::Runtime;
use reth_trie::HashedPostState;
use std::{
    panic::{self, AssertUnwindSafe},
    sync::{mpsc, Arc},
};
use tracing::error;

type HashedPostStateJob = Box<dyn FnOnce() -> HashedPostState + Send + 'static>;

/// Block-scoped post-execution outputs.
///
/// Each field is resolved exactly once by the background post-exec task. Callers can wait on
/// individual artifacts as needed.
#[derive(Debug, Default)]
pub struct PostExecOutput {
    receipt_root_bloom: OnceLock<Option<(B256, Bloom)>>,
    withdrawals_root: OnceLock<Option<B256>>,
    hashed_post_state: OnceLock<Option<Arc<HashedPostState>>>,
}

impl PostExecOutput {
    #[inline]
    fn set_receipt_root_bloom(&self, value: Option<(B256, Bloom)>) {
        let _ = self.receipt_root_bloom.set(value);
    }

    #[inline]
    fn set_withdrawals_root(&self, value: Option<B256>) {
        let _ = self.withdrawals_root.set(value);
    }

    #[inline]
    fn set_hashed_post_state(&self, value: Option<Arc<HashedPostState>>) {
        let _ = self.hashed_post_state.set(value);
    }

    /// Waits for the receipt root + logs bloom output.
    pub fn receipt_root_bloom(&self) -> Option<(B256, Bloom)> {
        self.receipt_root_bloom.wait().clone()
    }

    /// Waits for the withdrawals root output.
    pub fn withdrawals_root(&self) -> Option<B256> {
        self.withdrawals_root.wait().clone()
    }

    /// Waits for the hashed post state output.
    pub fn hashed_post_state(&self) -> Option<Arc<HashedPostState>> {
        self.hashed_post_state.wait().clone()
    }
}

/// Block-scoped handle for post-execution background work.
///
/// The handle streams receipts while execution is running, then schedules hashed post-state
/// computation and exposes all outputs through [`PostExecOutput`].
pub struct PostExecHandle<R> {
    receipt_tx: Option<CrossbeamSender<IndexedReceipt<R>>>,
    hashed_post_state_tx: Option<mpsc::Sender<HashedPostStateJob>>,
    output: Arc<PostExecOutput>,
}

impl<R> core::fmt::Debug for PostExecHandle<R> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("PostExecHandle").finish()
    }
}

impl<R: Receipt + 'static> PostExecHandle<R> {
    /// Creates a new block-scoped post-exec handle and spawns the background task.
    pub fn new(
        executor: &Runtime,
        receipts_len: usize,
        withdrawals: Option<Vec<Withdrawal>>,
    ) -> Self {
        let (receipt_tx, receipt_rx) = crossbeam_channel::unbounded();
        let (hashed_post_state_tx, hashed_post_state_rx) = mpsc::channel::<HashedPostStateJob>();
        let output = Arc::new(PostExecOutput::default());
        let output_for_task = Arc::clone(&output);

        executor.spawn_blocking_named("post-exec", move || {
            let run = || {
                output_for_task
                    .set_withdrawals_root(withdrawals.as_deref().map(calculate_withdrawals_root));

                let (result_tx, result_rx) = tokio::sync::oneshot::channel();
                ReceiptRootTaskHandle::new(receipt_rx, result_tx).run(receipts_len);
                output_for_task.set_receipt_root_bloom(result_rx.blocking_recv().ok());

                let hashed_post_state = hashed_post_state_rx
                    .recv()
                    .ok()
                    .and_then(|job| panic::catch_unwind(AssertUnwindSafe(job)).ok().map(Arc::new));
                output_for_task.set_hashed_post_state(hashed_post_state);
            };

            if panic::catch_unwind(AssertUnwindSafe(run)).is_err() {
                error!(
                    target: "engine::tree::payload_processor",
                    "post-exec task panicked, returning empty outputs"
                );
            }

            if output_for_task.withdrawals_root.get().is_none() {
                output_for_task.set_withdrawals_root(None);
            }
            if output_for_task.receipt_root_bloom.get().is_none() {
                output_for_task.set_receipt_root_bloom(None);
            }
            if output_for_task.hashed_post_state.get().is_none() {
                output_for_task.set_hashed_post_state(None);
            }
        });

        Self {
            receipt_tx: Some(receipt_tx),
            hashed_post_state_tx: Some(hashed_post_state_tx),
            output,
        }
    }

    /// Streams one receipt to the background receipt-root task.
    pub fn push_receipt(&self, index: usize, receipt: R) {
        if self
            .receipt_tx
            .as_ref()
            .is_some_and(|tx| tx.send(IndexedReceipt::new(index, receipt)).is_err())
        {
            error!(
                target: "engine::tree::payload_processor",
                index,
                "post-exec task dropped before receipt event",
            );
        }
    }

    /// Finalizes the receipt stream for this block.
    #[inline]
    pub fn finish_receipts(&mut self) {
        self.receipt_tx.take();
    }

    /// Queues hashed post-state computation for this block.
    ///
    /// This should be called exactly once per block after execution output is available.
    pub fn spawn_hashed_post_state(
        &mut self,
        f: impl FnOnce() -> HashedPostState + Send + 'static,
    ) {
        let Some(tx) = self.hashed_post_state_tx.take() else {
            error!(
                target: "engine::tree::payload_processor",
                "hashed-post-state job already queued or handle dropped"
            );
            return;
        };

        if tx.send(Box::new(f)).is_err() {
            error!(
                target: "engine::tree::payload_processor",
                "post-exec task dropped before hashed-post-state job was queued",
            );
        }
    }

    /// Waits for and returns the computed receipt root + logs bloom.
    pub fn receipt_root_bloom(&self) -> Option<(B256, Bloom)> {
        self.output.receipt_root_bloom()
    }

    /// Waits for and returns the computed withdrawals root.
    pub fn withdrawals_root(&self) -> Option<B256> {
        self.output.withdrawals_root()
    }

    /// Waits for and returns the computed hashed post state.
    pub fn hashed_post_state(&self) -> Option<Arc<HashedPostState>> {
        self.output.hashed_post_state()
    }
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

    fn sample_withdrawals() -> Vec<Withdrawal> {
        vec![Withdrawal { index: 1, validator_index: 2, address: Address::ZERO, amount: 3 }]
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

        let mut handle = PostExecHandle::<Receipt>::new(&rt, receipts.len(), None);
        for (index, receipt) in receipts.into_iter().enumerate() {
            handle.push_receipt(index, receipt);
        }
        handle.finish_receipts();
        handle.spawn_hashed_post_state(HashedPostState::default);

        let (root, bloom) = handle.receipt_root_bloom().unwrap();
        assert_eq!(root, expected_root);
        assert_eq!(bloom, expected_bloom);
    }

    #[test]
    fn post_exec_handle_handles_out_of_order_receipts() {
        let rt = test_runtime();

        let receipts = sample_receipts();
        let (expected_root, expected_bloom) = expected_root_bloom(&receipts);

        let mut handle = PostExecHandle::<Receipt>::new(&rt, receipts.len(), None);
        for (index, receipt) in receipts.into_iter().enumerate().rev() {
            handle.push_receipt(index, receipt);
        }
        handle.finish_receipts();
        handle.spawn_hashed_post_state(HashedPostState::default);

        let (root, bloom) = handle.receipt_root_bloom().unwrap();
        assert_eq!(root, expected_root);
        assert_eq!(bloom, expected_bloom);
    }

    #[test]
    fn post_exec_handle_returns_none_for_incomplete_stream() {
        let rt = test_runtime();

        let mut handle = PostExecHandle::<Receipt>::new(&rt, 2, None);
        handle.push_receipt(0, Receipt::default());
        handle.finish_receipts();
        handle.spawn_hashed_post_state(HashedPostState::default);

        assert!(handle.receipt_root_bloom().is_none());
    }

    #[test]
    fn post_exec_handle_waits_for_hashed_state() {
        let rt = test_runtime();

        let mut handle = PostExecHandle::<Receipt>::new(&rt, 0, None);
        handle.finish_receipts();
        handle.spawn_hashed_post_state(HashedPostState::default);

        assert!(handle.hashed_post_state().is_some());
    }

    #[test]
    fn post_exec_handle_computes_withdrawals_root() {
        let rt = test_runtime();

        let withdrawals = sample_withdrawals();
        let expected = Some(calculate_withdrawals_root(&withdrawals));

        let mut handle = PostExecHandle::<Receipt>::new(&rt, 0, Some(withdrawals));
        handle.finish_receipts();
        handle.spawn_hashed_post_state(HashedPostState::default);

        assert_eq!(handle.withdrawals_root(), expected);
    }

    #[test]
    fn post_exec_handle_withdrawals_root_none_without_withdrawals() {
        let rt = test_runtime();

        let mut handle = PostExecHandle::<Receipt>::new(&rt, 0, None);
        handle.finish_receipts();
        handle.spawn_hashed_post_state(HashedPostState::default);

        assert_eq!(handle.withdrawals_root(), None);
    }

    #[test]
    fn post_exec_handle_aborted_block_then_next_succeeds() {
        let rt = test_runtime();

        let handle = PostExecHandle::<Receipt>::new(&rt, 2, None);
        handle.push_receipt(0, Receipt::default());
        drop(handle);

        let mut handle = PostExecHandle::<Receipt>::new(&rt, 1, None);
        handle.push_receipt(0, Receipt::default());
        handle.finish_receipts();
        handle.spawn_hashed_post_state(HashedPostState::default);

        assert!(handle.receipt_root_bloom().is_some());
    }
}
