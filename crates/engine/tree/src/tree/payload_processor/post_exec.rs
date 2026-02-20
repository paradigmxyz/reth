//! Long-lived post-execution worker for incremental receipt-root computation.
//!
//! This keeps the lifecycle simple: one worker thread per coordinator, sequentially
//! handling blocks by reusing the existing `ReceiptRootTaskHandle`.

use super::receipt_root_task::{IndexedReceipt, ReceiptRootTaskHandle};
use alloy_primitives::{Bloom, B256};
use crossbeam_channel::{self, Receiver as CrossbeamReceiver, Sender as CrossbeamSender};
use reth_primitives_traits::Receipt;
use std::{
    sync::mpsc::{self, Receiver, Sender},
    thread::JoinHandle,
};
use tokio::sync::oneshot;
use tracing::error;

/// Handle to the long-lived post-execution worker.
pub struct PostExecCoordinator<R> {
    commands_tx: Sender<WorkerCommand<R>>,
    worker: Option<JoinHandle<()>>,
}

impl<R> core::fmt::Debug for PostExecCoordinator<R> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("PostExecCoordinator").finish()
    }
}

impl<R: Receipt + Send + 'static> PostExecCoordinator<R> {
    /// Create a new coordinator and its worker thread.
    pub fn new() -> Self {
        let (commands_tx, commands_rx) = mpsc::channel();
        let worker = std::thread::Builder::new()
            .name("post-exec".to_string())
            .spawn(move || PostExecWorker::run(commands_rx))
            .expect("failed to spawn post-exec worker");

        Self { commands_tx, worker: Some(worker) }
    }

    /// Begin receipt-root streaming for a new block.
    pub fn begin_block(&self, receipts_len: usize) -> PostExecBlockHandle<R> {
        let (receipt_tx, receipt_rx) = crossbeam_channel::unbounded();
        let (result_tx, result_rx) = oneshot::channel();
        if self
            .commands_tx
            .send(WorkerCommand::RunBlock { receipts_len, receipt_rx, result_tx })
            .is_err()
        {
            error!(
                target: "engine::tree::payload_processor",
                "post-exec worker dropped before RunBlock command",
            );
        }

        PostExecBlockHandle { receipt_tx: Some(receipt_tx), result_rx: Some(result_rx) }
    }
}

impl<R> Drop for PostExecCoordinator<R> {
    fn drop(&mut self) {
        let _ = self.commands_tx.send(WorkerCommand::Shutdown);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

/// Block-scoped stream handle used by execution to send receipts and finalize.
pub struct PostExecBlockHandle<R> {
    receipt_tx: Option<CrossbeamSender<IndexedReceipt<R>>>,
    result_rx: Option<oneshot::Receiver<(B256, Bloom)>>,
}

impl<R> core::fmt::Debug for PostExecBlockHandle<R> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("PostExecBlockHandle").finish()
    }
}

impl<R: Receipt + Send + 'static> PostExecBlockHandle<R> {
    /// Stream one receipt to the background worker.
    pub fn push_receipt(&self, index: usize, receipt: R) {
        if self
            .receipt_tx
            .as_ref()
            .is_some_and(|tx| tx.send(IndexedReceipt::new(index, receipt)).is_err())
        {
            error!(
                target: "engine::tree::payload_processor",
                index,
                "post-exec worker dropped before Receipt event",
            );
        }
    }

    /// Finalize this block and return a receiver for the computed root+bloom.
    pub fn finalize(mut self) -> oneshot::Receiver<(B256, Bloom)> {
        // Close the stream to signal end-of-block to the worker.
        self.receipt_tx.take();
        self.result_rx.take().expect("result receiver must be present")
    }
}

enum WorkerCommand<R> {
    RunBlock {
        receipts_len: usize,
        receipt_rx: CrossbeamReceiver<IndexedReceipt<R>>,
        result_tx: oneshot::Sender<(B256, Bloom)>,
    },
    Shutdown,
}

struct PostExecWorker;

