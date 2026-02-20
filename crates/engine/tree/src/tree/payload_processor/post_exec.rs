//! Long-lived post-execution worker for incremental receipt-root computation.
//!
//! The worker receives per-block events from execution and maintains block-local
//! accumulators keyed by block id. This avoids per-block task spawning overhead.

use super::receipt_root_task::IndexedReceipt;
use alloy_eips::Encodable2718;
use alloy_primitives::{Bloom, B256};
use reth_primitives_traits::Receipt;
use reth_tasks::{LazyHandle, Runtime};
use reth_trie_common::ordered_root::OrderedTrieRootEncodedBuilder;
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc::{self, Receiver, Sender},
    },
};
use tokio::sync::oneshot;
use tracing::{debug, error, warn};

/// Handle to the long-lived post-execution worker.
pub struct PostExecCoordinator<R> {
    events_tx: Sender<PostExecEvent<R>>,
    next_block_id: AtomicU64,
    _worker_handle: LazyHandle<()>,
}

impl<R> core::fmt::Debug for PostExecCoordinator<R> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("PostExecCoordinator")
            .field("next_block_id", &self.next_block_id.load(Ordering::Relaxed))
            .finish()
    }
}

impl<R: Receipt + Send + 'static> PostExecCoordinator<R> {
    /// Create a new worker bound to the given runtime.
    pub fn new(runtime: &Runtime) -> Self {
        let (events_tx, events_rx) = mpsc::channel();
        let worker_handle =
            runtime.spawn_blocking_named("post-exec", move || PostExecWorker::run(events_rx));

        Self { events_tx, next_block_id: AtomicU64::new(1), _worker_handle: worker_handle }
    }

    /// Begin receipt-root streaming for a new block.
    pub fn begin_block(&self, receipts_len: usize) -> PostExecBlockHandle<R> {
        let block_id = self.next_block_id.fetch_add(1, Ordering::Relaxed);
        if self.events_tx.send(PostExecEvent::BeginBlock { block_id, receipts_len }).is_err() {
            error!(
                target: "engine::tree::payload_processor",
                block_id,
                "post-exec worker dropped before BeginBlock event",
            );
        }

        PostExecBlockHandle { block_id, events_tx: self.events_tx.clone(), completed: false }
    }
}

/// Block-scoped stream handle used by execution to send receipts and finalize.
pub struct PostExecBlockHandle<R> {
    block_id: u64,
    events_tx: Sender<PostExecEvent<R>>,
    completed: bool,
}

impl<R> core::fmt::Debug for PostExecBlockHandle<R> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("PostExecBlockHandle")
            .field("block_id", &self.block_id)
            .field("completed", &self.completed)
            .finish()
    }
}

impl<R: Receipt + Send + 'static> PostExecBlockHandle<R> {
    /// Stream one receipt to the background worker.
    pub fn push_receipt(&self, index: usize, receipt: R) {
        if self
            .events_tx
            .send(PostExecEvent::Receipt {
                block_id: self.block_id,
                receipt: IndexedReceipt::new(index, receipt),
            })
            .is_err()
        {
            error!(
                target: "engine::tree::payload_processor",
                block_id = self.block_id,
                index,
                "post-exec worker dropped before Receipt event",
            );
        }
    }

    /// Finalize this block and return a receiver for the computed root+bloom.
    pub fn finalize(mut self) -> oneshot::Receiver<(B256, Bloom)> {
        self.completed = true;
        let (result_tx, result_rx) = oneshot::channel();
        if self
            .events_tx
            .send(PostExecEvent::FinalizeBlock { block_id: self.block_id, result_tx })
            .is_err()
        {
            error!(
                target: "engine::tree::payload_processor",
                block_id = self.block_id,
                "post-exec worker dropped before FinalizeBlock event",
            );
        }
        result_rx
    }
}

impl<R> Drop for PostExecBlockHandle<R> {
    fn drop(&mut self) {
        if self.completed {
            return;
        }

        let _ = self.events_tx.send(PostExecEvent::AbortBlock { block_id: self.block_id });
    }
}

enum PostExecEvent<R> {
    BeginBlock { block_id: u64, receipts_len: usize },
    Receipt { block_id: u64, receipt: IndexedReceipt<R> },
    FinalizeBlock { block_id: u64, result_tx: oneshot::Sender<(B256, Bloom)> },
    AbortBlock { block_id: u64 },
}

#[derive(Debug)]
struct ReceiptAccumulator {
    builder: OrderedTrieRootEncodedBuilder,
    aggregated_bloom: Bloom,
    expected_count: usize,
    received_count: usize,
    encode_buf: Vec<u8>,
}

impl ReceiptAccumulator {
    fn new(receipts_len: usize) -> Self {
        Self {
            builder: OrderedTrieRootEncodedBuilder::new(receipts_len),
            aggregated_bloom: Bloom::ZERO,
            expected_count: receipts_len,
            received_count: 0,
            encode_buf: Vec::new(),
        }
    }

