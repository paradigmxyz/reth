//! Receipt root computation in a background task.
//!
//! Receipts arrive through an asynchronous stream. A cooperative worker encodes bounded batches,
//! sharing its driver between a production named thread and deterministic simulation. Closing
//! the result receiver stops input consumption and computation at the next scheduling point.

use alloy_eips::Encodable2718;
use alloy_primitives::{Bloom, B256};
use futures::{Stream, StreamExt};
use reth_primitives_traits::Receipt;
use reth_tasks::TaskRuntime;
use reth_trie_common::ordered_root::OrderedTrieRootEncodedBuilder;
use std::collections::HashMap;
use tokio::sync::oneshot;

const RECEIPT_ENCODE_BUF_INITIAL_CAPACITY: usize = 512;
const RECEIPT_BATCH_SIZE: usize = 64;

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

/// Handle for running the receipt root computation from an asynchronous stream.
///
/// The driver yields while waiting for input and between bounded batches. Production runs it on
/// a named thread; deterministic simulation schedules the same future without blocking receives.
#[derive(Debug)]
pub struct ReceiptRootTaskHandle<S> {
    /// Receiver for indexed receipts.
    receipt_rx: S,
    /// Sender for the computed result.
    result_tx: oneshot::Sender<(B256, Bloom)>,
}

impl<R, S> ReceiptRootTaskHandle<S>
where
    R: Receipt + 'static,
    S: Stream<Item = IndexedReceipt<R>> + Send,
{
    /// Creates a new handle from the receipt receiver and result sender channels.
    pub const fn new(receipt_rx: S, result_tx: oneshot::Sender<(B256, Bloom)>) -> Self {
        Self { receipt_rx, result_tx }
    }

    /// Runs the receipt root computation, consuming the handle.
    ///
    /// This method receives indexed receipts from the channel, encodes them,
    /// and builds the trie incrementally. When all receipts have been received
    /// (channel closed), it sends the result through the oneshot channel.
    ///
    /// At most 64 receipts are encoded between yields, including receipts buffered out of order.
    /// Once cancellation is observed, the driver stops without publishing a root.
    ///
    /// # Arguments
    ///
    /// * `receipts_len` - The total number of receipts expected. When provided, an incomplete
    ///   stream does not produce a result.
    #[tracing::instrument(
        name = "receipt_root",
        target = "engine::tree::payload_processor",
        level = "debug",
        skip_all
    )]
    pub async fn run(self, runtime: TaskRuntime, receipts_len: impl Into<Option<usize>>) {
        let receipts_len = receipts_len.into();
        let mut result_tx = self.result_tx;
        let mut receipts = Box::pin(self.receipt_rx);
        let mut builder = ReceiptRootBuilder::new();
        'consume: loop {
            let mut budget = RECEIPT_BATCH_SIZE;
            for _ in 0..RECEIPT_BATCH_SIZE {
                let receipt = tokio::select! {
                    biased;
                    _ = result_tx.closed() => return,
                    receipt = receipts.next() => receipt,
                };
                let Some(receipt) = receipt else { break 'consume };
                builder.push(receipt, &mut budget);
                if budget == 0 {
                    break;
                }
            }
            loop {
                // Yield even when the source remains ready, so abort and result closure are
                // observed while draining a large block or a long out-of-order suffix.
                runtime.yield_now().await;
                if result_tx.is_closed() {
                    return;
                }
                if !builder.pending.contains_key(&builder.next) {
                    break;
                }
                let mut budget = RECEIPT_BATCH_SIZE;
                builder.flush_pending(&mut budget);
            }
        }

        if result_tx.is_closed() {
            return;
        }
        if let Some(result) = builder.finish(receipts_len) {
            let _ = result_tx.send(result);
        }
    }
}

/// Accumulates receipts independently of how their stream is driven.
struct ReceiptRootBuilder<R> {
    builder: OrderedTrieRootEncodedBuilder,
    aggregated_bloom: Bloom,
    encode_buf: Vec<u8>,
    next: usize,
    pending: HashMap<usize, R>,
}

/// Computes the same receipt root on the caller when execution runs as one synchronous unit.
pub(crate) fn receipt_root_from_indexed<R: Receipt>(
    receipts: impl Iterator<Item = IndexedReceipt<R>>,
    expected_len: usize,
) -> Option<(B256, Bloom)> {
    let mut builder = ReceiptRootBuilder::new();
    let mut budget = usize::MAX;
    for receipt in receipts {
        builder.push(receipt, &mut budget);
    }
    builder.finish(Some(expected_len))
}

