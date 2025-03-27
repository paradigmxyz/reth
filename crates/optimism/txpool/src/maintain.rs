//! Support for maintaining the state of the transaction pool

/// The interval for which we check transaction against supervisor, 10 min.
const TRANSACTION_VALIDITY_WINDOW: u64 = 600;
/// Interval in seconds at which the transaction should be revalidated.
const OFFSET_TIME: u64 = 60;

use crate::{
    conditional::MaybeConditionalTransaction,
    interop::{MaybeInteropTransaction, TransactionInterop},
    validator::is_valid_cross_tx,
};
use alloy_consensus::{conditional::BlockConditionalAttributes, BlockHeader, Transaction};
use futures_util::{future::BoxFuture, FutureExt, Stream, StreamExt};
use reth_chain_state::CanonStateNotification;
use reth_metrics::{metrics::Counter, Metrics};
use reth_optimism_primitives::SupervisorClient;
use reth_primitives_traits::NodePrimitives;
use reth_transaction_pool::{error::PoolTransactionError, PoolTransaction, TransactionPool};

/// Transaction pool maintenance metrics
#[derive(Metrics)]
#[metrics(scope = "transaction_pool")]
struct MaintainPoolMetrics {
    /// Counter indicating the number of conditional transactions removed from
    /// the pool because of exceeded block attributes.
    removed_tx_conditional: Counter,
}

impl MaintainPoolMetrics {
    #[inline]
    fn inc_removed_tx_conditional(&self, count: usize) {
        self.removed_tx_conditional.increment(count as u64);
    }
}

/// Returns a spawnable future for maintaining the state of the transaction pool.
pub fn maintain_transaction_pool_future<N, Pool, St>(
    pool: Pool,
    events: St,
    supervisor_client: SupervisorClient,
) -> BoxFuture<'static, ()>
where
    N: NodePrimitives,
    Pool: TransactionPool + 'static,
    Pool::Transaction: MaybeConditionalTransaction + MaybeInteropTransaction,
    St: Stream<Item = CanonStateNotification<N>> + Send + Unpin + 'static,
{
    async move {
        maintain_transaction_pool(pool, events, supervisor_client).await;
    }
    .boxed()
}

/// Maintains the state of the transaction pool by handling new blocks and reorgs.
///
/// This listens for any new blocks and reorgs and updates the transaction pool's state accordingly
pub async fn maintain_transaction_pool<N, Pool, St>(
    pool: Pool,
    mut events: St,
    supervisor_client: SupervisorClient,
) where
    N: NodePrimitives,
    Pool: TransactionPool,
    Pool::Transaction: MaybeConditionalTransaction + MaybeInteropTransaction,
    St: Stream<Item = CanonStateNotification<N>> + Send + Unpin + 'static,
{
    let metrics = MaintainPoolMetrics::default();
    loop {
        let Some(event) = events.next().await else { break };
        if let CanonStateNotification::Commit { new } = event {
            if new.is_empty() {
                continue;
            }
            let block_attr = BlockConditionalAttributes {
                number: new.tip().number(),
                timestamp: new.tip().timestamp(),
            };
            let mut to_remove = Vec::new();
            let mut to_revalidate = Vec::new();
            for tx in &pool.pooled_transactions() {
                if tx.transaction.has_exceeded_block_attributes(&block_attr) {
                    to_remove.push(*tx.hash());
                }
                // Only interop txs have this field set
                if let Some(interop) = tx.transaction.interop() {
                    if !interop.is_valid(block_attr.timestamp) {
                        // That means tx didn't revalidated during [`OFFSET_TIME`] time
                        // We could assume that it won't be validated at all and remove it
                        to_remove.push(*tx.hash());
                    } else if interop.is_stale(block_attr.timestamp, OFFSET_TIME) {
                        // If tx has less then [`OFFSET_TIME`] of valid time we revalidate it
                        to_revalidate.push(tx.clone())
                    }
                }
            }
            if !to_revalidate.is_empty() {
                // TODO: We should change it to a single batch call once supervisor implements it
                // TODO: add metrics for revalidation
                for tx in to_revalidate {
                    if let Some(Err(err)) = is_valid_cross_tx(
                        tx.transaction.access_list(),
                        tx.transaction.hash(),
                        block_attr.timestamp,
                        Some(TRANSACTION_VALIDITY_WINDOW),
                        // We could assume that interop is enabled, because
                        // tx.transaction.interop() would be set only in
                        // this case
                        true,
                        Some(&supervisor_client),
                    )
                    .await
                    {
                        // We remove only bad transaction. If error caused by supervisor instability
                        // or other fixable issues transaction would be validated on next state
                        // change, so we ignore it
                        if err.is_bad_transaction() {
                            to_remove.push(*tx.transaction.hash());
                        }
                    } else {
                        tx.transaction.set_interop(TransactionInterop {
                            timeout: block_attr.timestamp + TRANSACTION_VALIDITY_WINDOW,
                        })
                    }
                }
            }
            if !to_remove.is_empty() {
                metrics.inc_removed_tx_conditional(to_remove.len());
                let _ = pool.remove_transactions(to_remove);
            }
        }
    }
}
