//! Support for maintaining the state of the transaction pool

/// The interval for which we check transaction against supervisor, 10 min.
const TRANSACTION_VALIDITY_WINDOW: u64 = 600;
/// Interval in seconds at which the transaction should be revalidated.
const OFFSET_TIME: u64 = 60;
/// Maximum number of supervisor requests at the same time
const MAX_SUPERVISOR_QUERIES: usize = 10;

use crate::{
    conditional::MaybeConditionalTransaction,
    interop::{is_stale_interop, is_valid_interop, MaybeInteropTransaction},
    supervisor::SupervisorClient,
};
use alloy_consensus::{conditional::BlockConditionalAttributes, BlockHeader};
use alloy_primitives::TxHash;
use futures_util::{future::BoxFuture, FutureExt, Stream, StreamExt};
use metrics::{Gauge, Histogram};
use reth_chain_state::CanonStateNotification;
use reth_metrics::{metrics::Counter, Metrics};
use reth_primitives_traits::NodePrimitives;
use reth_transaction_pool::{error::PoolTransactionError, PoolTransaction, TransactionPool};
use std::{collections::HashMap, time::Instant};
use tracing::warn;

/// Transaction pool maintenance metrics
#[derive(Metrics)]
#[metrics(scope = "transaction_pool")]
struct MaintainPoolConditionalMetrics {
    /// Counter indicating the number of conditional transactions removed from
    /// the pool because of exceeded block attributes.
    removed_tx_conditional: Counter,
}

impl MaintainPoolConditionalMetrics {
    #[inline]
    fn inc_removed_tx_conditional(&self, count: usize) {
        self.removed_tx_conditional.increment(count as u64);
    }
}

/// Transaction pool maintenance metrics
#[derive(Metrics)]
#[metrics(scope = "transaction_pool")]
struct MaintainPoolInteropMetrics {
    /// Counter indicating the number of conditional transactions removed from
    /// the pool because of exceeded block attributes.
    removed_tx_interop: Counter,
    /// Number of interop transactions currently in the pool
    pooled_interop_transactions: Gauge,

    /// Counter for interop transactions that became stale and need revalidation
    stale_interop_transactions: Counter,
    /// Histogram for measuring supervisor revalidation duration (congestion metric)
    supervisor_revalidation_duration_seconds: Histogram,

    /// Total number of supervisor revalidation attempts across all interop transactions
    revalidation_attempts_total: Counter,
    /// Histogram recording the number of revalidation attempts a tx had at the moment it was
    /// removed due to supervisor outcome or interop reclassification.
    revalidation_attempts_on_removal: Histogram,
    /// Number of interop transactions currently tracked for revalidation attempts
    revalidation_tracked_entries: Gauge,
}

impl MaintainPoolInteropMetrics {
    #[inline]
    fn inc_removed_tx_interop(&self, count: usize) {
        self.removed_tx_interop.increment(count as u64);
    }
    #[inline]
    fn set_interop_txs_in_pool(&self, count: usize) {
        self.pooled_interop_transactions.set(count as f64);
    }

    #[inline]
    fn inc_stale_tx_interop(&self, count: usize) {
        self.stale_interop_transactions.increment(count as u64);
    }

    /// Record supervisor revalidation duration
    #[inline]
    fn record_supervisor_duration(&self, duration: std::time::Duration) {
        self.supervisor_revalidation_duration_seconds.record(duration.as_secs_f64());
    }

    /// Increment total number of supervisor revalidation attempts
    #[inline]
    fn inc_revalidation_attempts(&self, count: usize) {
        self.revalidation_attempts_total.increment(count as u64);
    }

    /// Record how many attempts a transaction had when it was removed
    #[inline]
    fn record_attempts_on_removal(&self, attempts: u32) {
        self.revalidation_attempts_on_removal.record(attempts as f64);
    }

    /// Set current number of tracked entries for revalidation attempts
    #[inline]
    fn set_tracked_entries(&self, count: usize) {
        self.revalidation_tracked_entries.set(count as f64);
    }
}
/// Returns a spawnable future for maintaining the state of the conditional txs in the transaction
/// pool.
pub fn maintain_transaction_pool_conditional_future<N, Pool, St>(
    pool: Pool,
    events: St,
) -> BoxFuture<'static, ()>
where
    N: NodePrimitives,
    Pool: TransactionPool + 'static,
    Pool::Transaction: MaybeConditionalTransaction,
    St: Stream<Item = CanonStateNotification<N>> + Send + Unpin + 'static,
{
    async move {
        maintain_transaction_pool_conditional(pool, events).await;
    }
    .boxed()
}

