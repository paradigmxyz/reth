//! Support for maintaining the state of the transaction pool

use crate::{
    traits::{CanonicalStateUpdate, ChangedAccount},
    BlockInfo, Pool, TransactionOrdering, TransactionPool, TransactionValidator,
};
use futures_util::{Stream, StreamExt};
use reth_primitives::{Address, BlockHash, BlockNumberOrTag, FromRecoveredTransaction};
use reth_provider::{BlockProvider, CanonStateNotification, PostState, StateProviderFactory};
use std::{
    borrow::Borrow,
    collections::HashSet,
    hash::{Hash, Hasher},
};
use tracing::{debug, warn};

/// Maximum (reorg) depth we handle when updating the transaction pool: `new.number -
/// last_seen.number`
const MAX_UPDATE_DEPTH: u64 = 64;

/// Maintains the state of the transaction pool by handling new blocks and reorgs.
///
/// This listens for any new blocks and reorgs and updates the transaction pool's state accordingly
#[allow(unused)]
pub async fn maintain_transaction_pool<Client, V, T, St>(
    client: Client,
    pool: Pool<V, T>,
    mut events: St,
) where
    Client: StateProviderFactory + BlockProvider,
    V: TransactionValidator,
    T: TransactionOrdering<Transaction = <V as TransactionValidator>::Transaction>,
    St: Stream<Item = CanonStateNotification> + Unpin,
{
    // ensure the pool points to latest state
    if let Ok(Some(latest)) = client.block(BlockNumberOrTag::Latest.into()) {
        let latest = latest.seal_slow();
        let info = BlockInfo {
            last_seen_block_hash: latest.hash,
            last_seen_block_number: latest.number,
            pending_basefee: latest.next_block_base_fee().unwrap_or_default() as u128,
        };
        pool.set_block_info(info);
    }

    // keeps track of any dirty accounts that we know of are out of sync with the pool
    let mut dirty_addresses = HashSet::new();

    // keeps track of the state of the pool wrt to blocks
    let mut maintained_state = MaintainedPoolState::InSync;

    // Listen for new chain events and derive the update action for the pool
    while let Some(event) = events.next().await {
        let pool_info = pool.block_info();

        // TODO from time to time re-check the unique accounts in the pool and remove and resync
        // based on the tracked state

        match event {
            CanonStateNotification::Reorg { old, new } => {
                let (old_blocks, old_state) = old.inner();
                let (new_blocks, new_state) = new.inner();
                let new_tip = new_blocks.tip();
                let new_first = new_blocks.first();
                let old_first = old_blocks.first();

                // check if the reorg is not canonical with the pool's block
                if !(old_first.parent_hash == pool_info.last_seen_block_hash ||
                    new_first.parent_hash == pool_info.last_seen_block_hash)
                {
                    // the new block points to a higher block than the oldest block in the old chain
                    maintained_state = MaintainedPoolState::Drift;
                }

                // base fee for the next block: `new_tip+1`
                let pending_block_base_fee =
                    new_tip.next_block_base_fee().unwrap_or_default() as u128;

                // we know all changed account in the new chain
                let new_changed_accounts: HashSet<_> =
                    changed_accounts_iter(new_state).map(ChangedAccountEntry).collect();

                // find all accounts that were changed in the old chain but _not_ in the new chain
                let missing_changed_acc = old_state
                    .accounts()
                    .keys()
                    .copied()
                    .filter(|addr| !new_changed_accounts.contains(addr));

                // for these we need to fetch the nonce+balance from the db at the new tip
                let mut changed_accounts =
                    match load_accounts(&client, new_tip.hash, missing_changed_acc) {
                        Ok(LoadedAccounts { accounts, failed_to_load }) => {
                            // extend accounts we failed to load from database
                            dirty_addresses.extend(failed_to_load);

                            accounts
                        }
                        Err(err) => {
                            let (addresses, err) = *err;
                            warn!(
                                ?err,
                                "failed to load missing changed accounts at new tip: {:?}",
                                new_tip.hash
                            );
                            dirty_addresses.extend(addresses);
                            vec![]
                        }
                    };

                // also include all accounts from new chain
                // we can use extend here because they are unique
                changed_accounts.extend(new_changed_accounts.into_iter().map(|entry| entry.0));

                // all transactions mined in the new chain
                let new_mined_transactions: HashSet<_> =
                    new_blocks.transactions().map(|tx| tx.hash).collect();

                // update the pool then re-inject the pruned transactions
                // find all transactions that were mined in the old chain but not in the new chain
                let pruned_old_transactions = old_blocks
                    .transactions()
                    .filter(|tx| !new_mined_transactions.contains(&tx.hash))
                    .filter_map(|tx| tx.clone().into_ecrecovered())
                    .map(|tx| {
                        <V as TransactionValidator>::Transaction::from_recovered_transaction(tx)
                    })
                    .collect();

                // update the pool first
                let update = CanonicalStateUpdate {
                    hash: new_tip.hash,
                    number: new_tip.number,
                    pending_block_base_fee,
                    changed_accounts,
                    // all transactions mined in the new chain need to be removed from the pool
                    mined_transactions: new_mined_transactions.into_iter().collect(),
                };
                pool.on_canonical_state_change(update);

                // all transactions that were mined in the old chain but not in the new chain need
                // to be re-injected
                //
                // Note: we no longer know if the tx was local or external
                let _ = pool.add_external_transactions(pruned_old_transactions).await;
                // TODO: metrics
            }
            CanonStateNotification::Revert { old } => {
                // this similar to the inverse of a commit where we need to insert the transactions
                // back into the pool and update the pool's state accordingly

                let (blocks, state) = old.inner();
                let first_block = blocks.first();

                if first_block.hash == pool_info.last_seen_block_hash {
                    // nothing to update
                    continue
                }

                // base fee for the next block: `first_block+1`
                let pending_block_base_fee =
                    first_block.next_block_base_fee().unwrap_or_default() as u128;

                let mut changed_accounts = Vec::with_capacity(state.accounts().len());
                for acc in changed_accounts_iter(state) {
                    // we can always clear the dirty flag for this account
                    dirty_addresses.remove(&acc.address);
                    changed_accounts.push(acc);
                }

                let update = CanonicalStateUpdate {
                    hash: first_block.hash,
                    number: first_block.number,
                    pending_block_base_fee,
                    changed_accounts,
                    // no tx to prune in the reverted chain
                    mined_transactions: vec![],
                };
                pool.on_canonical_state_change(update);

                let pruned_old_transactions = blocks
                    .transactions()
                    .filter_map(|tx| tx.clone().into_ecrecovered())
                    .map(|tx| {
                        <V as TransactionValidator>::Transaction::from_recovered_transaction(tx)
                    })
                    .collect();

                // all transactions that were mined in the old chain need to be re-injected
                //
                // Note: we no longer know if the tx was local or external
                let _ = pool.add_external_transactions(pruned_old_transactions).await;
                // TODO: metrics
            }
            CanonStateNotification::Commit { new } => {
                let (blocks, state) = new.inner();
                let tip = blocks.tip();

                // base fee for the next block: `tip+1`
                let pending_block_base_fee = tip.next_block_base_fee().unwrap_or_default() as u128;

                let first_block = blocks.first();

                // check if the depth is too large and should be skipped, this could happen after
                // initial sync or long re-sync
                let depth = tip.number.abs_diff(pool_info.last_seen_block_number);
                if depth > MAX_UPDATE_DEPTH {
                    maintained_state = MaintainedPoolState::Drift;
                    debug!(?depth, "skipping deep canonical update");
                    let info = BlockInfo {
                        last_seen_block_hash: tip.hash,
                        last_seen_block_number: tip.number,
                        pending_basefee: pending_block_base_fee,
                    };
                    pool.set_block_info(info);
                    continue
                }

                let mut changed_accounts = Vec::with_capacity(state.accounts().len());
                for acc in changed_accounts_iter(state) {
                    // we can always clear the dirty flag for this account
                    dirty_addresses.remove(&acc.address);
                    changed_accounts.push(acc);
                }

                let mined_transactions = blocks.transactions().map(|tx| tx.hash).collect();

                // check if the range of the commit is canonical with the pool's block
                if first_block.parent_hash != pool_info.last_seen_block_hash {
                    // we received a new canonical chain commit but the commit is not canonical with
                    // the pool's block, this could happen after initial sync or
                    // long re-sync
                    maintained_state = MaintainedPoolState::Drift;
                }

                // Canonical update
                let update = CanonicalStateUpdate {
                    hash: tip.hash,
                    number: tip.number,
                    pending_block_base_fee,
                    changed_accounts,
                    mined_transactions,
                };
                pool.on_canonical_state_change(update);
            }
        }
    }
}

