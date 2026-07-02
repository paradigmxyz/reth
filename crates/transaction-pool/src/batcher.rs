//! Transaction batching for `Pool` insertion for high-throughput scenarios
//!
//! This module provides transaction batching logic to reduce lock contention when processing
//! many concurrent transaction pool insertions.
//!
//! The batcher operates in one of two modes:
//! - **Immediate mode** (`batch_timeout = None`, the default): requests are processed as soon as
//!   they arrive and each insertion task is spawned without a concurrency bound, identical to an
//!   unbatched processor.
//! - **Batch-and-timeout mode** (`batch_timeout = Some(..)`): requests are coalesced until either
//!   `max_batch_size` is reached or the timeout expires after the first request was buffered.
//!   Spawned insertion batches are bounded by `max_concurrent_batches`.

use crate::{
    config::BatchConfig, error::PoolError, metrics::BatchMetrics, AddedTransactionOutcome,
    PoolTransaction, TransactionOrigin, TransactionPool,
};
use pin_project::pin_project;
use std::{
    future::Future,
    pin::Pin,
    sync::Arc,
    task::{ready, Context, Poll},
    time::Duration,
};
use tokio::{
    sync::{mpsc, oneshot, OwnedSemaphorePermit, Semaphore},
    time::Sleep,
};
use tokio_util::sync::PollSemaphore;

/// A single batch transaction request
#[derive(Debug)]
pub struct BatchTxRequest<T: PoolTransaction> {
    /// Origin of the transaction (e.g. Local, External)
    origin: TransactionOrigin,
    /// Tx to be inserted in to the pool
    pool_tx: T,
    /// Channel to send result back to caller
    response_tx: oneshot::Sender<Result<AddedTransactionOutcome, PoolError>>,
}

impl<T> BatchTxRequest<T>
where
    T: PoolTransaction,
{
    /// Create a new batch transaction request
    pub const fn new(
        origin: TransactionOrigin,
        pool_tx: T,
        response_tx: oneshot::Sender<Result<AddedTransactionOutcome, PoolError>>,
    ) -> Self {
        Self { origin, pool_tx, response_tx }
    }
}

/// Transaction batch processor that handles batch processing
///
/// Supports two modes:
/// - **Immediate mode** (`batch_timeout = None`): Processes requests as soon as available (greedy),
///   spawning insertion tasks without a concurrency bound
/// - **Batch-and-timeout mode** (`batch_timeout = Some(...)`): Waits for `max_batch_size` OR
///   timeout before processing, with spawned batches bounded by `batch_permits`
#[pin_project]
pub struct BatchTxProcessor<Pool: TransactionPool> {
    pool: Pool,
    max_batch_size: usize,
    /// Timeout for partial batches. `None` keeps the immediate processing mode.
    batch_timeout: Option<Duration>,
    buf: Vec<BatchTxRequest<Pool::Transaction>>,
    #[pin]
    request_rx: mpsc::UnboundedReceiver<BatchTxRequest<Pool::Transaction>>,
    /// Deadline armed when the first request enters an empty batch.
    #[pin]
    batch_deadline: Option<Sleep>,
    /// Tracks an expired deadline while waiting for an insertion permit.
    flush_due: bool,
    /// Limits how many insertion batches can be processed concurrently.
    ///
    /// Only used in batch-and-timeout mode; immediate mode spawns unbounded.
    batch_permits: PollSemaphore,
    metrics: BatchMetrics,
}

impl<Pool> std::fmt::Debug for BatchTxProcessor<Pool>
where
    Pool: TransactionPool + std::fmt::Debug,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BatchTxProcessor")
            .field("pool", &self.pool)
            .field("max_batch_size", &self.max_batch_size)
            .field("batch_timeout", &self.batch_timeout)
            .field("buf_len", &self.buf.len())
            .field("has_batch_deadline", &self.batch_deadline.is_some())
            .finish()
    }
}

