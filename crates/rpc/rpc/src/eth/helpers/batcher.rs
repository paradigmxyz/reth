//! Transaction batching for high-throughput RPC scenarios
//!
//! This module provides transaction batching to reduce lock contention when processing
//! many concurrent `send_raw_transaction` calls.

use alloy_primitives::B256;
use reth_errors::RethError;
use reth_rpc_eth_types::EthApiError;
use reth_transaction_pool::{
    AddedTransactionOutcome, PoolTransaction, TransactionOrigin, TransactionPool,
};
use std::{
    sync::atomic::{AtomicU64, Ordering},
    sync::Arc,
    time::Duration,
};
use tokio::sync::{mpsc, oneshot};
use tracing::trace;

/// Configuration for tx pool batch insertion
#[derive(Debug, Clone)]
pub struct TxBatchConfig {
    /// Interval between processing batches
    pub batch_interval: Duration,
    /// Channel buffer size for incoming batch tx requests
    pub channel_buffer_size: usize,
    /// Number of transactions that triggers immediate batch processing
    pub batch_threshold: usize,
}

impl Default for TxBatchConfig {
    fn default() -> Self {
        Self {
            batch_interval: Duration::from_millis(5),
            channel_buffer_size: 5000,
            batch_threshold: 1000,
        }
    }
}

/// A single batch transaction request
///  All transactions processed through the batcher are considered local
/// transactions (`TransactionOrigin::Local`) when inserted into the pool.
#[derive(Debug)]
pub struct BatchTxRequest<T: PoolTransaction> {
    /// Tx to be inserted in to the pool
    pool_tx: T,
    /// Channel to send result back to caller
    response_tx: oneshot::Sender<Result<B256, EthApiError>>,
}

impl<T> BatchTxRequest<T>
where
    T: PoolTransaction,
{
    /// Create a new batch transaction request
    pub const fn new(pool_tx: T, response_tx: oneshot::Sender<Result<B256, EthApiError>>) -> Self {
        Self { pool_tx, response_tx }
    }
}

/// Transaction batcher responsible for batch inserting txs into the pool
#[derive(Debug)]
pub struct TxBatcher<Pool: TransactionPool> {
    /// Pool for tx insertions
    pub pool: Pool,
    /// Channel for batch tx requests
    pub request_tx: mpsc::Sender<BatchTxRequest<Pool::Transaction>>,
    /// Atomic counter for pending requests
    pub pending_count: Arc<AtomicU64>,
}

