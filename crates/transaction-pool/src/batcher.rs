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

/// A single batch transaction request
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
    request_rx: mpsc::UnboundedReceiver<BatchTxRequest<Pool::Transaction>>,
}

impl<Pool> BatchTxProcessor<Pool>
where
    Pool: TransactionPool + 'static,
{
    /// Create a new `BatchTxProcessor`
    pub fn new(
        pool: Pool,
        max_batch_size: usize,
    ) -> (Self, mpsc::UnboundedSender<BatchTxRequest<Pool::Transaction>>) {
        let (request_tx, request_rx) = mpsc::unbounded_channel();

        let processor = Self { pool, max_batch_size, buf: Vec::with_capacity(1), request_rx };

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
            ready!(this.request_rx.poll_recv_many(cx, this.buf, *this.max_batch_size));

            if !this.buf.is_empty() {
                let batch = std::mem::take(this.buf);
                let pool = this.pool.clone();
                tokio::spawn(async move {
                    Self::process_batch(&pool, batch).await;
                });
                this.buf.reserve(1);

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
        let (processor, request_tx) = BatchTxProcessor::new(pool.clone(), 1000);

        // Spawn the processor
        let handle = tokio::spawn(processor);

        let mut responses = Vec::new();

        for i in 0..50 {
            let tx = MockTransaction::legacy().with_nonce(i).with_gas_price(100);
            let (response_tx, response_rx) = tokio::sync::oneshot::channel();

            request_tx.send(BatchTxRequest::new(tx, response_tx)).expect("Could not send batch tx");
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
        let (processor, request_tx) = BatchTxProcessor::new(pool.clone(), 1000);

        // Spawn the processor
        let handle = tokio::spawn(processor);

        let mut results = Vec::new();
        for i in 0..10 {
            let tx = MockTransaction::legacy().with_nonce(i).with_gas_price(100);
            let (response_tx, response_rx) = tokio::sync::oneshot::channel();
            let request = BatchTxRequest::new(tx, response_tx);
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
        let (processor, request_tx) = BatchTxProcessor::new(pool.clone(), max_batch_size);

        // Spawn batch processor with threshold
        let handle = tokio::spawn(processor);

        let mut futures = FuturesUnordered::new();
        for i in 0..max_batch_size {
            let tx = MockTransaction::legacy().with_nonce(i as u64).with_gas_price(100);
            let (response_tx, response_rx) = tokio::sync::oneshot::channel();
            let request = BatchTxRequest::new(tx, response_tx);
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
}
