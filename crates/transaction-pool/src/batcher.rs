//! Transaction batching for `Pool` insertion for high-throughput scenarios
//!
//! This module provides transaction batching logic to reduce lock contention when processing
//! many concurrent transaction pool insertions.

use crate::{AddedTransactionOutcome, PoolTransaction, TransactionOrigin, TransactionPool};
use alloy_primitives::B256;
use pin_project::pin_project;
use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};
use tokio::sync::{mpsc, oneshot};

/// Configuration for tx pool batch insertion
#[derive(Debug, Clone)]
pub struct TxBatchConfig {
    /// Channel buffer size for incoming batch tx requests
    pub channel_buffer_size: usize,
    /// Number of transactions that triggers immediate batch processing
    pub batch_threshold: usize,
}

impl Default for TxBatchConfig {
    fn default() -> Self {
        Self { channel_buffer_size: 5000, batch_threshold: 1000 }
    }
}

/// Error type for transaction batching operations
#[derive(Debug, thiserror::Error)]
pub enum TxBatchError {
    /// Transaction batcher channel closed
    #[error("Transaction batcher channel closed")]
    BatcherChannelClosed,
    /// Transaction response channel closed
    #[error("Transaction response channel closed")]
    ResponseChannelClosed,
}

/// A single batch transaction request
/// All transactions processed through the batcher are considered local
/// transactions (`TransactionOrigin::Local`) when inserted into the pool.
#[derive(Debug)]
pub struct BatchTxRequest<T: PoolTransaction> {
    /// Tx to be inserted in to the pool
    pool_tx: T,
    /// Channel to send result back to caller
    response_tx: oneshot::Sender<Result<B256, TxBatchError>>,
}

impl<T> BatchTxRequest<T>
where
    T: PoolTransaction,
{
    /// Create a new batch transaction request
    pub const fn new(pool_tx: T, response_tx: oneshot::Sender<Result<B256, TxBatchError>>) -> Self {
        Self { pool_tx, response_tx }
    }
}

/// Transaction batch processor that handles batch processing
#[pin_project]
#[derive(Debug)]
pub struct TxBatchProcessor<Pool: TransactionPool> {
    pool: Pool,
    max_batch_size: usize,
    channel_buffer_size: usize,
    #[pin]
    request_rx: mpsc::Receiver<BatchTxRequest<Pool::Transaction>>,
}

impl<Pool> TxBatchProcessor<Pool>
where
    Pool: TransactionPool + Clone + Send + Sync + 'static,
    Pool::Transaction: Send + Sync,
{
    /// Create a new `TxBatchProcessor`
    pub fn new(
        pool: Pool,
        max_batch_size: usize,
        channel_buffer_size: usize,
    ) -> (Self, mpsc::Sender<BatchTxRequest<Pool::Transaction>>) {
        let (request_tx, request_rx) = mpsc::channel(channel_buffer_size);

        let processor = Self { pool, max_batch_size, channel_buffer_size, request_rx };

        (processor, request_tx)
    }

    /// Process a batch of transaction requests, grouped by origin
    async fn process_batch(pool: &Pool, batch: Vec<BatchTxRequest<Pool::Transaction>>) {
        let (pool_transactions, response_tx): (Vec<_>, Vec<_>) =
            batch.into_iter().map(|req| (req.pool_tx, req.response_tx)).unzip();

        let pool_results = pool.add_transactions(TransactionOrigin::Local, pool_transactions).await;

        for (response_tx, pool_result) in response_tx.into_iter().zip(pool_results) {
            let final_result = match pool_result {
                Ok(AddedTransactionOutcome { hash, .. }) => Ok(hash),
                Err(_) => Err(TxBatchError::BatcherChannelClosed),
            };

            let _ = response_tx.send(final_result);
        }
    }
}

impl<Pool> Future for TxBatchProcessor<Pool>
where
    Pool: TransactionPool + Clone + Send + Sync + 'static,
    Pool::Transaction: Send + Sync,
{
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut this = self.project();

        loop {
            // Drain all available requests from the receiver
            let mut batch = Vec::with_capacity(*this.channel_buffer_size);
            while let Poll::Ready(Some(request)) = this.request_rx.poll_recv(cx) {
                batch.push(request);

                // Check if the max batch size threshold has been reached
                if batch.len() >= *this.max_batch_size {
                    break;
                }
            }

            if !batch.is_empty() {
                let pool = this.pool.clone();
                tokio::spawn(async move {
                    Self::process_batch(&pool, batch).await;
                });

                continue;
            }

            // No requests available, return Pending to wait for more
            return Poll::Pending;
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

        TxBatchProcessor::process_batch(&pool, batch_requests).await;

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
        let (processor, request_tx) = TxBatchProcessor::new(pool.clone(), 1000, 100);

        // Spawn the processor
        let handle = tokio::spawn(processor);

        let mut responses = Vec::new();

        for i in 0..50 {
            let tx = MockTransaction::legacy().with_nonce(i).with_gas_price(100);
            let (response_tx, response_rx) = tokio::sync::oneshot::channel();

            request_tx.send(BatchTxRequest::new(tx, response_tx)).await.unwrap();
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
        let (processor, request_tx) = TxBatchProcessor::new(pool.clone(), 1000, 100);

        // Spawn the processor
        let handle = tokio::spawn(processor);

        let mut results = Vec::new();
        for i in 0..10 {
            let tx = MockTransaction::legacy().with_nonce(i).with_gas_price(100);
            let (response_tx, response_rx) = tokio::sync::oneshot::channel();
            let request = BatchTxRequest::new(tx, response_tx);
            request_tx.send(request).await.unwrap();
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
    async fn test_batch_threshold() {
        let pool = testing_pool();
        let batch_threshold = 10;
        let (processor, request_tx) = TxBatchProcessor::new(pool.clone(), batch_threshold, 100);

        // Spawn batch processor with threshold
        let handle = tokio::spawn(processor);

        let mut futures = FuturesUnordered::new();
        for i in 0..batch_threshold {
            let tx = MockTransaction::legacy().with_nonce(i as u64).with_gas_price(100);
            let (response_tx, response_rx) = tokio::sync::oneshot::channel();
            let request = BatchTxRequest::new(tx, response_tx);
            let request_tx_clone = request_tx.clone();

            let tx_fut = async move {
                request_tx_clone.send(request).await.unwrap();
                response_rx.await.unwrap()
            };
            futures.push(tx_fut);
        }

        while let Some(result) = timeout(Duration::from_millis(50), futures.next())
            .await
            .expect("Timeout waiting for transaction result")
        {
            assert!(result.is_ok());
        }

        handle.abort();
    }
}