impl<Pool> TxBatcher<Pool>
where
    Pool: TransactionPool + Clone + Send + Sync + 'static,
    Pool::Transaction: Send + Sync,
{
    /// Create a new `TxBatcher`
    pub fn new(
        pool: Pool,
        channel_buffer_size: usize,
    ) -> (Self, mpsc::Receiver<BatchTxRequest<Pool::Transaction>>) {
        let (request_tx, request_rx) = mpsc::channel(channel_buffer_size);
        let pending_count = Arc::new(AtomicU64::new(0));

        let batcher = Self { pool, request_tx, pending_count };
        (batcher, request_rx)
    }

    /// Pending batch transactions
    pub fn pending_count(&self) -> Arc<AtomicU64> {
        self.pending_count.clone()
    }

    /// Add transaction to the pool via batching
    pub async fn add_transaction(&self, pool_tx: Pool::Transaction) -> Result<B256, EthApiError> {
        let (response_tx, response_rx) = oneshot::channel();
        let request = BatchTxRequest::new(pool_tx, response_tx);

        self.request_tx.send(request).await.map_err(|_| {
            EthApiError::Internal(RethError::Other("Transaction batcher tx closed".into()))
        })?;

        // Increment pending count after successful send
        self.pending_count.fetch_add(1, Ordering::SeqCst);
        dbg!(&self.pending_count.load(Ordering::SeqCst));

        response_rx.await.map_err(|_| {
            EthApiError::Internal(RethError::Other("Transaction response rx closed".into()))
        })?
    }

    /// Process batch transaction insertions
    pub async fn process_batches(
        pool: Pool,
        batch_interval: Duration,
        batch_threshold: usize,
        pending_count: Arc<AtomicU64>,
        mut request_rx: mpsc::Receiver<BatchTxRequest<Pool::Transaction>>,
    ) {
        let mut interval = tokio::time::interval(batch_interval);

        loop {
            tokio::select! {
                // Check for timeout
                _ = interval.tick() => {
                    // Drain all pending requests from the channel
                    let mut batch = Vec::new();
                    while let Ok(request) = request_rx.try_recv() {
                        batch.push(request);
                    }

                    if !batch.is_empty() {
                        trace!(batch_size = batch.len(), "Processing batch due to timeout");
                        pending_count.store(0, Ordering::SeqCst);
                        Self::process_batch(&pool, batch).await;
                    }
                }
                // Process if threshold reached
                _ = async {
                    loop {
                        tokio::task::yield_now().await;
                        let count = pending_count.load(Ordering::SeqCst);
                        dbg!(count, batch_threshold);
                        if count >= batch_threshold as u64 {
                            break;
                        }
                    }
                } => {
                    // Drain all pending requests from the channel
                    let mut batch = Vec::new();
                    while let Ok(request) = request_rx.try_recv() {
                        batch.push(request);
                    }

                    if !batch.is_empty() {
                        trace!(batch_size = batch.len(), "Processing batch due to threshold");
                        pending_count.store(0, Ordering::SeqCst);
                        Self::process_batch(&pool, batch).await;
                    }
                }
            }
        }
    }

    /// Process a batch of transaction requests, grouped by origin
    async fn process_batch(pool: &Pool, batch: Vec<BatchTxRequest<Pool::Transaction>>) {
        let batch_size = batch.len();
        trace!(target = "", batch_size, "Processing batch");

        // NOTE: remove clone
        let pool_transactions = batch.iter().map(|req| req.pool_tx.clone()).collect();
        let pool_results = pool.add_transactions(TransactionOrigin::Local, pool_transactions).await;

        for (request, pool_result) in batch.into_iter().zip(pool_results) {
            let final_result = match pool_result {
                Ok(AddedTransactionOutcome { hash, .. }) => Ok(hash),
                Err(e) => Err(EthApiError::from(e)),
            };

            request.response_tx.send(final_result).expect("TODO: handle errror");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::stream::{FuturesUnordered, StreamExt};
    use reth_transaction_pool::test_utils::{testing_pool, MockTransaction};
    use std::time::Duration;
    use tokio::time::timeout;

    #[tokio::test]
    async fn test_process_batch() {
        let pool = testing_pool();

        let mut batch_requests = Vec::new();
        let mut responses = Vec::new();
        //
        for i in 0..100 {
            let tx = MockTransaction::legacy().with_nonce(i).with_gas_price(100);
            let (response_tx, response_rx) = tokio::sync::oneshot::channel();

            batch_requests.push(BatchTxRequest::new(tx, response_tx));
            responses.push(response_rx);
        }

        TxBatcher::process_batch(&pool, batch_requests).await;

        for response_rx in responses {
            let result = timeout(Duration::from_millis(5), response_rx)
                .await
                .expect("Timeout waiting for response")
                .expect("Response channel was closed unexpectedly");
            assert!(result.is_ok());
        }
    }

    #[tokio::test]
    async fn test_process_batches() {
        let pool = testing_pool();
        let (request_tx, request_rx) = tokio::sync::mpsc::channel(100);
        let interval = Duration::from_millis(5);

        // Spawn task to process batch requests
        let pool_clone = pool.clone();
        let pending_count = Arc::new(AtomicU64::new(0));
        let handle = tokio::spawn(async move {
            TxBatcher::<_>::process_batches(pool_clone, interval, 1000, pending_count, request_rx)
                .await;
        });

        let mut responses = Vec::new();

        for i in 0..50 {
            let tx = MockTransaction::legacy().with_nonce(i).with_gas_price(100);
            let (response_tx, response_rx) = tokio::sync::oneshot::channel();

            request_tx.send(BatchTxRequest::new(tx, response_tx)).await.unwrap();
            responses.push(response_rx);
        }

        // Wait for interval to process first batch
        tokio::time::sleep(Duration::from_millis(5)).await;

        // Verify all responses are received and successful
        for rx in responses {
            let result = timeout(Duration::from_millis(5), rx)
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
        let (batcher, request_rx) = TxBatcher::new(pool.clone(), 100);

        // Spawn task to process batch requests
        let pool_clone = pool.clone();
        let pending_count = batcher.pending_count();
        let handle = tokio::spawn(async move {
            TxBatcher::<_>::process_batches(
                pool_clone,
                Duration::from_millis(5),
                1000,
                pending_count,
                request_rx,
            )
            .await;
        });

        let mut results = Vec::new();
        for i in 0..10 {
            let tx = MockTransaction::legacy().with_nonce(i).with_gas_price(100);
            let result_future = batcher.add_transaction(tx);
            results.push(result_future);
        }

        for res in results {
            let result = timeout(Duration::from_millis(10), res)
                .await
                .expect("Timeout waiting for transaction result");
            assert!(result.is_ok());
        }

        drop(batcher);
        handle.abort();
    }

    #[tokio::test]
    async fn test_batch_threshold() {
        let pool = testing_pool();
        let (batcher, request_rx) = TxBatcher::new(pool.clone(), 100);

        let pool_clone = pool.clone();
        let batch_threshold = 10;
        let pending_count = batcher.pending_count();

        // Spawn batch processor with long batch interval
        let handle = tokio::spawn(async move {
            TxBatcher::<_>::process_batches(
                pool_clone,
                Duration::from_secs(1),
                batch_threshold,
                pending_count,
                request_rx,
            )
            .await;
        });

        let mut futures = FuturesUnordered::new();
        for i in 0..batch_threshold {
            dbg!(i);
            let tx = MockTransaction::legacy().with_nonce(i as u64).with_gas_price(100);
            let tx_fut = batcher.add_transaction(tx);
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