impl<Pool> BatchTxProcessor<Pool>
where
    Pool: TransactionPool + 'static,
{
    /// Create a new `BatchTxProcessor`
    pub fn new(
        pool: Pool,
        config: BatchConfig,
    ) -> (Self, mpsc::UnboundedSender<BatchTxRequest<Pool::Transaction>>) {
        let (request_tx, request_rx) = mpsc::unbounded_channel();
        let BatchConfig { max_batch_size, batch_timeout, max_concurrent_batches } = config;
        let max_batch_size = max_batch_size.max(1);
        let max_concurrent_batches = max_concurrent_batches.clamp(1, Semaphore::MAX_PERMITS);

        let processor = Self {
            pool,
            max_batch_size,
            batch_timeout,
            buf: Vec::with_capacity(max_batch_size),
            request_rx,
            batch_deadline: None,
            flush_due: false,
            batch_permits: PollSemaphore::new(Arc::new(Semaphore::new(max_concurrent_batches))),
            metrics: BatchMetrics::default(),
        };

        tracing::debug!(
            target: "txpool::batcher",
            max_batch_size,
            ?batch_timeout,
            max_concurrent_batches,
            mode = if batch_timeout.is_none() { "immediate" } else { "batch-and-timeout" },
            "Transaction batcher initialized"
        );

        (processor, request_tx)
    }

    async fn process_request(pool: &Pool, req: BatchTxRequest<Pool::Transaction>) {
        let BatchTxRequest { origin, pool_tx, response_tx } = req;
        let pool_result = pool.add_transaction(origin, pool_tx).await;
        let _ = response_tx.send(pool_result);
    }

    /// Process a batch of transaction requests with per-transaction origins
    async fn process_batch(pool: &Pool, batch: Vec<BatchTxRequest<Pool::Transaction>>) {
        if batch.len() == 1 {
            Self::process_request(pool, batch.into_iter().next().expect("batch is not empty"))
                .await;
            return
        }

        // Same-origin batches can skip the per-transaction origins allocation.
        let mut batch_iter = batch.iter();
        if let Some(origin) = batch_iter.next().map(|req| req.origin) &&
            batch_iter.all(|req| req.origin == origin)
        {
            let (transactions, response_txs): (Vec<_>, Vec<_>) =
                batch.into_iter().map(|req| (req.pool_tx, req.response_tx)).unzip();

            let pool_results = pool.add_transactions(origin, transactions).await;
            for (response_tx, pool_result) in response_txs.into_iter().zip(pool_results) {
                let _ = response_tx.send(pool_result);
            }
            return
        }

        let (transactions, response_txs): (Vec<_>, Vec<_>) =
            batch.into_iter().map(|req| ((req.origin, req.pool_tx), req.response_tx)).unzip();

        let pool_results = pool.add_transactions_with_origins(transactions).await;
        for (response_tx, pool_result) in response_txs.into_iter().zip(pool_results) {
            let _ = response_tx.send(pool_result);
        }
    }

    /// Spawn a batch processing task, holding the concurrency permit (if any) until the batch is
    /// fully processed
    fn spawn_batch(
        pool: &Pool,
        buf: &mut Vec<BatchTxRequest<Pool::Transaction>>,
        permit: Option<OwnedSemaphorePermit>,
        metrics: &BatchMetrics,
    ) {
        if buf.is_empty() {
            return;
        }
        metrics.batch_size.record(buf.len() as f64);
        let batch = std::mem::take(buf);
        let pool = pool.clone();
        tokio::spawn(async move {
            let _permit = permit;
            Self::process_batch(&pool, batch).await;
        });
    }

    /// Immediate mode: greedily spawn whatever requests are available, without a concurrency
    /// bound.
    ///
    /// This is the pass-through path used when no `batch_timeout` is configured and matches the
    /// behavior of an unbatched processor.
    fn poll_immediate(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        let mut this = self.project();

        loop {
            let n =
                ready!(this.request_rx.as_mut().poll_recv_many(cx, this.buf, *this.max_batch_size));
            Self::spawn_batch(this.pool, this.buf, None, this.metrics);
            if n == 0 {
                // channel closed
                return Poll::Ready(())
            }
        }
    }

    /// Batch-and-timeout mode: coalesce requests until the batch is full or the deadline armed by
    /// the first buffered request expires, bounding spawned batches with `batch_permits`.
    fn poll_batched(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        let mut this = self.project();

        loop {
            while this.buf.len() < *this.max_batch_size {
                let was_empty = this.buf.is_empty();
                let remaining_capacity = *this.max_batch_size - this.buf.len();
                match this.request_rx.as_mut().poll_recv_many(cx, this.buf, remaining_capacity) {
                    Poll::Ready(0) => {
                        // Channel closed, flush remaining and exit
                        if !this.buf.is_empty() {
                            let Some(permit) = ready!(this.batch_permits.poll_acquire(cx)) else {
                                return Poll::Ready(())
                            };
                            Self::spawn_batch(this.pool, this.buf, Some(permit), this.metrics);
                        }
                        return Poll::Ready(())
                    }
                    Poll::Ready(_n) => {
                        if was_empty && let Some(timeout) = *this.batch_timeout {
                            this.batch_deadline.set(Some(tokio::time::sleep(timeout)));
                            *this.flush_due = false;
                        }
                    }
                    Poll::Pending => break,
                }
            }

            if this.buf.is_empty() {
                return Poll::Pending
            }

            // A full batch flushes without consulting the deadline
            if this.buf.len() < *this.max_batch_size && !*this.flush_due {
                let Some(deadline) = this.batch_deadline.as_mut().as_pin_mut() else {
                    return Poll::Pending
                };
                ready!(deadline.poll(cx));
                *this.flush_due = true;
            }

            let Some(permit) = ready!(this.batch_permits.poll_acquire(cx)) else {
                return Poll::Ready(())
            };
            Self::spawn_batch(this.pool, this.buf, Some(permit), this.metrics);
            this.batch_deadline.set(None);
            *this.flush_due = false;
        }
    }
}