impl<R: Receipt> ReceiptRootBuilder<R> {
    fn new() -> Self {
        Self {
            builder: OrderedTrieRootEncodedBuilder::new(),
            aggregated_bloom: Bloom::ZERO,
            encode_buf: Vec::with_capacity(RECEIPT_ENCODE_BUF_INITIAL_CAPACITY),
            next: 0,
            pending: HashMap::new(),
        }
    }

    fn push(&mut self, indexed_receipt: IndexedReceipt<R>, budget: &mut usize) {
        if indexed_receipt.index == self.next && *budget > 0 {
            self.push_next(indexed_receipt.receipt);
            *budget -= 1;
            self.flush_pending(budget);
        } else {
            self.pending.insert(indexed_receipt.index, indexed_receipt.receipt);
        }
    }

    /// A late low-index receipt may unblock an entire block. Bound that flush too, so queued
    /// receipts cannot turn one worker poll into unbounded work between cancellation points.
    fn flush_pending(&mut self, budget: &mut usize) {
        while *budget > 0 {
            let Some(receipt) = self.pending.remove(&self.next) else { break };
            self.push_next(receipt);
            *budget -= 1;
        }
    }

    fn push_next(&mut self, receipt: R) {
        let receipt_with_bloom = receipt.with_bloom_ref();
        self.encode_buf.clear();
        receipt_with_bloom.encode_2718(&mut self.encode_buf);
        self.aggregated_bloom |= *receipt_with_bloom.bloom_ref();
        self.builder.push_next(&self.encode_buf);
        self.next += 1;
    }

