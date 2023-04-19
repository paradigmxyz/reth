//! Support for maintaining the state of the transaction pool

use crate::{
    traits::{CanonicalStateUpdate, ChangedAccount},
    Pool, TransactionOrdering, TransactionPool, TransactionValidator,
};
use futures_util::{Stream, StreamExt};
use reth_consensus_common::validation::calculate_next_block_base_fee;
use reth_provider::{BlockProvider, CanonStateNotification, PostState, StateProviderFactory};

/// Maintains the state of the transaction pool by handling new blocks and reorgs.
///
/// This listens for any new blocks and reorgs and updates the transaction pool's state accordingly
pub async fn maintain_transaction_pool<Client, V, T, St>(
    _client: Client,
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
            CanonStateNotification::Reorg { old: _, new: _ } => {
                // TODO use new chain to update the pool's state and reinject missing transactions
                // from the old chain TODO if pool inside the old chain chain, we
                // need to reset this and find missing account changes after comparing new state
                // accounts with old state accounts
            }
            CanonStateNotification::Revert { old } => {
                // this similar to the inverse of a commit where we need to insert the transactions
                // back into the pool and update the pool's state accordingly

                let (blocks, _state) = old.inner();
                let first_block = blocks.first();
                // TODO is this inclusive or exclusive?
                let _pending_block_base_fee =
                    first_block.base_fee_per_gas.unwrap_or_default() as u128;

                // check if the range of the commit is canonical
                if first_block.parent_hash == pool_info.last_seen_block_hash {
                    // We reverted to the state that the pool already tracks, so we skip this
                    // TODO insert all transactions back into the pool?
                }

                // TODO this could be a revert to prior block or a revert to a later block than that
                // what's being tracked
            }
            CanonStateNotification::Commit { new } => {
                let (blocks, state) = new.inner();
                let tip = blocks.tip();
                // base fee for the next block: `tip+1`
                let pending_block_base_fee = calculate_next_block_base_fee(
                    tip.gas_used,
                    tip.gas_limit,
                    tip.base_fee_per_gas.unwrap_or_default(),
                ) as u128;

                let first_block = blocks.first();
                // check if the range of the commit is canonical
                if first_block.parent_hash == pool_info.last_seen_block_hash {
                    let changed_accounts = changed_accounts(state);
                    let mined_transactions = blocks.transactions().map(|tx| tx.hash).collect();
                    // Canonical update
                    let update = CanonicalStateUpdate {
                        hash: tip.hash,
                        number: tip.number,
                        pending_block_base_fee,
                        changed_accounts,
                        mined_transactions,
                    };
                    pool.on_canonical_state_change(update);
                } else {
                    // range update, need to get account state for transactions
                }
            }
        }
    }
}

fn changed_accounts(state: &PostState) -> Vec<ChangedAccount> {
    state
        .accounts()
        .iter()
        .filter_map(|(addr, acc)| acc.map(|acc| (addr, acc)))
        .map(|(address, acc)| ChangedAccount {
            address: *address,
            nonce: acc.nonce,
            balance: acc.balance,
        })
        .collect()
}