impl<Pool> Future for BatchTxProcessor<Pool>
where
    Pool: TransactionPool + 'static,
{
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if self.batch_timeout.is_none() {
            self.poll_immediate(cx)
        } else {
            self.poll_batched(cx)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::{testing_pool, MockTransaction};
    use futures::{
        stream::{FuturesUnordered, StreamExt},
        task::{waker_ref, ArcWake},
    };
    use std::{
        sync::atomic::{AtomicUsize, Ordering},
        time::Duration,
    };
    use tokio::time::timeout;

    #[derive(Default)]
    struct WakeCounter {
        wakes: AtomicUsize,
    }

    impl ArcWake for WakeCounter {
        fn wake_by_ref(arc_self: &std::sync::Arc<Self>) {
            arc_self.wakes.fetch_add(1, Ordering::SeqCst);
        }
    }

    fn batch_config(max_batch_size: usize, batch_timeout: Option<Duration>) -> BatchConfig {
        BatchConfig { max_batch_size, batch_timeout, ..Default::default() }
    }

    #[tokio::test]
    async fn test_process_batch() {
        let pool = testing_pool();

        let mut batch_requests = Vec::new();
        let mut responses = Vec::new();

        for i in 0..100 {
            let tx = MockTransaction::legacy().with_nonce(i).with_gas_price(100);
            let (response_tx, response_rx) = tokio::sync::oneshot::channel();

            batch_requests.push(BatchTxRequest::new(TransactionOrigin::Local, tx, response_tx));
            responses.push(response_rx);
        }

        BatchTxProcessor::process_batch(&pool, batch_requests).await;

        for response_rx in responses {
            let result = timeout(Duration::from_millis(5), response_rx)
                .await
                .expect("Timeout waiting for response")
                .expect("Response channel was closed unexpectedly");
            assert!(result.is_ok());
        }
    }

    #[tokio::test]
    async fn test_process_batch_mixed_origins() {
        let pool = testing_pool();

        let mut batch_requests = Vec::new();
        let mut responses = Vec::new();

        for (nonce, origin) in [
            (0, TransactionOrigin::Local),
            (1, TransactionOrigin::External),
            (2, TransactionOrigin::Private),
        ] {
            let tx = MockTransaction::legacy().with_nonce(nonce).with_gas_price(100);
            let (response_tx, response_rx) = tokio::sync::oneshot::channel();

            batch_requests.push(BatchTxRequest::new(origin, tx, response_tx));
            responses.push(response_rx);
        }

        BatchTxProcessor::process_batch(&pool, batch_requests).await;

        for response_rx in responses {
            let result = timeout(Duration::from_millis(5), response_rx)
                .await
                .expect("Timeout waiting for response")
                .expect("Response channel was closed unexpectedly");
            assert!(result.is_ok());
        }
    }

    #[tokio::test]
    async fn test_batch_processor() {
        let pool = testing_pool();
        let config = batch_config(1000, None);
        let (processor, request_tx) = BatchTxProcessor::new(pool.clone(), config);

        // Spawn the processor
        let handle = tokio::spawn(processor);

        let mut responses = Vec::new();

        for i in 0..50 {
            let tx = MockTransaction::legacy().with_nonce(i).with_gas_price(100);
            let (response_tx, response_rx) = tokio::sync::oneshot::channel();

            request_tx
                .send(BatchTxRequest::new(TransactionOrigin::Local, tx, response_tx))
                .expect("Could not send batch tx");
            responses.push(response_rx);
        }

        tokio::time::sleep(Duration::from_millis(10)).await;

        for rx in responses {
            let result = timeout(Duration::from_millis(10), rx)
                .await
                .expect("Timeout waiting for response")
                .expect("Response channel was closed unexpectedly");
            assert!(result.is_ok());
        }

        drop(request_tx);
        handle.abort();
    }

    #[tokio::test]
    async fn test_add_transaction() {
        let pool = testing_pool();
        let config = batch_config(1000, None);
        let (processor, request_tx) = BatchTxProcessor::new(pool.clone(), config);

        // Spawn the processor
        let handle = tokio::spawn(processor);

        let mut results = Vec::new();
        for i in 0..10 {
            let tx = MockTransaction::legacy().with_nonce(i).with_gas_price(100);
            let (response_tx, response_rx) = tokio::sync::oneshot::channel();
            let request = BatchTxRequest::new(TransactionOrigin::Local, tx, response_tx);
            request_tx.send(request).expect("Could not send batch tx");
            results.push(response_rx);
        }

        for res in results {
            let result = timeout(Duration::from_millis(10), res)
                .await
                .expect("Timeout waiting for transaction result");
            assert!(result.is_ok());
        }

        handle.abort();
    }

    #[tokio::test]
    async fn test_max_batch_size() {
        let pool = testing_pool();
        let max_batch_size = 10;
        let config = batch_config(max_batch_size, None);
        let (processor, request_tx) = BatchTxProcessor::new(pool.clone(), config);

        // Spawn batch processor with threshold
        let handle = tokio::spawn(processor);

        let mut futures = FuturesUnordered::new();
        for i in 0..max_batch_size {
            let tx = MockTransaction::legacy().with_nonce(i as u64).with_gas_price(100);
            let (response_tx, response_rx) = tokio::sync::oneshot::channel();
            let request = BatchTxRequest::new(TransactionOrigin::Local, tx, response_tx);
            let request_tx_clone = request_tx.clone();

            let tx_fut = async move {
                request_tx_clone.send(request).expect("Could not send batch tx");
                response_rx.await.expect("Could not receive batch response")
            };
            futures.push(tx_fut);
        }

        while let Some(result) = timeout(Duration::from_millis(5), futures.next())
            .await
            .expect("Timeout waiting for transaction result")
        {
            assert!(result.is_ok());
        }

        handle.abort();
    }

    #[tokio::test]
    async fn test_max_batch_size_zero_is_clamped() {
        let pool = testing_pool();
        let config = batch_config(0, None);
        let (processor, request_tx) = BatchTxProcessor::new(pool.clone(), config);
        assert_eq!(processor.max_batch_size, 1);

        let handle = tokio::spawn(processor);

        let tx = MockTransaction::legacy().with_nonce(0).with_gas_price(100);
        let (response_tx, response_rx) = tokio::sync::oneshot::channel();
        request_tx
            .send(BatchTxRequest::new(TransactionOrigin::Local, tx, response_tx))
            .expect("Could not send batch tx");

        let result = timeout(Duration::from_millis(50), response_rx)
            .await
            .expect("max_batch_size = 0 should be clamped and not hang")
            .expect("Response channel was closed unexpectedly");
        assert!(result.is_ok());

        handle.abort();
    }

    /// Test that batch is flushed when timeout expires (even if batch is not full)
    #[tokio::test]
    async fn test_batch_timeout_triggers_flush() {
        let pool = testing_pool();
        // Large batch size, small timeout - timeout should trigger the flush
        let batch_timeout = Duration::from_millis(50);
        let config = batch_config(1000, Some(batch_timeout));
        let (processor, request_tx) = BatchTxProcessor::new(pool.clone(), config);

        let handle = tokio::spawn(processor);

        // Send fewer transactions than max_batch_size
        let mut responses = Vec::new();
        for i in 0..5 {
            let tx = MockTransaction::legacy().with_nonce(i).with_gas_price(100);
            let (response_tx, response_rx) = tokio::sync::oneshot::channel();
            request_tx
                .send(BatchTxRequest::new(TransactionOrigin::Local, tx, response_tx))
                .expect("Could not send batch tx");
            responses.push(response_rx);
        }

        // Wait slightly longer than batch_timeout
        tokio::time::sleep(batch_timeout + Duration::from_millis(20)).await;

        // All transactions should be processed even though batch wasn't full
        for rx in responses {
            let result = timeout(Duration::from_millis(10), rx)
                .await
                .expect("Timeout waiting for response - batch timeout did not trigger flush")
                .expect("Response channel was closed unexpectedly");
            assert!(result.is_ok());
        }

        drop(request_tx);
        handle.abort();
    }

    /// Test that `max_batch_size` triggers flush before timeout
    #[tokio::test]
    async fn test_max_batch_size_triggers_before_timeout() {
        let pool = testing_pool();
        let max_batch_size = 5;
        // Long timeout, small batch size - batch size should trigger first
        let batch_timeout = Duration::from_secs(60);
        let config = batch_config(max_batch_size, Some(batch_timeout));
        let (processor, request_tx) = BatchTxProcessor::new(pool.clone(), config);

        let handle = tokio::spawn(processor);

        // Send exactly max_batch_size transactions
        let mut responses = Vec::new();
        for i in 0..max_batch_size {
            let tx = MockTransaction::legacy().with_nonce(i as u64).with_gas_price(100);
            let (response_tx, response_rx) = tokio::sync::oneshot::channel();
            request_tx
                .send(BatchTxRequest::new(TransactionOrigin::Local, tx, response_tx))
                .expect("Could not send batch tx");
            responses.push(response_rx);
        }

        // Should complete quickly without waiting for the 60s timeout
        for rx in responses {
            let result = timeout(Duration::from_millis(100), rx)
                .await
                .expect("Timeout - max_batch_size did not trigger flush before timeout")
                .expect("Response channel was closed unexpectedly");
            assert!(result.is_ok());
        }

        drop(request_tx);
        handle.abort();
    }

    /// Test that `None` timeout maintains original immediate processing behavior
    #[tokio::test]
    async fn test_none_timeout_immediate_processing() {
        let pool = testing_pool();
        let config = batch_config(1000, None);
        let (processor, request_tx) = BatchTxProcessor::new(pool.clone(), config);

        let handle = tokio::spawn(processor);

        // Send a single transaction
        let tx = MockTransaction::legacy().with_nonce(0).with_gas_price(100);
        let (response_tx, response_rx) = tokio::sync::oneshot::channel();
        request_tx
            .send(BatchTxRequest::new(TransactionOrigin::Local, tx, response_tx))
            .expect("Could not send batch tx");

        // Should be processed immediately (within a few ms)
        let result = timeout(Duration::from_millis(10), response_rx)
            .await
            .expect("None timeout mode should process immediately")
            .expect("Response channel was closed unexpectedly");
        assert!(result.is_ok());

        handle.abort();
    }

    #[tokio::test]
    async fn test_timeout_starts_when_first_item_is_buffered() {
        use std::future::Future;

        let pool = testing_pool();
        let batch_timeout = Duration::from_millis(25);
        let config = batch_config(10, Some(batch_timeout));
        let (processor, request_tx) = BatchTxProcessor::new(pool.clone(), config);
        let mut processor = Box::pin(processor);

        tokio::time::sleep(batch_timeout + Duration::from_millis(10)).await;

        let tx = MockTransaction::legacy().with_nonce(0).with_gas_price(100);
        let (response_tx, mut response_rx) = tokio::sync::oneshot::channel();
        request_tx
            .send(BatchTxRequest::new(TransactionOrigin::Local, tx, response_tx))
            .expect("send failed");

        let waker = futures::task::noop_waker();
        let mut cx = Context::from_waker(&waker);
        let result = processor.as_mut().poll(&mut cx);
        assert!(result.is_pending(), "Partial batch should wait for its own deadline");
        assert!(response_rx.try_recv().is_err(), "Timeout should not fire from idle time");

        tokio::time::sleep(batch_timeout + Duration::from_millis(10)).await;

        let result = processor.as_mut().poll(&mut cx);
        assert!(result.is_pending(), "Should return Pending after spawning timed-out batch");

        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(response_rx.try_recv().is_ok(), "Batch should flush after its own timeout");
    }

    // ===== Manual Poll Tests =====

    /// Test that polling with empty buffer returns Pending
    #[tokio::test]
    async fn test_poll_empty_returns_pending() {
        use std::future::Future;

        let pool = testing_pool();
        let config = batch_config(10, Some(Duration::from_secs(60)));
        let (processor, _request_tx) = BatchTxProcessor::new(pool, config);

        let mut processor = Box::pin(processor);

        // Poll once - should return Pending since no items
        let waker = futures::task::noop_waker();
        let mut cx = Context::from_waker(&waker);
        let result = processor.as_mut().poll(&mut cx);
        assert!(result.is_pending(), "Empty buffer should return Pending");
    }

    /// Test that full batch spawns immediately without waiting for interval
    #[tokio::test]
    async fn test_poll_full_batch_spawns_immediately() {
        use std::future::Future;

        let pool = testing_pool();
        let max_batch_size = 5;
        // Very long timeout - should NOT be needed for full batch
        let config = batch_config(max_batch_size, Some(Duration::from_secs(3600)));
        let (processor, request_tx) = BatchTxProcessor::new(pool.clone(), config);

        let mut processor = Box::pin(processor);

        // Send exactly max_batch_size items
        let mut responses = Vec::new();
        for i in 0..max_batch_size {
            let tx = MockTransaction::legacy().with_nonce(i as u64).with_gas_price(100);
            let (response_tx, response_rx) = tokio::sync::oneshot::channel();
            request_tx
                .send(BatchTxRequest::new(TransactionOrigin::Local, tx, response_tx))
                .expect("send failed");
            responses.push(response_rx);
        }

        // Poll once - should process the full batch immediately
        let waker = futures::task::noop_waker();
        let mut cx = Context::from_waker(&waker);
        let result = processor.as_mut().poll(&mut cx);
        assert!(result.is_pending(), "Should return Pending after spawning batch");

        // Give spawned task time to complete
        tokio::time::sleep(Duration::from_millis(50)).await;

        // All responses should be ready
        for mut rx in responses {
            let result = rx.try_recv();
            assert!(result.is_ok(), "Response should be ready after batch spawned");
        }
    }

    /// Test that batch dispatch waits for a concurrency permit in batch-and-timeout mode.
    #[tokio::test]
    async fn test_poll_full_batch_waits_for_batch_permit() {
        use std::future::Future;

        let pool = testing_pool();
        let config = BatchConfig {
            max_batch_size: 1,
            batch_timeout: Some(Duration::from_secs(3600)),
            max_concurrent_batches: 1,
        };
        let (mut processor, request_tx) = BatchTxProcessor::new(pool.clone(), config);

        let waker = futures::task::noop_waker();
        let mut cx = Context::from_waker(&waker);
        let held_permit = match processor.batch_permits.poll_acquire(&mut cx) {
            Poll::Ready(Some(permit)) => permit,
            _ => panic!("expected available batch permit"),
        };

        let mut processor = Box::pin(processor);
        let tx = MockTransaction::legacy().with_nonce(0).with_gas_price(100);
        let (response_tx, mut response_rx) = tokio::sync::oneshot::channel();
        request_tx
            .send(BatchTxRequest::new(TransactionOrigin::Local, tx, response_tx))
            .expect("send failed");

        let result = processor.as_mut().poll(&mut cx);
        assert!(result.is_pending(), "Should wait for a batch permit");
        assert!(response_rx.try_recv().is_err(), "Batch should not be dispatched without permit");

        drop(held_permit);

        let result = processor.as_mut().poll(&mut cx);
        assert!(result.is_pending(), "Should return Pending after spawning");

        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(response_rx.try_recv().is_ok(), "Batch should be dispatched after permit release");
    }

    /// Test that immediate mode spawns insertions without acquiring a concurrency permit.
    #[tokio::test]
    async fn test_immediate_mode_ignores_permit_cap() {
        use std::future::Future;

        let pool = testing_pool();
        let config =
            BatchConfig { max_batch_size: 1, batch_timeout: None, max_concurrent_batches: 1 };
        let (mut processor, request_tx) = BatchTxProcessor::new(pool.clone(), config);

        // Hold the only permit; immediate mode must dispatch regardless
        let waker = futures::task::noop_waker();
        let mut cx = Context::from_waker(&waker);
        let _held_permit = match processor.batch_permits.poll_acquire(&mut cx) {
            Poll::Ready(Some(permit)) => permit,
            _ => panic!("expected available batch permit"),
        };

        let mut processor = Box::pin(processor);
        let tx = MockTransaction::legacy().with_nonce(0).with_gas_price(100);
        let (response_tx, mut response_rx) = tokio::sync::oneshot::channel();
        request_tx
            .send(BatchTxRequest::new(TransactionOrigin::Local, tx, response_tx))
            .expect("send failed");

        let result = processor.as_mut().poll(&mut cx);
        assert!(result.is_pending(), "Should return Pending after spawning");

        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(
            response_rx.try_recv().is_ok(),
            "Immediate mode should dispatch without a concurrency permit"
        );
    }

    /// Test that partial batch with timeout waits before flushing
    #[tokio::test]
    async fn test_poll_partial_batch_with_timeout_waits() {
        use std::future::Future;

        let pool = testing_pool();
        let max_batch_size = 10;
        // Long timeout - batch should NOT flush immediately
        let config = batch_config(max_batch_size, Some(Duration::from_secs(3600)));
        let (processor, request_tx) = BatchTxProcessor::new(pool, config);

        let mut processor = Box::pin(processor);

        // Send fewer items than max_batch_size
        let tx = MockTransaction::legacy().with_nonce(0).with_gas_price(100);
        let (response_tx, mut response_rx) = tokio::sync::oneshot::channel();
        request_tx
            .send(BatchTxRequest::new(TransactionOrigin::Local, tx, response_tx))
            .expect("send failed");

        // Poll once - should return Pending (waiting for timeout)
        let waker = futures::task::noop_waker();
        let mut cx = Context::from_waker(&waker);
        let result = processor.as_mut().poll(&mut cx);
        assert!(result.is_pending(), "Partial batch with timeout should return Pending");

        // Response should NOT be ready yet (batch not flushed)
        let try_result = response_rx.try_recv();
        assert!(try_result.is_err(), "Partial batch should not be flushed immediately");
    }

    #[tokio::test]
    async fn test_partial_batch_registers_channel_waker() {
        use std::future::Future;

        let pool = testing_pool();
        let config = batch_config(2, Some(Duration::from_secs(3600)));
        let (processor, request_tx) = BatchTxProcessor::new(pool.clone(), config);
        let mut processor = Box::pin(processor);

        let tx = MockTransaction::legacy().with_nonce(0).with_gas_price(100);
        let (response_tx, response_rx_0) = tokio::sync::oneshot::channel();
        request_tx
            .send(BatchTxRequest::new(TransactionOrigin::Local, tx, response_tx))
            .expect("send failed");

        let wake_counter = std::sync::Arc::new(WakeCounter::default());
        let waker = waker_ref(&wake_counter);
        let mut cx = Context::from_waker(&waker);

        let result = processor.as_mut().poll(&mut cx);
        assert!(result.is_pending(), "Partial batch should wait for more transactions");

        let wakes_before = wake_counter.wakes.load(Ordering::SeqCst);
        let tx = MockTransaction::legacy().with_nonce(1).with_gas_price(100);
        let (response_tx, response_rx_1) = tokio::sync::oneshot::channel();
        request_tx
            .send(BatchTxRequest::new(TransactionOrigin::Local, tx, response_tx))
            .expect("send failed");
        assert!(
            wake_counter.wakes.load(Ordering::SeqCst) > wakes_before,
            "Partial batch must register the channel waker while waiting for timeout"
        );

        let result = processor.as_mut().poll(&mut cx);
        assert!(result.is_pending(), "Should return Pending after spawning full batch");

        for rx in [response_rx_0, response_rx_1] {
            let result = timeout(Duration::from_millis(50), rx)
                .await
                .expect("Timeout waiting for full batch response")
                .expect("Response channel was closed unexpectedly");
            assert!(result.is_ok());
        }
    }

    #[tokio::test]
    async fn test_due_timeout_flushes_after_permit_released() {
        use std::future::Future;

        let pool = testing_pool();
        let config = BatchConfig {
            max_batch_size: 10,
            batch_timeout: Some(Duration::from_millis(10)),
            max_concurrent_batches: 1,
        };
        let (mut processor, request_tx) = BatchTxProcessor::new(pool.clone(), config);

        let waker = futures::task::noop_waker();
        let mut cx = Context::from_waker(&waker);
        let held_permit = match processor.batch_permits.poll_acquire(&mut cx) {
            Poll::Ready(Some(permit)) => permit,
            _ => panic!("expected available batch permit"),
        };

        let mut processor = Box::pin(processor);
        let tx = MockTransaction::legacy().with_nonce(0).with_gas_price(100);
        let (response_tx, mut response_rx) = tokio::sync::oneshot::channel();
        request_tx
            .send(BatchTxRequest::new(TransactionOrigin::Local, tx, response_tx))
            .expect("send failed");

        let result = processor.as_mut().poll(&mut cx);
        assert!(result.is_pending(), "Partial batch should wait for timeout");

        tokio::time::sleep(Duration::from_millis(20)).await;

        let result = processor.as_mut().poll(&mut cx);
        assert!(result.is_pending(), "Due batch should wait for permit");
        assert!(response_rx.try_recv().is_err(), "Batch should not flush without permit");

        drop(held_permit);

        let result = processor.as_mut().poll(&mut cx);
        assert!(result.is_pending(), "Should flush due batch after permit release");

        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(response_rx.try_recv().is_ok(), "Due batch should not wait for another timeout");
    }

    /// Test that partial batch in immediate mode (no interval) flushes right away
    #[tokio::test]
    async fn test_poll_partial_batch_immediate_mode_flushes() {
        use std::future::Future;

        let pool = testing_pool();
        let max_batch_size = 10;
        // No timeout = immediate mode
        let config = batch_config(max_batch_size, None);
        let (processor, request_tx) = BatchTxProcessor::new(pool.clone(), config);

        let mut processor = Box::pin(processor);

        // Send a single item (partial batch)
        let tx = MockTransaction::legacy().with_nonce(0).with_gas_price(100);
        let (response_tx, mut response_rx) = tokio::sync::oneshot::channel();
        request_tx
            .send(BatchTxRequest::new(TransactionOrigin::Local, tx, response_tx))
            .expect("send failed");

        // Poll once - should spawn batch immediately in immediate mode
        let waker = futures::task::noop_waker();
        let mut cx = Context::from_waker(&waker);
        let result = processor.as_mut().poll(&mut cx);
        assert!(result.is_pending(), "Should return Pending after spawning");

        // Give spawned task time to complete
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Response should be ready
        let result = response_rx.try_recv();
        assert!(result.is_ok(), "Immediate mode should flush partial batch right away");
    }

    /// Test that channel close flushes remaining items
    #[tokio::test]
    async fn test_poll_channel_close_flushes_remaining() {
        use std::future::Future;

        let pool = testing_pool();
        let max_batch_size = 10;
        let config = batch_config(max_batch_size, Some(Duration::from_secs(3600)));
        let (processor, request_tx) = BatchTxProcessor::new(pool.clone(), config);

        let mut processor = Box::pin(processor);

        // Send a partial batch
        let tx = MockTransaction::legacy().with_nonce(0).with_gas_price(100);
        let (response_tx, mut response_rx) = tokio::sync::oneshot::channel();
        request_tx
            .send(BatchTxRequest::new(TransactionOrigin::Local, tx, response_tx))
            .expect("send failed");

        // Poll once to receive the item
        let waker = futures::task::noop_waker();
        let mut cx = Context::from_waker(&waker);
        let _ = processor.as_mut().poll(&mut cx);

        // Drop sender to close channel
        drop(request_tx);

        // Poll again - should flush remaining and return Ready
        let result = processor.as_mut().poll(&mut cx);
        assert!(result.is_ready(), "Should return Ready when channel closes");

        // Give spawned task time to complete
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Response should be ready
        let result = response_rx.try_recv();
        assert!(result.is_ok(), "Channel close should flush remaining items");
    }
}
