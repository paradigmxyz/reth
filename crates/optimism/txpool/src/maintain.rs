//! Support for maintaining the state of the transaction pool

use alloy_consensus::{conditional::BlockConditionalAttributes, BlockHeader};
use futures_util::{future::BoxFuture, FutureExt, Stream, StreamExt};
use reth_primitives_traits::NodePrimitives;
use reth_provider::CanonStateNotification;
use reth_transaction_pool::TransactionPool;

use crate::conditional::MaybeConditionalTransaction;

/// Returns a spawnable future for maintaining the state of the transaction pool.
pub fn maintain_transaction_pool_future<N, Pool, St>(
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
        maintain_transaction_pool(pool, events).await;
    }
    .boxed()
}

/// Maintains the state of the transaction pool by handling new blocks and reorgs.
///
/// This listens for any new blocks and reorgs and updates the transaction pool's state accordingly
pub async fn maintain_transaction_pool<N, Pool, St>(pool: Pool, mut events: St)
where
    N: NodePrimitives,
    Pool: TransactionPool,
    Pool::Transaction: MaybeConditionalTransaction,
    St: Stream<Item = CanonStateNotification<N>> + Send + Unpin + 'static,
{
    loop {
        let Some(event) = events.next().await else { break };
        if let CanonStateNotification::Commit { new } = event {
            if !new.is_empty() {
                let header = new.tip().clone().into_header();
                let mut to_remove = Vec::new();
                for tx in &pool.pooled_transactions() {
                    if let Some(conditional) = tx.transaction.conditional() {
                        if conditional.has_exceeded_block_attributes(&BlockConditionalAttributes {
                            number: header.number(),
                            timestamp: header.timestamp(),
                        }) {
                            to_remove.push(*tx.hash());
                        }
                    }
                }
                let _ = pool.remove_transactions(to_remove);
            }
        }
    }
}
