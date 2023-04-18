//! Support for manages the state of the transaction pool

use crate::TransactionPool;
use futures_util::{Stream, StreamExt};
use reth_provider::CanonStateNotification;

/// Manages the state of the transaction pool by handling new blocks and reorgs.
pub async fn manage_transaction_pool<Pool, St>(pool: Pool, mut events: St)
where
    Pool: TransactionPool + 'static,
    St: Stream<Item = CanonStateNotification> + Unpin + 'static,
{
    while let Some(event) = events.next().await {
        // match event {
        //     CanonStateNotification::Reorg { old, new } => {
        //     }
        //     CanonStateNotification::Revert { old } => {
        //
        //     }
        //     CanonStateNotification::Commit { new } => {
        //         new.bl
        //     }
        // }
    }
}
