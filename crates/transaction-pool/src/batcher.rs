//! Transaction batching for `Pool` insertion for high-throughput scenarios
//!
//! This module provides transaction batching logic to reduce lock contention when processing
//! many concurrent transaction pool insertions.
//!
//! The batcher supports an optional timeout mechanism: when `batch_timeout` is non-zero,
//! transactions are batched until either `max_batch_size` is reached OR the timeout expires.
//! When `batch_timeout` is zero, the batcher processes requests immediately (zero-cost path).

use crate::{
    config::BatchConfig, error::PoolError, AddedTransactionOutcome, PoolTransaction,
    TransactionOrigin, TransactionPool,
};
use pin_project::pin_project;
use std::{
    future::Future,
    pin::Pin,
    task::{ready, Context, Poll},
    time::Duration,
};
use tokio::{
    sync::{mpsc, oneshot},
    time::Interval,
};

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
/// - **Immediate mode** (`batch_timeout = None`): Processes requests as soon as available (greedy)
/// - **Batch-and-timeout mode** (`batch_timeout = Some(...)`): Waits for `max_batch_size` OR
///   timeout before processing
#[pin_project]
pub struct BatchTxProcessor<Pool: TransactionPool> {
    pool: Pool,
    max_batch_size: usize,
    buf: Vec<BatchTxRequest<Pool::Transaction>>,
    #[pin]
    request_rx: mpsc::UnboundedReceiver<BatchTxRequest<Pool::Transaction>>,
    /// Optional interval for batch timeout. None = immediate mode (zero-cost)
    #[pin]
    interval: Option<Interval>,
}

impl<Pool> std::fmt::Debug for BatchTxProcessor<Pool>
where
    Pool: TransactionPool + std::fmt::Debug,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BatchTxProcessor")
            .field("pool", &self.pool)
            .field("max_batch_size", &self.max_batch_size)
            .field("buf_len", &self.buf.len())
            .field("has_interval", &self.interval.is_some())
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
        let BatchConfig { max_batch_size, batch_timeout } = config;

        // Only create interval if timeout is provided
        let interval = batch_timeout.map(|timeout| {
            let mut interval = tokio::time::interval(timeout);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            interval
        });

        let processor = Self {
            pool,
            max_batch_size,
            buf: Vec::with_capacity(max_batch_size),
            request_rx,
            interval,
        };

        const fn as_ms(d: Option<Duration>) -> u64 {
            if let Some(d) = d {
                d.as_millis() as u64
            } else {
                0
            }
        }

        tracing::info!(
            target: "txpool::batcher",
            max_batch_size,
            batch_timeout_ms = as_ms(batch_timeout),
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

        let (transactions, response_txs): (Vec<_>, Vec<_>) =
            batch.into_iter().map(|req| ((req.origin, req.pool_tx), req.response_tx)).unzip();

        let pool_results = pool.add_transactions_with_origins(transactions).await;
        for (response_tx, pool_result) in response_txs.into_iter().zip(pool_results) {
            let _ = response_tx.send(pool_result);
        }
    }

    /// Spawn a batch processing task
    fn spawn_batch(pool: &Pool, buf: &mut Vec<BatchTxRequest<Pool::Transaction>>) {
        if buf.is_empty() {
            return;
        }
        let batch = std::mem::take(buf);
        let pool = pool.clone();
        tokio::spawn(async move {
            Self::process_batch(&pool, batch).await;
        });
    }
}