/// Keeps track of the pool's state, whether the accounts in the pool are in sync with the actual
/// state.
#[derive(Eq, PartialEq)]
enum MaintainedPoolState {
    /// Pool is assumed to be in sync with the state
    InSync,
    /// Pool could be out of sync with the state
    Drift,
}

/// A unique ChangedAccount identified by its address that can be used for deduplication
#[derive(Eq)]
struct ChangedAccountEntry(ChangedAccount);

impl PartialEq for ChangedAccountEntry {
    fn eq(&self, other: &Self) -> bool {
        self.0.address == other.0.address
    }
}

impl Hash for ChangedAccountEntry {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.address.hash(state);
    }
}

impl Borrow<Address> for ChangedAccountEntry {
    fn borrow(&self) -> &Address {
        &self.0.address
    }
}

#[derive(Default)]
struct LoadedAccounts {
    /// All accounts that were loaded
    accounts: Vec<ChangedAccount>,
    /// All accounts that failed to load
    failed_to_load: Vec<Address>,
}

/// Loads all accounts at the given state
///
/// Returns an error with all given addresses if the state is not available.
///
/// Note: this expects _unique_ addresses
fn load_accounts<Client, I>(
    client: &Client,
    at: BlockHash,
    addresses: I,
) -> Result<LoadedAccounts, Box<(HashSet<Address>, reth_interfaces::Error)>>
where
    I: Iterator<Item = Address>,

    Client: StateProviderFactory,
{
    let mut res = LoadedAccounts::default();
    let state = match client.history_by_block_hash(at) {
        Ok(state) => state,
        Err(err) => return Err(Box::new((addresses.collect(), err))),
    };
    for addr in addresses {
        if let Ok(maybe_acc) = state.basic_account(addr) {
            let acc = maybe_acc
                .map(|acc| ChangedAccount { address: addr, nonce: acc.nonce, balance: acc.balance })
                .unwrap_or_else(|| ChangedAccount::empty(addr));
            res.accounts.push(acc)
        } else {
            // failed to load account.
            res.failed_to_load.push(addr);
        }
    }
    Ok(res)
}

/// Extracts all changed accounts from the PostState
fn changed_accounts_iter(state: &PostState) -> impl Iterator<Item = ChangedAccount> + '_ {
    state.accounts().iter().filter_map(|(addr, acc)| acc.map(|acc| (addr, acc))).map(
        |(address, acc)| ChangedAccount {
            address: *address,
            nonce: acc.nonce,
            balance: acc.balance,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changed_acc_entry() {
        let changed_acc = ChangedAccountEntry(ChangedAccount::empty(Address::random()));
        let mut copy = changed_acc.0;
        copy.nonce = 10;
        assert!(changed_acc.eq(&ChangedAccountEntry(copy)));
    }
}
