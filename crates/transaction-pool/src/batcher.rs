//! Transaction batching for `Pool` insertion for high-throughput scenarios
//!
//! This module provides transaction batching logic to reduce lock contention when processing
//! many concurrent transaction pool insertions.

use crate::{
    error::PoolError, AddedTransactionOutcome, PoolTransaction, TransactionOrigin, TransactionPool,
};
use pin_project::pin_project;
use std::{
    future::Future,
    pin::Pin,
    task::{ready, Context, Poll},
};
use tokio::sync::{mpsc, oneshot};

/// Default capacity for the transaction batch request channel.
///
/// This bounds the number of pending batch requests to prevent unbounded memory growth
/// under denial-of-service attacks where malicious users blast large transactions and quickly
/// cancel requests. The value of 2048 provides enough headroom for burst scenarios while ensuring
/// backpressure is applied before memory usage becomes problematic.
pub const DEFAULT_BATCH_CHANNEL_CAPACITY: usize = 2048;

/// Default maximum batch size for processing transactions.
pub const DEFAULT_MAX_BATCH_SIZE: usize = 1000;

/// Configuration for the transaction batch processor.
#[derive(Debug, Clone, Copy)]
pub struct BatchConfig {
    /// Maximum number of transactions to process in a single batch.
    pub max_batch_size: usize,
    /// Capacity of the bounded channel for pending batch requests.
    ///
    /// This limits memory usage and provides backpressure under high load.
    pub channel_capacity: usize,
}

impl Default for BatchConfig {
    fn default() -> Self {
        Self {
            max_batch_size: DEFAULT_MAX_BATCH_SIZE,
            channel_capacity: DEFAULT_BATCH_CHANNEL_CAPACITY,
        }
    }
}

impl BatchConfig {
    /// Creates a new batch configuration with the given settings.
    pub const fn new(max_batch_size: usize, channel_capacity: usize) -> Self {
        Self { max_batch_size, channel_capacity }
    }

    /// Sets the maximum batch size.
    pub const fn with_max_batch_size(mut self, max_batch_size: usize) -> Self {
        self.max_batch_size = max_batch_size;
        self
    }

    /// Sets the channel capacity.
    pub const fn with_channel_capacity(mut self, channel_capacity: usize) -> Self {
        self.channel_capacity = channel_capacity;
        self
    }
}

/// A single batch transaction request.
///
/// All transactions processed through the batcher are considered local
/// transactions (`TransactionOrigin::Local`) when inserted into the pool.
#[derive(Debug)]
pub struct BatchTxRequest<T: PoolTransaction> {
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
        pool_tx: T,
        response_tx: oneshot::Sender<Result<AddedTransactionOutcome, PoolError>>,
    ) -> Self {
        Self { pool_tx, response_tx }
    }
}

/// Transaction batch processor that handles batch processing
#[pin_project]
#[derive(Debug)]
pub struct BatchTxProcessor<Pool: TransactionPool> {
    pool: Pool,
    max_batch_size: usize,
    buf: Vec<BatchTxRequest<Pool::Transaction>>,
    #[pin]
    request_rx: mpsc::Receiver<BatchTxRequest<Pool::Transaction>>,
}

impl<Pool> BatchTxProcessor<Pool>
where
    Pool: TransactionPool + 'static,
{
    /// Create a new `BatchTxProcessor` with the given configuration.
    pub fn new(
        pool: Pool,
        config: BatchConfig,
    ) -> (Self, mpsc::Sender<BatchTxRequest<Pool::Transaction>>) {
        let (request_tx, request_rx) = mpsc::channel(config.channel_capacity);

        let processor = Self {
            pool,
            max_batch_size: config.max_batch_size,
            buf: Vec::with_capacity(1),
            request_rx,
        };

        (processor, request_tx)
    }

    async fn process_request(pool: &Pool, req: BatchTxRequest<Pool::Transaction>) {
        let BatchTxRequest { pool_tx, response_tx } = req;
        let pool_result = pool.add_transaction(TransactionOrigin::Local, pool_tx).await;
        let _ = response_tx.send(pool_result);
    }

    /// Process a batch of transaction requests, grouped by origin
    async fn process_batch(pool: &Pool, mut batch: Vec<BatchTxRequest<Pool::Transaction>>) {
        if batch.len() == 1 {
            Self::process_request(pool, batch.remove(0)).await;
            return
        }

        let (pool_transactions, response_tx): (Vec<_>, Vec<_>) =
            batch.into_iter().map(|req| (req.pool_tx, req.response_tx)).unzip();

        let pool_results = pool.add_transactions(TransactionOrigin::Local, pool_transactions).await;

        for (response_tx, pool_result) in response_tx.into_iter().zip(pool_results) {
            let _ = response_tx.send(pool_result);
        }
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
            // Drain all available requests from the receiver
            let n = ready!(this.request_rx.poll_recv_many(cx, this.buf, *this.max_batch_size));

            // Channel closed and no remaining items - shutdown gracefully
            if n == 0 && this.buf.is_empty() {
                return Poll::Ready(());
            }

            if !this.buf.is_empty() {
                let batch = std::mem::take(this.buf);
                let pool = this.pool.clone();
                tokio::spawn(async move {
                    Self::process_batch(&pool, batch).await;
                });
                this.buf.reserve(1);
            }
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

            batch_requests.push(BatchTxRequest::new(tx, response_tx));
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
        let config = BatchConfig::default().with_max_batch_size(1000);
        let (processor, request_tx) = BatchTxProcessor::new(pool.clone(), config);

        // Spawn the processor
        let handle = tokio::spawn(processor);

        let mut responses = Vec::new();

        for i in 0..50 {
            let tx = MockTransaction::legacy().with_nonce(i).with_gas_price(100);
            let (response_tx, response_rx) = tokio::sync::oneshot::channel();

            request_tx
                .try_send(BatchTxRequest::new(tx, response_tx))
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
        let config = BatchConfig::default().with_max_batch_size(1000);
        let (processor, request_tx) = BatchTxProcessor::new(pool.clone(), config);

        // Spawn the processor
        let handle = tokio::spawn(processor);

        let mut results = Vec::new();
        for i in 0..10 {
            let tx = MockTransaction::legacy().with_nonce(i).with_gas_price(100);
            let (response_tx, response_rx) = tokio::sync::oneshot::channel();
            let request = BatchTxRequest::new(tx, response_tx);
            request_tx.try_send(request).expect("Could not send batch tx");
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
        let config = BatchConfig::default().with_max_batch_size(max_batch_size);
        let (processor, request_tx) = BatchTxProcessor::new(pool.clone(), config);

        // Spawn batch processor with threshold
        let handle = tokio::spawn(processor);

        let mut futures = FuturesUnordered::new();
        for i in 0..max_batch_size {
            let tx = MockTransaction::legacy().with_nonce(i as u64).with_gas_price(100);
            let (response_tx, response_rx) = tokio::sync::oneshot::channel();
            let request = BatchTxRequest::new(tx, response_tx);
            let request_tx_clone = request_tx.clone();

            let tx_fut = async move {
                request_tx_clone.try_send(request).expect("Could not send batch tx");
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
}