    fn push<R: Receipt>(&mut self, receipt: IndexedReceipt<R>) {
        let receipt_with_bloom = receipt.receipt.with_bloom_ref();
        self.encode_buf.clear();
        receipt_with_bloom.encode_2718(&mut self.encode_buf);
        self.aggregated_bloom |= *receipt_with_bloom.bloom_ref();

        match self.builder.push(receipt.index, &self.encode_buf) {
            Ok(()) => {
                self.received_count += 1;
            }
            Err(err) => {
                error!(
                    target: "engine::tree::payload_processor",
                    index = receipt.index,
                    ?err,
                    "post-exec worker received invalid receipt index",
                );
            }
        }
    }

    fn finalize(self) -> Option<(B256, Bloom)> {
        let Ok(root) = self.builder.finalize() else {
            error!(
                target: "engine::tree::payload_processor",
                expected = self.expected_count,
                received = self.received_count,
                "post-exec worker received incomplete receipts",
            );
            return None;
        };

        Some((root, self.aggregated_bloom))
    }
}

struct PostExecWorker {
    inflight: HashMap<u64, ReceiptAccumulator>,
}

impl PostExecWorker {
    fn run<R: Receipt>(events_rx: Receiver<PostExecEvent<R>>) {
        let mut worker = Self { inflight: HashMap::new() };
        while let Ok(event) = events_rx.recv() {
            worker.handle_event(event);
        }

        if !worker.inflight.is_empty() {
            debug!(
                target: "engine::tree::payload_processor",
                inflight = worker.inflight.len(),
                "post-exec worker shutting down with unfinished blocks",
            );
        }
    }

    fn handle_event<R: Receipt>(&mut self, event: PostExecEvent<R>) {
        match event {
            PostExecEvent::BeginBlock { block_id, receipts_len } => {
                let previous =
                    self.inflight.insert(block_id, ReceiptAccumulator::new(receipts_len));
                if previous.is_some() {
                    warn!(
                        target: "engine::tree::payload_processor",
                        block_id,
                        "post-exec worker replaced existing inflight block state",
                    );
                }
            }
            PostExecEvent::Receipt { block_id, receipt } => {
                let Some(accumulator) = self.inflight.get_mut(&block_id) else {
                    warn!(
                        target: "engine::tree::payload_processor",
                        block_id,
                        "post-exec worker received receipt for unknown block",
                    );
                    return;
                };
                accumulator.push(receipt);
            }
            PostExecEvent::FinalizeBlock { block_id, result_tx } => {
                let Some(accumulator) = self.inflight.remove(&block_id) else {
                    warn!(
                        target: "engine::tree::payload_processor",
                        block_id,
                        "post-exec worker finalized unknown block",
                    );
                    return;
                };

                if let Some(result) = accumulator.finalize() {
                    let _ = result_tx.send(result);
                }
            }
            PostExecEvent::AbortBlock { block_id } => {
                self.inflight.remove(&block_id);
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
    use reth_tasks::Runtime;

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
        let runtime = Runtime::test();
        let coordinator = PostExecCoordinator::<Receipt>::new(&runtime);

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
        let runtime = Runtime::test();
        let coordinator = PostExecCoordinator::<Receipt>::new(&runtime);

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
        let runtime = Runtime::test();
        let coordinator = PostExecCoordinator::<Receipt>::new(&runtime);

        let mut receipts = sample_receipts();
        let first = receipts.remove(0);
        let second = receipts.remove(0);

        let handle = coordinator.begin_block(2);
        handle.push_receipt(0, first.clone());
        handle.push_receipt(0, second);

        assert!(handle.finalize().await.is_err());
    }

    #[tokio::test]
    async fn post_exec_worker_supports_multiple_inflight_blocks() {
        let runtime = Runtime::test();
        let coordinator = PostExecCoordinator::<Receipt>::new(&runtime);

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

        let (root_b, bloom_b) = handle_b.finalize().await.unwrap();
        let (root_a, bloom_a) = handle_a.finalize().await.unwrap();

        assert_eq!(root_a, expected_root_a);
        assert_eq!(bloom_a, expected_bloom_a);
        assert_eq!(root_b, expected_root_b);
        assert_eq!(bloom_b, expected_bloom_b);
    }

    #[tokio::test]
    async fn post_exec_worker_aborts_incomplete_blocks() {
        let runtime = Runtime::test();
        let coordinator = PostExecCoordinator::<Receipt>::new(&runtime);

        let handle = coordinator.begin_block(2);
        handle.push_receipt(0, Receipt::default());
        drop(handle);

        let second = coordinator.begin_block(1);
        second.push_receipt(0, Receipt::default());
        assert!(second.finalize().await.is_ok());
    }
}
