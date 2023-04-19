//! Support for maintaining the state of the transaction pool

use crate::{Pool, TransactionOrdering, TransactionPool, TransactionValidator};
use futures_util::{Stream, StreamExt};
use reth_consensus_common::validation::calculate_next_block_base_fee;
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
                // this similar to the inverse of a commit where we need to insert the transactions back into the pool and update the pool's state accordingly
                let (blocks, state) = old.into_inner();
                let tip = blocks.tip();
                // base fee for the next block
                let next_base_fee = calculate_next_block_base_fee(tip.gas_used, tip.gas_limit, tip.base_fee_per_gas.unwrap_or_default());
            }
            CanonStateNotification::Commit { new } => {
                let (blocks, state) = new.into_inner();
                let tip = blocks.tip();

                // base fee for the next block
                let next_base_fee = calculate_next_block_base_fee(tip.gas_used, tip.gas_limit, tip.base_fee_per_gas.unwrap_or_default());

                // check the range of the Commit
                if tip.parent_hash == pool_info.last_seen_block_hash {
                    // Canonical update
                } else {
                    // range update, need to enforce conditions manually
                }
            }
        }
    }
}