    fn finish(self, receipts_len: Option<usize>) -> Option<(B256, Bloom)> {
        if receipts_len.is_some_and(|len| len != self.next) {
            tracing::error!(
                target: "engine::tree::payload_processor",
                expected = receipts_len,
                received = self.next,
                "Receipt root task received incomplete receipts, execution likely aborted"
            );
            return None;
        }

        if !self.pending.is_empty() {
            tracing::error!(
                target: "engine::tree::payload_processor",
                received = self.next,
                pending = self.pending.len(),
                "Receipt root task received gapped receipts"
            );
            return None;
        }

        Some((self.builder.finalize(), self.aggregated_bloom))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::{proofs::calculate_receipt_root, TxReceipt};
    use alloy_primitives::{b256, hex, Address, Bytes, Log};
    use commonware_runtime::{deterministic, Runner, Supervisor};
    use rand_08::{seq::SliceRandom, SeedableRng};
    use reth_ethereum_primitives::{Receipt, TxType};
    use std::time::Duration;
    use tokio::sync::mpsc;
    use tokio_stream::wrappers::{ReceiverStream, UnboundedReceiverStream};

    #[derive(Clone, Copy, Debug)]
    enum StreamEnd {
        Complete,
        Missing(usize),
        Cancel,
        DropResult,
    }

    #[derive(Debug, PartialEq, Eq)]
    struct SimulationOutcome {
        audit: String,
        delivered: Vec<usize>,
        result: Option<(B256, Bloom)>,
        rejected_sends: usize,
    }

    fn simulate_receipts(
        seed: u64,
        receipts: Vec<Receipt>,
        expected_len: Option<usize>,
        end: StreamEnd,
    ) -> SimulationOutcome {
        eprintln!(
            "receipt DST: seed={seed}, receipts={}, expected_len={expected_len:?}, end={end:?}",
            receipts.len()
        );
        let config = deterministic::Config::default()
            .with_seed(seed)
            .with_timeout(Some(Duration::from_secs(10)));
        deterministic::Runner::new(config).start(|context| async move {
            let runtime = TaskRuntime::deterministic(context.child("receipts"));
            let (tx, rx) =
                runtime.bounded_channel::<IndexedReceipt<Receipt>>(1 + seed as usize % 4);
            let (result_tx, result_rx) = oneshot::channel();
            let mut result_rx = Some(result_rx);
            let (started_tx, started_rx) = oneshot::channel();
            let (trace_tx, mut trace_rx) = mpsc::unbounded_channel();
            let mut started_tx = Some(started_tx);
            let mut received = 0;
            let stream = ReceiverStream::new(rx).inspect(move |receipt| {
                trace_tx.send(receipt.index).unwrap();
                received += 1;
                if received == 3 {
                    let _ = started_tx.take().unwrap().send(());
                }
            });
            let consumer = runtime.spawn_named_task(
                "receipt_root",
                ReceiptRootTaskHandle::new(stream, result_tx).run(runtime.clone(), expected_len),
            );

            let mut order: Vec<_> = receipts.into_iter().enumerate().collect();
            order.shuffle(&mut rand_08::rngs::StdRng::seed_from_u64(seed));
            let mut producers = Vec::new();
            for producer in 0..4 {
                let tx = tx.clone();
                let receipts: Vec<_> = order
                    .iter()
                    .skip(producer)
                    .step_by(4)
                    .filter(|(index, _)| {
                        !matches!(end, StreamEnd::Missing(missing) if missing == *index)
                    })
                    .cloned()
                    .collect();
                let sender_runtime = runtime.clone();
                producers.push(runtime.spawn("receipt_sender", async move {
                    for (index, receipt) in receipts {
                        sender_runtime.sleep(Duration::from_millis((index % 3 + 1) as u64)).await;
                        if tx.send(IndexedReceipt::new(index, receipt)).await.is_err() {
                            return 1;
                        }
                    }
                    0
                }));
            }
            drop(tx);

            if matches!(end, StreamEnd::Cancel | StreamEnd::DropResult) {
                // Cancel only after the driver has consumed part of the stream. The bounded
                // channel ensures senders still have pending work when its receiver is dropped.
                started_rx.await.unwrap();
                if matches!(end, StreamEnd::Cancel) {
                    consumer.abort();
                    assert!(consumer.await.is_err());
                } else {
                    drop(result_rx.take());
                    consumer.await.unwrap();
                }
            } else {
                consumer.await.unwrap();
            }

            let mut rejected_sends = 0;
            for producer in producers {
                rejected_sends += producer.await.unwrap();
            }
            let mut delivered = Vec::new();
            while let Some(index) = trace_rx.recv().await {
                delivered.push(index);
            }

            SimulationOutcome {
                audit: context.auditor().state(),
                delivered,
                result: match result_rx {
                    Some(result_rx) => result_rx.await.ok(),
                    None => None,
                },
                rejected_sends,
            }
        })
    }

    fn simulation_receipts(count: usize) -> Vec<Receipt> {
        (0..count)
            .map(|index| Receipt {
                tx_type: if index % 2 == 0 { TxType::Legacy } else { TxType::Eip1559 },
                cumulative_gas_used: (index as u64 + 1) * 21_000,
                success: index % 3 != 0,
                logs: vec![Log {
                    address: Address::with_last_byte(index as u8),
                    data: alloy_primitives::LogData::new_unchecked(
                        vec![B256::with_last_byte(index as u8)],
                        Bytes::from(vec![index as u8]),
                    ),
                }],
            })
            .collect()
    }

    #[test]
    fn late_receipt_respects_encoding_budget() {
        let receipts = simulation_receipts(2 * RECEIPT_BATCH_SIZE + 3);
        let expected = calculate_receipt_root(
            &receipts.iter().map(|receipt| receipt.with_bloom_ref()).collect::<Vec<_>>(),
        );
        let mut builder = ReceiptRootBuilder::new();
        // Queue a suffix larger than two work budgets, then unblock it with receipt zero.
        for (index, receipt) in receipts.iter().cloned().enumerate().skip(1) {
            let mut budget = RECEIPT_BATCH_SIZE;
            builder.push(IndexedReceipt::new(index, receipt), &mut budget);
        }
        let mut budget = RECEIPT_BATCH_SIZE;
        builder.push(IndexedReceipt::new(0, receipts[0].clone()), &mut budget);
        assert_eq!(builder.next, RECEIPT_BATCH_SIZE);
        assert_eq!(budget, 0);
        while !builder.pending.is_empty() {
            let before = builder.next;
            let mut budget = RECEIPT_BATCH_SIZE;
            builder.flush_pending(&mut budget);
            assert!(builder.next - before <= RECEIPT_BATCH_SIZE);
            assert!(builder.next > before);
        }
        assert_eq!(builder.finish(Some(receipts.len())).unwrap().0, expected);
    }

    #[test]
    fn deterministic_receipt_streams() {
        let seeds: Vec<u64> = match std::env::var("RETH_DST_SEED") {
            Ok(seed) => vec![seed.parse().expect("RETH_DST_SEED must be a u64")],
            Err(std::env::VarError::NotPresent) => (0..16).collect(),
            Err(error) => panic!("invalid RETH_DST_SEED: {error}"),
        };
        // Cross the RLP key-order boundary at index 128 with distinct receipts and nonzero blooms.
        let receipts = simulation_receipts(130);
        let with_bloom: Vec<_> = receipts.iter().map(|receipt| receipt.with_bloom_ref()).collect();
        let expected = (
            calculate_receipt_root(&with_bloom),
            with_bloom.iter().fold(Bloom::ZERO, |bloom, receipt| bloom | receipt.bloom_ref()),
        );
        let mut delivery_orders = std::collections::BTreeSet::new();

        for seed in seeds.iter().copied() {
            for expected_len in [Some(receipts.len()), None] {
                let outcome =
                    simulate_receipts(seed, receipts.clone(), expected_len, StreamEnd::Complete);
                assert_eq!(outcome.result, Some(expected), "seed {seed}");
                assert_eq!(outcome.rejected_sends, 0, "seed {seed}");
                assert_eq!(outcome.delivered.len(), receipts.len());
                assert!(outcome.delivered.windows(2).any(|pair| pair[0] > pair[1]));
                delivery_orders.insert(outcome.delivered.clone());
                assert_eq!(
                    outcome,
                    simulate_receipts(seed, receipts.clone(), expected_len, StreamEnd::Complete),
                    "replay diverged for seed {seed}"
                );
            }

            // An unknown total still requires a contiguous stream. A known total also detects a
            // missing suffix after an otherwise valid sequence of receipts.
            for (expected_len, missing) in [(None, 64), (Some(receipts.len()), 129)] {
                let outcome = simulate_receipts(
                    seed,
                    receipts.clone(),
                    expected_len,
                    StreamEnd::Missing(missing),
                );
                assert_eq!(outcome.result, None, "seed {seed}");
                assert_eq!(outcome.rejected_sends, 0);
                assert_eq!(outcome.delivered.len(), receipts.len() - 1);
                assert_eq!(
                    outcome,
                    simulate_receipts(
                        seed,
                        receipts.clone(),
                        expected_len,
                        StreamEnd::Missing(missing)
                    ),
                    "replay diverged for seed {seed}"
                );
            }

            for end in [StreamEnd::Cancel, StreamEnd::DropResult] {
                let outcome = simulate_receipts(seed, receipts.clone(), Some(receipts.len()), end);
                assert_eq!(outcome.result, None, "seed {seed}");
                assert!(outcome.rejected_sends > 0);
                assert!(outcome.delivered.len() >= 3 && outcome.delivered.len() < receipts.len());
                assert_eq!(
                    outcome,
                    simulate_receipts(seed, receipts.clone(), Some(receipts.len()), end),
                    "replay diverged for seed {seed}"
                );
            }
        }
        if seeds.len() > 1 {
            assert!(delivery_orders.len() > 1, "seeds did not vary delivery order");
        }

        let empty = simulate_receipts(seeds[0], Vec::new(), Some(0), StreamEnd::Complete);
        assert_eq!(empty.result, Some((reth_trie_common::EMPTY_ROOT_HASH, Bloom::ZERO)));
        assert!(empty.delivered.is_empty());
        assert_eq!(empty, simulate_receipts(seeds[0], Vec::new(), Some(0), StreamEnd::Complete));
    }

    #[tokio::test]
    async fn receipt_worker_stops_when_result_is_dropped() {
        let (receipt_tx, receipt_rx) = mpsc::unbounded_channel::<IndexedReceipt<Receipt>>();
        let (result_tx, result_rx) = oneshot::channel();
        let (started_tx, started_rx) = oneshot::channel();
        let mut started_tx = Some(started_tx);
        let receipts = UnboundedReceiverStream::new(receipt_rx).inspect(move |_| {
            if let Some(started_tx) = started_tx.take() {
                let _ = started_tx.send(());
            }
        });
        let runtime = TaskRuntime::from(reth_tasks::Runtime::test());
        let worker = runtime.spawn_named_task(
            "receipt-root",
            ReceiptRootTaskHandle::new(receipts, result_tx).run(runtime.clone(), None),
        );
        receipt_tx.send(IndexedReceipt::new(0, Receipt::default())).unwrap();
        started_rx.await.unwrap();

        // Keep the producer alive: result cancellation must wake the worker without waiting
        // for the input channel to close or another receipt to arrive.
        drop(result_rx);
        tokio::time::timeout(Duration::from_secs(5), worker).await.unwrap().unwrap();
        assert!(receipt_tx.is_closed());
    }

    #[tokio::test]
    async fn test_receipt_root_task_empty() {
        let (_tx, rx) = mpsc::unbounded_channel::<IndexedReceipt<Receipt>>();
        let (result_tx, result_rx) = oneshot::channel();
        drop(_tx);

        let handle = ReceiptRootTaskHandle::new(UnboundedReceiverStream::new(rx), result_tx);
        let runtime = TaskRuntime::from(reth_tasks::Runtime::test());
        handle.run(runtime, 0).await;

        let (root, bloom) = result_rx.await.unwrap();

        // Empty trie root
        assert_eq!(root, reth_trie_common::EMPTY_ROOT_HASH);
        assert_eq!(bloom, Bloom::ZERO);
    }

    #[tokio::test]
    async fn test_receipt_root_task_single_receipt() {
        let receipts: Vec<Receipt> = vec![Receipt::default()];

        let (tx, rx) = mpsc::unbounded_channel();
        let (result_tx, result_rx) = oneshot::channel();
        let receipts_len = receipts.len();

        let handle = ReceiptRootTaskHandle::new(UnboundedReceiverStream::new(rx), result_tx);
        let runtime = TaskRuntime::from(reth_tasks::Runtime::test());
        let join_handle =
            runtime.spawn_named_task("receipt-root", handle.run(runtime.clone(), receipts_len));

        for (i, receipt) in receipts.clone().into_iter().enumerate() {
            tx.send(IndexedReceipt::new(i, receipt)).unwrap();
        }
        drop(tx);

        join_handle.await.unwrap();
        let (root, _bloom) = result_rx.await.unwrap();

        // Verify against the standard calculation
        let receipts_with_bloom: Vec<_> = receipts.iter().map(|r| r.with_bloom_ref()).collect();
        let expected_root = calculate_receipt_root(&receipts_with_bloom);

        assert_eq!(root, expected_root);
    }

    #[tokio::test]
    async fn test_receipt_root_task_multiple_receipts() {
        let receipts: Vec<Receipt> = vec![Receipt::default(); 5];

        let (tx, rx) = mpsc::unbounded_channel();
        let (result_tx, result_rx) = oneshot::channel();
        let receipts_len = receipts.len();

        let handle = ReceiptRootTaskHandle::new(UnboundedReceiverStream::new(rx), result_tx);
        let runtime = TaskRuntime::from(reth_tasks::Runtime::test());
        let join_handle =
            runtime.spawn_named_task("receipt-root", handle.run(runtime.clone(), receipts_len));

        for (i, receipt) in receipts.into_iter().enumerate() {
            tx.send(IndexedReceipt::new(i, receipt)).unwrap();
        }
        drop(tx);

        join_handle.await.unwrap();
        let (root, bloom) = result_rx.await.unwrap();

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
        let (tx, rx) = mpsc::unbounded_channel();
        let (result_tx, result_rx) = oneshot::channel();
        let receipts_len = receipts.len();

        let handle = ReceiptRootTaskHandle::new(UnboundedReceiverStream::new(rx), result_tx);
        let runtime = TaskRuntime::from(reth_tasks::Runtime::test());
        let join_handle =
            runtime.spawn_named_task("receipt-root", handle.run(runtime.clone(), receipts_len));

        for (i, receipt) in receipts.into_iter().enumerate() {
            tx.send(IndexedReceipt::new(i, receipt)).unwrap();
        }
        drop(tx);

        join_handle.await.unwrap();
        let (task_root, task_bloom) = result_rx.await.unwrap();

        assert_eq!(task_root, expected_root);
        assert_eq!(task_bloom, expected_bloom);
    }

    #[tokio::test]
    async fn test_receipt_root_task_out_of_order() {
        let receipts: Vec<Receipt> = vec![Receipt::default(); 5];

        // Calculate expected values first (before we move receipts)
        let receipts_with_bloom: Vec<_> = receipts.iter().map(|r| r.with_bloom_ref()).collect();
        let expected_root = calculate_receipt_root(&receipts_with_bloom);

        let (tx, rx) = mpsc::unbounded_channel();
        let (result_tx, result_rx) = oneshot::channel();
        let receipts_len = receipts.len();

        let handle = ReceiptRootTaskHandle::new(UnboundedReceiverStream::new(rx), result_tx);
        let runtime = TaskRuntime::from(reth_tasks::Runtime::test());
        let join_handle =
            runtime.spawn_named_task("receipt-root", handle.run(runtime.clone(), receipts_len));

        // Send in reverse order to test out-of-order handling
        for (i, receipt) in receipts.into_iter().enumerate().rev() {
            tx.send(IndexedReceipt::new(i, receipt)).unwrap();
        }
        drop(tx);

        join_handle.await.unwrap();
        let (root, _bloom) = result_rx.await.unwrap();

        assert_eq!(root, expected_root);
    }
}
