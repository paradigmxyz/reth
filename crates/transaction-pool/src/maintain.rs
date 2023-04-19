//! Support for maintaining the state of the transaction pool

use crate::{Pool, TransactionOrdering, TransactionPool, TransactionValidator};
use futures_util::{Stream, StreamExt};
use reth_provider::{BlockProvider, CanonStateNotification, StateProviderFactory};

/// Maintains the state of the transaction pool by handling new blocks and reorgs.
///
/// This listens for any new blocks and reorgs and updates the transaction pool's state accordingly
pub async fn maintain_transaction_pool<Client, V, T, St>(
    client: Client,
    pool: Pool<V, T>,
    mut events: St,
) where
    Client: StateProviderFactory + BlockProvider + 'static,
    V: TransactionValidator,
    T: TransactionOrdering<Transaction = <V as TransactionValidator>::Transaction>,
    St: Stream<Item = CanonStateNotification> + Unpin + 'static,
{
    // TODO set current head for the pool

    // Listen for new chain events and derive the update action for the pool
    while let Some(event) = events.next().await {
        let pool_info = pool.block_info();

        match event {
            CanonStateNotification::Reorg { old, new } => {
            }
            CanonStateNotification::Revert { old } => {

            }
            CanonStateNotification::Commit { new } => {
            }
        }
    }
}