impl<Pool> Future for BatchTxProcessor<Pool>
where
    Pool: TransactionPool + 'static,
{
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut this = self.project();

        loop {
            // 1. Collect available requests using poll_recv_many (non-blocking)
            let remaining_capacity = this.max_batch_size.saturating_sub(this.buf.len());
            if remaining_capacity > 0 {
                match this.request_rx.as_mut().poll_recv_many(cx, this.buf, remaining_capacity) {
                    Poll::Ready(0) => {
                        // Channel closed, flush remaining and exit
                        Self::spawn_batch(this.pool, this.buf);
                        return Poll::Ready(())
                    }
                    Poll::Ready(_n) => {
                        // Received some items, continue to check conditions
                    }
                    Poll::Pending => {
                        // No items immediately available
                    }
                }
            }

            // 2. Early return if buffer is empty — no need to poll the interval since there is
            //    nothing buffered to flush
            if this.buf.is_empty() {
                return Poll::Pending
            }

            // 3. If batch is full, spawn immediately and continue (skip interval polling)
            if this.buf.len() >= *this.max_batch_size {
                Self::spawn_batch(this.pool, this.buf);
                // Reset interval if present
                if let Some(mut interval) = this.interval.as_mut().as_pin_mut() {
                    interval.as_mut().reset();
                }
                continue
            }

            // 4. Batch not full - check if we should flush based on mode
            if let Some(mut interval) = this.interval.as_mut().as_pin_mut() {
                // Batch-and-timeout mode: wait for interval tick before flushing.
                // If poll_tick returns Pending, we fall through to return Pending below.
                ready!(interval.as_mut().poll_tick(cx));
            }
            // Immediate mode (no interval) always falls through to flush.

            // Note: wakers are already registered by poll_recv_many and
            // interval.poll_tick returning Pending above.
            Self::spawn_batch(this.pool, this.buf);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::{testing_pool, MockTransaction};
    use futures::stream::{FuturesUnordered, StreamExt};
    use std::time::Duration;
    use tokio::time::timeout;

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
    async fn test_batch_processor() {
        let pool = testing_pool();
        let config = BatchConfig { max_batch_size: 1000, batch_timeout: None };
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
        let config = BatchConfig { max_batch_size: 1000, batch_timeout: None };
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
        let config = BatchConfig { max_batch_size, batch_timeout: None };
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

    /// Test that batch is flushed when timeout expires (even if batch is not full)
    #[tokio::test]
    async fn test_batch_timeout_triggers_flush() {
        let pool = testing_pool();
        // Large batch size, small timeout - timeout should trigger the flush
        let batch_timeout = Duration::from_millis(50);
        let config = BatchConfig { max_batch_size: 1000, batch_timeout: Some(batch_timeout) };
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
        let config = BatchConfig { max_batch_size, batch_timeout: Some(batch_timeout) };
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

    /// Test that zero timeout maintains original immediate processing behavior
    #[tokio::test]
    async fn test_zero_timeout_immediate_processing() {
        let pool = testing_pool();
        // Zero timeout (None) = immediate mode based on our logic (if configured that way)
        // But here we test explicit "None" configuration
        let config = BatchConfig { max_batch_size: 1000, batch_timeout: None };
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
            .expect("Zero timeout mode should process immediately")
            .expect("Response channel was closed unexpectedly");
        assert!(result.is_ok());

        handle.abort();
    }

    // ===== Manual Poll Tests =====

    /// Test that polling with empty buffer returns Pending
    #[tokio::test]
    async fn test_poll_empty_returns_pending() {
        use std::future::Future;

        let pool = testing_pool();
        let config =
            BatchConfig { max_batch_size: 10, batch_timeout: Some(Duration::from_secs(60)) };
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
        let config = BatchConfig { max_batch_size, batch_timeout: Some(Duration::from_secs(3600)) };
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

    /// Test that partial batch with interval waits for timeout (does not flush immediately)
    #[tokio::test]
    async fn test_poll_partial_batch_with_interval_waits() {
        use std::future::Future;

        let pool = testing_pool();
        let max_batch_size = 10;
        // Long timeout - batch should NOT flush immediately
        let config = BatchConfig { max_batch_size, batch_timeout: Some(Duration::from_secs(3600)) };
        let (processor, request_tx) = BatchTxProcessor::new(pool, config);

        let mut processor = Box::pin(processor);

        // Send fewer items than max_batch_size
        let tx = MockTransaction::legacy().with_nonce(0).with_gas_price(100);
        let (response_tx, mut response_rx) = tokio::sync::oneshot::channel();
        request_tx
            .send(BatchTxRequest::new(TransactionOrigin::Local, tx, response_tx))
            .expect("send failed");

        // Poll once - should return Pending (waiting for interval)
        let waker = futures::task::noop_waker();
        let mut cx = Context::from_waker(&waker);
        let result = processor.as_mut().poll(&mut cx);
        assert!(result.is_pending(), "Partial batch with interval should return Pending");

        // Response should NOT be ready yet (batch not flushed)
        let try_result = response_rx.try_recv();
        assert!(try_result.is_err(), "Partial batch should not be flushed immediately");
    }

    /// Test that partial batch in immediate mode (no interval) flushes right away
    #[tokio::test]
    async fn test_poll_partial_batch_immediate_mode_flushes() {
        use std::future::Future;

        let pool = testing_pool();
        let max_batch_size = 10;
        // No timeout = immediate mode
        let config = BatchConfig { max_batch_size, batch_timeout: None };
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
        let config = BatchConfig { max_batch_size, batch_timeout: Some(Duration::from_secs(3600)) };
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