/// Maintains the state of the conditional tx in the transaction pool by handling new blocks and
/// reorgs.
///
/// This listens for any new blocks and reorgs and updates the conditional txs in the
/// transaction pool's state accordingly
pub async fn maintain_transaction_pool_conditional<N, Pool, St>(pool: Pool, mut events: St)
where
    N: NodePrimitives,
    Pool: TransactionPool,
    Pool::Transaction: MaybeConditionalTransaction,
    St: Stream<Item = CanonStateNotification<N>> + Send + Unpin + 'static,
{
    let metrics = MaintainPoolConditionalMetrics::default();
    loop {
        let Some(event) = events.next().await else { break };
        if let CanonStateNotification::Commit { new } = event {
            let block_attr = BlockConditionalAttributes {
                number: new.tip().number(),
                timestamp: new.tip().timestamp(),
            };
            let mut to_remove = Vec::new();
            for tx in &pool.pooled_transactions() {
                if tx.transaction.has_exceeded_block_attributes(&block_attr) {
                    to_remove.push(*tx.hash());
                }
            }
            if !to_remove.is_empty() {
                let removed = pool.remove_transactions(to_remove);
                metrics.inc_removed_tx_conditional(removed.len());
            }
        }
    }
}

/// Returns a spawnable future for maintaining the state of the interop tx in the transaction pool.
pub fn maintain_transaction_pool_interop_future<N, Pool, St>(
    pool: Pool,
    events: St,
    supervisor_client: SupervisorClient,
) -> BoxFuture<'static, ()>
where
    N: NodePrimitives,
    Pool: TransactionPool + 'static,
    Pool::Transaction: MaybeInteropTransaction,
    St: Stream<Item = CanonStateNotification<N>> + Send + Unpin + 'static,
{
    async move {
        maintain_transaction_pool_interop(pool, events, supervisor_client).await;
    }
    .boxed()
}

/// Maintains the state of the interop tx in the transaction pool by handling new blocks and reorgs.
///
/// This listens for any new blocks and reorgs and updates the interop tx in the transaction pool's
/// state accordingly
pub async fn maintain_transaction_pool_interop<N, Pool, St>(
    pool: Pool,
    mut events: St,
    supervisor_client: SupervisorClient,
) where
    N: NodePrimitives,
    Pool: TransactionPool,
    Pool::Transaction: MaybeInteropTransaction,
    St: Stream<Item = CanonStateNotification<N>> + Send + Unpin + 'static,
{
    let metrics = MaintainPoolInteropMetrics::default();
    let mut revalidation_attempts: HashMap<TxHash, u32> = HashMap::new();

    loop {
        let Some(event) = events.next().await else { break };
        if let CanonStateNotification::Commit { new } = event {
            let timestamp = new.tip().timestamp();
            let mut to_remove = Vec::new();
            let mut to_revalidate = Vec::new();
            let mut interop_count = 0;

            // scan all pooled interop transactions
            for pooled_tx in pool.pooled_transactions() {
                if let Some(interop_deadline_val) = pooled_tx.transaction.interop_deadline() {
                    interop_count += 1;
                    if !is_valid_interop(interop_deadline_val, timestamp) {
                        to_remove.push(*pooled_tx.transaction.hash());
                    } else if is_stale_interop(interop_deadline_val, timestamp, OFFSET_TIME) {
                        to_revalidate.push(pooled_tx.transaction.clone());
                    }
                }
            }

            metrics.set_interop_txs_in_pool(interop_count);

            if !to_revalidate.is_empty() {
                metrics.inc_stale_tx_interop(to_revalidate.len());

                let revalidation_start = Instant::now();
                let revalidation_stream = supervisor_client.revalidate_interop_txs_stream(
                    to_revalidate,
                    timestamp,
                    TRANSACTION_VALIDITY_WINDOW,
                    MAX_SUPERVISOR_QUERIES,
                );

                futures_util::pin_mut!(revalidation_stream);

                while let Some((tx_item_from_stream, validation_result)) =
                    revalidation_stream.next().await
                {
                    let tx_hash = *tx_item_from_stream.hash();
                    let attempts = revalidation_attempts.entry(tx_hash).or_insert(0);
                    *attempts += 1;
                    metrics.inc_revalidation_attempts(1);

                    match validation_result {
                        Some(Ok(())) => {
                            tx_item_from_stream
                                .set_interop_deadline(timestamp + TRANSACTION_VALIDITY_WINDOW);
                        }
                        Some(Err(err)) => {
                            if err.is_bad_transaction() {
                                if let Some(attempts) = revalidation_attempts.remove(&tx_hash) {
                                    metrics.record_attempts_on_removal(attempts);
                                }
                                to_remove.push(tx_hash);
                            }
                        }
                        None => {
                            warn!(
                                target: "txpool",
                                hash = %tx_hash,
                                "Interop transaction no longer considered cross-chain during revalidation; removing."
                            );
                            if let Some(attempts) = revalidation_attempts.remove(&tx_hash) {
                                metrics.record_attempts_on_removal(attempts);
                            }
                            to_remove.push(tx_hash);
                        }
                    }
                }

                metrics.record_supervisor_duration(revalidation_start.elapsed());
                metrics.set_tracked_entries(revalidation_attempts.len());
            }

            if !to_remove.is_empty() {
                // Cleanup any attempt tracking for transactions that are being removed for reasons
                // outside of supervisor revalidation outcome (e.g. deadline exceeded before stream)
                for hash in &to_remove {
                    revalidation_attempts.remove(hash);
                }
                let removed = pool.remove_transactions(to_remove);
                metrics.inc_removed_tx_interop(removed.len());
                metrics.set_tracked_entries(revalidation_attempts.len());
            }
        }
    }
}
