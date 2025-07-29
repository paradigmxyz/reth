//! Transaction batching for high-throughput RPC scenarios
//!
//! This module provides transaction batching to reduce lock contention when processing
//! many concurrent `send_raw_transaction` calls.

use alloy_primitives::B256;
use itertools::Itertools;
use reth_errors::RethError;
use reth_rpc_eth_types::EthApiError;
use reth_tasks::TaskSpawner;
use reth_transaction_pool::{
    AddedTransactionOutcome, PoolTransaction, TransactionOrigin, TransactionPool,
};
use std::time::Duration;
use tokio::{
    sync::{mpsc, oneshot},
    time::Interval,
};
use tracing::trace;

/// Configuration for tx pool batch insertion
#[derive(Debug, Clone)]
pub struct TxBatchConfig {
    /// Interval between processing batches
    pub interval: Duration,
    /// Channel buffer size for incoming batch tx requests
    pub channel_buffer_size: usize,
}

impl Default for TxBatchConfig {
    fn default() -> Self {
        Self { interval: Duration::from_millis(5), channel_buffer_size: 5000 }
    }
}

/// A single batch transaction request
#[derive(Debug)]
pub struct BatchTxRequest<T: PoolTransaction> {
    /// Tx to be inserted in to the pool
    pool_tx: T,
    /// Origin of the transaction
    origin: TransactionOrigin,
    /// Channel to send result back to caller
    response_tx: oneshot::Sender<Result<B256, EthApiError>>,
}

impl<T> BatchTxRequest<T>
where
    T: PoolTransaction,
{
    // TODO: new
}

/// Transaction batcher responsible for batch inserting txs into the pool
#[derive(Debug)]
pub struct TxBatcher<Pool: TransactionPool> {
    /// Pool for tx insertions
    pub pool: Pool,
    /// Channel for batch tx requests
    pub request_tx: mpsc::Sender<BatchTxRequest<Pool::Transaction>>,
    /// Batch insertion interval
    pub interval: Duration,
}

impl<Pool> TxBatcher<Pool>
where
    Pool: TransactionPool + Clone + Send + Sync + 'static,
    Pool::Transaction: Send + Sync,
{
    /// Create a new `TxBatcher`
    pub fn new(
        pool: Pool,
        interval: Duration,
        channel_buffer_size: usize,
    ) -> (Self, mpsc::Receiver<BatchTxRequest<Pool::Transaction>>) {
        let (request_tx, request_rx) = mpsc::channel(channel_buffer_size);

        let batcher = Self { pool, interval, request_tx };
        (batcher, request_rx)
    }

    /// Add transaction to the pool via batching
    pub async fn add_transaction(
        &self,
        origin: TransactionOrigin,
        pool_tx: Pool::Transaction,
    ) -> Result<B256, EthApiError> {
        let (response_tx, response_rx) = oneshot::channel();
        let request = BatchTxRequest { pool_tx, origin, response_tx };

        self.request_tx.send(request).await.map_err(|_| {
            EthApiError::Internal(RethError::Other("Transaction batcher tx closed".into()))
        })?;

        response_rx.await.map_err(|_| {
            EthApiError::Internal(RethError::Other("Transaction response rx closed".into()))
        })?
    }

    /// Process batch transaction insertions
    pub async fn process_batches(
        pool: Pool,
        interval: Duration,
        mut request_rx: mpsc::Receiver<BatchTxRequest<Pool::Transaction>>,
    ) {
        let mut interval = tokio::time::interval(interval);
        loop {
            interval.tick().await;

            // Drain all pending requests from the channel
            let mut batch = Vec::new();
            // TODO: drain without overhead
            while let Ok(request) = request_rx.try_recv() {
                batch.push(request);
            }

            if !batch.is_empty() {
                trace!(batch_size = batch.len(), "Processing drained batch");
                Self::process_batch(&pool, batch).await;
            }
        }
    }

    /// Process a batch of transaction requests, grouped by origin
    async fn process_batch(pool: &Pool, batch: Vec<BatchTxRequest<Pool::Transaction>>) {
        // Group requests by origin
        // TODO: can we simplify by knowing all txs will be marked as local
        let origin_groups = batch.into_iter().into_group_map_by(|request| request.origin);

        // Process each origin group separately
        for (origin, requests) in origin_groups {
            let batch_size = requests.len();
            trace!(origin = ?origin, batch_size, "Processing batch");

            // NOTE: remove clone
            let pool_transactions = requests.iter().map(|req| req.pool_tx.clone()).collect();
            let pool_results = pool.add_transactions(origin, pool_transactions).await;

            for (request, pool_result) in requests.into_iter().zip(pool_results) {
                let final_result = match pool_result {
                    Ok(AddedTransactionOutcome { hash, .. }) => Ok(hash),
                    Err(e) => Err(EthApiError::from(e)),
                };

                let _ = request.response_tx.send(final_result);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reth_transaction_pool::{
        test_utils::{testing_pool, MockTransaction, TransactionGenerator},
        TransactionOrigin,
    };
    use std::time::Duration;
    use tokio::time::timeout;

    #[tokio::test]
    async fn test_process_batch() {
        let pool = testing_pool();

        let mut batch_requests = Vec::new();
        let mut responses = Vec::new();
        //
        for i in 0..100 {
            let tx = MockTransaction::legacy().with_nonce(i);
            let (response_tx, response_rx) = tokio::sync::oneshot::channel();

            batch_requests.push(BatchTxRequest {
                pool_tx: tx,
                origin: TransactionOrigin::Local,
                response_tx,
            });
            responses.push(response_rx);
        }

        TxBatcher::process_batch(&pool, batch_requests).await;

        for response_rx in responses {
            let result = timeout(Duration::from_millis(100), response_rx).await.unwrap().unwrap();
            assert!(result.is_ok());
        }
    }

    #[test]
    fn test_process_batches() {}

    #[test]
    fn test_add_transaction() {}
}
