//! Transaction batching for high-throughput RPC scenarios
//!
//! This module provides transaction batching to reduce lock contention when processing
//! many concurrent `send_raw_transaction` calls.

use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

use alloy_primitives::B256;
use reth_rpc_eth_types::EthApiError;
use reth_transaction_pool::{
    AddedTransactionOutcome, PoolTransaction, TransactionOrigin, TransactionPool,
};
use tokio::{
    sync::{mpsc, oneshot},
    time::timeout,
};
use tower::hedge::Policy;
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
pub struct TxBatcher<Pool: TransactionPool> {
    /// Channel for sending batch requests
    request_tx: mpsc::Sender<BatchTxRequest<Pool::Transaction>>,
    /// Keep reference to pool for type safety
    _pool: std::marker::PhantomData<Pool>,
}

impl<Pool> TxBatcher<Pool>
where
    Pool: TransactionPool + Clone + Send + Sync + 'static,
    Pool::Transaction: Send + Sync,
{
    /// Create a new TxBatcher
    pub fn new(pool: Pool, config: TxBatchConfig) -> Self {
        let (request_tx, request_rx) = mpsc::channel(config.channel_buffer_size);
        // Spawn the background batching task
        // TODO: store handle
        tokio::spawn(Self::batch_processor(pool, config, request_rx));

        Self { request_tx, _pool: std::marker::PhantomData }
    }

    /// Submit a transaction for batch processing
    pub async fn submit_transaction(
        &self,
        pool_tx: Pool::Transaction,
    ) -> Result<B256, EthApiError> {
        let (response_tx, response_rx) = oneshot::channel();
        let request = BatchTxRequest { pool_tx, response_tx };

        // Send to batch processor
        self.request_tx.send(request).await.expect("TODO: handle error");

        // Wait for result
        response_rx.await.expect("TODO: handle error ")
    }

    /// Background task that processes batch insertions
    // TODO: spawn batch processors
    async fn batch_processor(
        pool: Pool,
        config: TxBatchConfig,
        mut request_rx: mpsc::Receiver<BatchTxRequest<Pool::Transaction>>,
    ) {
        todo!()
    }

    /// Process a batch of transaction requests
    async fn process_batch(pool: &Pool, batch: Vec<BatchTxRequest<Pool::Transaction>>) {
        todo!()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reth_transaction_pool::test_utils::testing_pool;
    use std::time::Duration;
}
