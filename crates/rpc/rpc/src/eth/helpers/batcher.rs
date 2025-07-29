//! Transaction batching for high-throughput RPC scenarios
//!
//! This module provides transaction batching to reduce lock contention when processing
//! many concurrent `send_raw_transaction` calls.

use std::{
    collections::{BTreeMap, HashMap, VecDeque},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use itertools::Itertools;

use alloy_primitives::B256;
use reth_rpc_eth_types::EthApiError;
use reth_transaction_pool::{
    AddedTransactionOutcome, PoolTransaction, TransactionOrigin, TransactionPool,
};
use tokio::{
    sync::{mpsc, oneshot},
    task::JoinHandle,
    time::Interval,
};
use tracing::{debug, trace, warn};

/// Configuration for tx pool batch insertion
#[derive(Debug, Clone)]
pub struct TxBatchConfig {
    /// Maximum transactions per batch
    pub max_batch_size: usize,
    /// Maximum time to wait before processing batch
    pub max_wait_time: Duration,
    /// Channel buffer size for incoming batch tx requests
    pub channel_buffer_size: usize,
}

impl Default for TxBatchConfig {
    fn default() -> Self {
        Self {
            max_batch_size: 1000,
            max_wait_time: Duration::from_millis(5),
            channel_buffer_size: 5000,
        }
    }
}

/// A single batch transaction request
#[derive(Debug)]
struct BatchTxRequest<T: PoolTransaction> {
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
    /// Channel for sending batch requests
    request_tx: mpsc::Sender<BatchTxRequest<Pool::Transaction>>,
    /// Handle to the background batch processor task
    processor_handle: JoinHandle<()>,
}

impl<Pool> TxBatcher<Pool>
where
    Pool: TransactionPool + Clone + Send + Sync + 'static,
    Pool::Transaction: Send + Sync,
{
    /// Create a new `TxBatcher`
    pub fn new(pool: Pool, config: TxBatchConfig) -> Self {
        let (request_tx, request_rx) = mpsc::channel(config.channel_buffer_size);

        // Spawn the background batching task
        let interval = tokio::time::interval(config.max_wait_time);
        let processor_handle = Self::spawn_batch_processor(pool, interval, request_rx);

        Self { request_tx, processor_handle }
    }

    /// Add transaction to the pool via batching
    pub async fn add_transaction(
        &self,
        origin: TransactionOrigin,
        pool_tx: Pool::Transaction,
    ) -> Result<B256, EthApiError> {
        let (response_tx, response_rx) = oneshot::channel();
        let request = BatchTxRequest { pool_tx, origin, response_tx };

        self.request_tx.send(request).await.expect("TODO: handle error");

        response_rx.await.expect("TODO: handle error ")
    }

    /// Spawns the batch tx processor
    fn spawn_batch_processor(
        pool: Pool,
        mut interval: Interval,
        mut request_rx: mpsc::Receiver<BatchTxRequest<Pool::Transaction>>,
    ) -> JoinHandle<()> {
        tokio::spawn(async move {
            loop {
                interval.tick().await;

                // Drain all pending requests from the channel
                let mut batch = Vec::new();
                while let Ok(request) = request_rx.try_recv() {
                    batch.push(request);
                }

                if !batch.is_empty() {
                    trace!(batch_size = batch.len(), "Processing drained batch");
                    Self::process_batch(&pool, batch).await;
                }
            }
        })
    }

    /// Process a batch of transaction requests, grouped by origin
    async fn process_batch(pool: &Pool, batch: Vec<BatchTxRequest<Pool::Transaction>>) {
        let origin_groups = batch.into_iter().into_group_map_by(|request| request.origin);

        // Process each origin group separately
        for (origin, requests) in origin_groups {
            let batch_size = requests.len();
            trace!(origin = ?origin, batch_size, "Processing origin batch");

            let pool_transactions: Vec<Pool::Transaction> =
                requests.iter().map(|req| req.pool_tx.clone()).collect();

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

impl<Pool: TransactionPool> Drop for TxBatcher<Pool> {
    fn drop(&mut self) {
        self.processor_handle.abort();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reth_transaction_pool::test_utils::testing_pool;
    use std::time::Duration;
}