impl PostExecWorker {
    fn run<R: Receipt>(commands_rx: Receiver<WorkerCommand<R>>) {
        while let Ok(command) = commands_rx.recv() {
            match command {
                WorkerCommand::RunBlock { receipts_len, receipt_rx, result_tx } => {
                    ReceiptRootTaskHandle::new(receipt_rx, result_tx).run(receipts_len);
                }
                WorkerCommand::Shutdown => break,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::{proofs::calculate_receipt_root, TxReceipt};
    use alloy_primitives::{Address, Bytes, Log, B256};
    use reth_ethereum_primitives::{Receipt, TxType};

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

    #[tokio::test]
    async fn post_exec_worker_computes_receipt_root_and_bloom() {
        let coordinator = PostExecCoordinator::<Receipt>::new();

        let receipts = sample_receipts();
        let (expected_root, expected_bloom) = expected_root_bloom(&receipts);

        let handle = coordinator.begin_block(receipts.len());
        for (index, receipt) in receipts.into_iter().enumerate() {
            handle.push_receipt(index, receipt);
        }

        let (root, bloom) = handle.finalize().await.unwrap();
        assert_eq!(root, expected_root);
        assert_eq!(bloom, expected_bloom);
    }

    #[tokio::test]
    async fn post_exec_worker_handles_out_of_order_receipts() {
        let coordinator = PostExecCoordinator::<Receipt>::new();

        let receipts = sample_receipts();
        let (expected_root, expected_bloom) = expected_root_bloom(&receipts);

        let handle = coordinator.begin_block(receipts.len());
        for (index, receipt) in receipts.into_iter().enumerate().rev() {
            handle.push_receipt(index, receipt);
        }

        let (root, bloom) = handle.finalize().await.unwrap();
        assert_eq!(root, expected_root);
        assert_eq!(bloom, expected_bloom);
    }

    #[tokio::test]
    async fn post_exec_worker_drops_duplicate_index_blocks() {
        let coordinator = PostExecCoordinator::<Receipt>::new();

        let mut receipts = sample_receipts();
        let first = receipts.remove(0);
        let second = receipts.remove(0);

        let handle = coordinator.begin_block(2);
        handle.push_receipt(0, first.clone());
        handle.push_receipt(0, second);

        assert!(handle.finalize().await.is_err());
    }

    #[tokio::test]
    async fn post_exec_worker_supports_multiple_queued_blocks() {
        let coordinator = PostExecCoordinator::<Receipt>::new();

        let receipts_a = sample_receipts();
        let (expected_root_a, expected_bloom_a) = expected_root_bloom(&receipts_a);

        let receipts_b = vec![Receipt::default(); 2];
        let (expected_root_b, expected_bloom_b) = expected_root_bloom(&receipts_b);

        let handle_a = coordinator.begin_block(receipts_a.len());
        let handle_b = coordinator.begin_block(receipts_b.len());

        handle_a.push_receipt(0, receipts_a[0].clone());
        handle_b.push_receipt(0, receipts_b[0].clone());
        handle_a.push_receipt(1, receipts_a[1].clone());
        handle_b.push_receipt(1, receipts_b[1].clone());
        handle_a.push_receipt(2, receipts_a[2].clone());

        let (root_a, bloom_a) = handle_a.finalize().await.unwrap();
        let (root_b, bloom_b) = handle_b.finalize().await.unwrap();

        assert_eq!(root_a, expected_root_a);
        assert_eq!(bloom_a, expected_bloom_a);
        assert_eq!(root_b, expected_root_b);
        assert_eq!(bloom_b, expected_bloom_b);
    }

    #[tokio::test]
    async fn post_exec_worker_aborts_incomplete_blocks() {
        let coordinator = PostExecCoordinator::<Receipt>::new();

        let handle = coordinator.begin_block(2);
        handle.push_receipt(0, Receipt::default());
        drop(handle);

        let second = coordinator.begin_block(1);
        second.push_receipt(0, Receipt::default());
        assert!(second.finalize().await.is_ok());
    }

    #[tokio::test]
    async fn post_exec_worker_handles_shutdown_after_finalize() {
        let coordinator = PostExecCoordinator::<Receipt>::new();
        let handle = coordinator.begin_block(1);
        handle.push_receipt(0, Receipt::default());
        let _ = handle.finalize().await.unwrap();
        drop(coordinator);
    }
}
