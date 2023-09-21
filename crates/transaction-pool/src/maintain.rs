//! Support for maintaining the state of the transaction pool

use crate::{
    blobstore::{BlobStoreCanonTracker, BlobStoreUpdates},
    metrics::MaintainPoolMetrics,
    traits::{CanonicalStateUpdate, ChangedAccount, TransactionPoolExt},
    BlockInfo, TransactionPool,
};
use futures_util::{
    future::{BoxFuture, Fuse, FusedFuture},
    FutureExt, Stream, StreamExt,
};
use reth_interfaces::RethError;
use reth_primitives::{
    Address, BlockHash, BlockNumber, BlockNumberOrTag, FromRecoveredTransaction,
};
use reth_provider::{
    BlockReaderIdExt, BundleStateWithReceipts, CanonStateNotification, ChainSpecProvider,
    StateProviderFactory,
};
use reth_tasks::TaskSpawner;
use std::{
    borrow::Borrow,
    collections::HashSet,
    hash::{Hash, Hasher},
};
use tokio::sync::oneshot;
use tracing::{debug, trace};

/// Additional settings for maintaining the transaction pool
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MaintainPoolConfig {
    /// Maximum (reorg) depth we handle when updating the transaction pool: `new.number -
    /// last_seen.number`
    ///
    /// Default: 64 (2 epochs)
    pub max_update_depth: u64,
    /// Maximum number of accounts to reload from state at once when updating the transaction pool.
    ///
    /// Default: 250
    pub max_reload_accounts: usize,
}

impl Default for MaintainPoolConfig {
    fn default() -> Self {
        Self { max_update_depth: 64, max_reload_accounts: 250 }
    }
}

/// Returns a spawnable future for maintaining the state of the transaction pool.
pub fn maintain_transaction_pool_future<Client, P, St, Tasks>(
    client: Client,
    pool: P,
    events: St,
    task_spawner: Tasks,
    config: MaintainPoolConfig,
) -> BoxFuture<'static, ()>
where
    Client: StateProviderFactory + BlockReaderIdExt + ChainSpecProvider + Clone + Send + 'static,
    P: TransactionPoolExt + 'static,
    St: Stream<Item = CanonStateNotification> + Send + Unpin + 'static,
    Tasks: TaskSpawner + 'static,
{
    async move {
        maintain_transaction_pool(client, pool, events, task_spawner, config).await;
    }
    .boxed()
}

/// Maintains the state of the transaction pool by handling new blocks and reorgs.
///
/// This listens for any new blocks and reorgs and updates the transaction pool's state accordingly
pub async fn maintain_transaction_pool<Client, P, St, Tasks>(
    client: Client,
    pool: P,
    mut events: St,
    task_spawner: Tasks,
    config: MaintainPoolConfig,
) where
    Client: StateProviderFactory + BlockReaderIdExt + ChainSpecProvider + Clone + Send + 'static,
    P: TransactionPoolExt + 'static,
    St: Stream<Item = CanonStateNotification> + Send + Unpin + 'static,
    Tasks: TaskSpawner + 'static,
{
    let metrics = MaintainPoolMetrics::default();
    let MaintainPoolConfig { max_update_depth, max_reload_accounts } = config;
    // ensure the pool points to latest state
    if let Ok(Some(latest)) = client.block_by_number_or_tag(BlockNumberOrTag::Latest) {
        let latest = latest.seal_slow();
        let chain_spec = client.chain_spec();
        let info = BlockInfo {
            last_seen_block_hash: latest.hash,
            last_seen_block_number: latest.number,
            pending_basefee: latest
                .next_block_base_fee(chain_spec.base_fee_params)
                .unwrap_or_default(),
            pending_blob_fee: latest.next_block_blob_fee(),
        };
        pool.set_block_info(info);
    }

    // keeps track of mined blob transaction so we can clean finalized transactions
    let mut blob_store_tracker = BlobStoreCanonTracker::default();

    // keeps track of the latest finalized block
    let mut last_finalized_block =
        FinalizedBlockTracker::new(client.finalized_block_number().ok().flatten());

    // keeps track of any dirty accounts that we know of are out of sync with the pool
    let mut dirty_addresses = HashSet::new();

    // keeps track of the state of the pool wrt to blocks
    let mut maintained_state = MaintainedPoolState::InSync;

    // the future that reloads accounts from state
    let mut reload_accounts_fut = Fuse::terminated();

    // The update loop that waits for new blocks and reorgs and performs pool updated
    // Listen for new chain events and derive the update action for the pool
    loop {
        trace!(target: "txpool", state=?maintained_state, "awaiting new block or reorg");

        metrics.set_dirty_accounts_len(dirty_addresses.len());
        let pool_info = pool.block_info();

        // after performing a pool update after a new block we have some time to properly update
        // dirty accounts and correct if the pool drifted from current state, for example after
        // restart or a pipeline run
        if maintained_state.is_drifted() {
            // assuming all senders are dirty
            dirty_addresses = pool.unique_senders();
            maintained_state = MaintainedPoolState::InSync;
        }

        // if we have accounts that are out of sync with the pool, we reload them in chunks
        if !dirty_addresses.is_empty() && reload_accounts_fut.is_terminated() {
            let (tx, rx) = oneshot::channel();
            let c = client.clone();
            let at = pool_info.last_seen_block_hash;
            let fut = if dirty_addresses.len() > max_reload_accounts {
                // need to chunk accounts to reload
                let accs_to_reload =
                    dirty_addresses.iter().copied().take(max_reload_accounts).collect::<Vec<_>>();
                for acc in &accs_to_reload {
                    // make sure we remove them from the dirty set
                    dirty_addresses.remove(acc);
                }
                async move {
                    let res = load_accounts(c, at, accs_to_reload.into_iter());
                    let _ = tx.send(res);
                }
                .boxed()
            } else {
                // can fetch all dirty accounts at once
                let accs_to_reload = std::mem::take(&mut dirty_addresses);
                async move {
                    let res = load_accounts(c, at, accs_to_reload.into_iter());
                    let _ = tx.send(res);
                }
                .boxed()
            };
            reload_accounts_fut = rx.fuse();
            task_spawner.spawn_blocking(fut);
        }

        // check if we have a new finalized block
        if let Some(finalized) =
            last_finalized_block.update(client.finalized_block_number().ok().flatten())
        {
            match blob_store_tracker.on_finalized_block(finalized) {
                BlobStoreUpdates::None => {}
                BlobStoreUpdates::Finalized(blobs) => {
                    // remove all finalized blobs from the blob store
                    pool.delete_blobs(blobs);
                }
            }
        }

        // outcomes of the futures we are waiting on
        let mut event = None;
        let mut reloaded = None;

        // select of account reloads and new canonical state updates which should arrive at the rate
        // of the block time (12s)
        tokio::select! {
            res = &mut reload_accounts_fut =>  {
                reloaded = Some(res);
            }
            ev = events.next() =>  {
                 if ev.is_none() {
                    // the stream ended, we are done
                    break;
                }
                event = ev;
            }
        }

        // handle the result of the account reload
        match reloaded {
            Some(Ok(Ok(LoadedAccounts { accounts, failed_to_load }))) => {
                // reloaded accounts successfully
                // extend accounts we failed to load from database
                dirty_addresses.extend(failed_to_load);
                // update the pool with the loaded accounts
                pool.update_accounts(accounts);
            }
            Some(Ok(Err(res))) => {
                // Failed to load accounts from state
                let (accs, err) = *res;
                debug!(target: "txpool", ?err, "failed to load accounts");
                dirty_addresses.extend(accs);
            }
            Some(Err(_)) => {
                // failed to receive the accounts, sender dropped, only possible if task panicked
                maintained_state = MaintainedPoolState::Drifted;
            }
            None => {}
        }

        // handle the new block or reorg
        let Some(event) = event else { continue };
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
                    maintained_state = MaintainedPoolState::Drifted;
                }

                let chain_spec = client.chain_spec();

                // fees for the next block: `new_tip+1`
                let pending_block_base_fee =
                    new_tip.next_block_base_fee(chain_spec.base_fee_params).unwrap_or_default();
                let pending_block_blob_fee = new_tip.next_block_blob_fee();

                // we know all changed account in the new chain
                let new_changed_accounts: HashSet<_> =
                    changed_accounts_iter(new_state).map(ChangedAccountEntry).collect();

                // find all accounts that were changed in the old chain but _not_ in the new chain
                let missing_changed_acc = old_state
                    .accounts_iter()
                    .map(|(a, _)| a)
                    .filter(|addr| !new_changed_accounts.contains(addr));

                // for these we need to fetch the nonce+balance from the db at the new tip
                let mut changed_accounts =
                    match load_accounts(client.clone(), new_tip.hash, missing_changed_acc) {
                        Ok(LoadedAccounts { accounts, failed_to_load }) => {
                            // extend accounts we failed to load from database
                            dirty_addresses.extend(failed_to_load);

                            accounts
                        }
                        Err(err) => {
                            let (addresses, err) = *err;
                            debug!(
                                target: "txpool",
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
                    .map(<P as TransactionPool>::Transaction::from_recovered_transaction)
                    .collect::<Vec<_>>();

                // update the pool first
                let update = CanonicalStateUpdate {
                    new_tip: &new_tip.block,
                    pending_block_base_fee,
                    pending_block_blob_fee,
                    changed_accounts,
                    // all transactions mined in the new chain need to be removed from the pool
                    mined_transactions: new_mined_transactions.into_iter().collect(),
                };
                pool.on_canonical_state_change(update);

                // all transactions that were mined in the old chain but not in the new chain need
                // to be re-injected
                //
                // Note: we no longer know if the tx was local or external
                metrics.inc_reinserted_transactions(pruned_old_transactions.len());
                let _ = pool.add_external_transactions(pruned_old_transactions).await;

                // keep track of mined blob transactions
                // TODO(mattsse): handle reorged transactions
                blob_store_tracker.add_new_chain_blocks(&new_blocks);
            }
            CanonStateNotification::Commit { new } => {
                let (blocks, state) = new.inner();
                let tip = blocks.tip();
                let chain_spec = client.chain_spec();

                // fees for the next block: `tip+1`
                let pending_block_base_fee =
                    tip.next_block_base_fee(chain_spec.base_fee_params).unwrap_or_default();
                let pending_block_blob_fee = tip.next_block_blob_fee();

                let first_block = blocks.first();
                trace!(
                    target: "txpool",
                    first = first_block.number,
                    tip = tip.number,
                    pool_block = pool_info.last_seen_block_number,
                    "update pool on new commit"
                );

                // check if the depth is too large and should be skipped, this could happen after
                // initial sync or long re-sync
                let depth = tip.number.abs_diff(pool_info.last_seen_block_number);
                if depth > max_update_depth {
                    maintained_state = MaintainedPoolState::Drifted;
                    debug!(target: "txpool", ?depth, "skipping deep canonical update");
                    let info = BlockInfo {
                        last_seen_block_hash: tip.hash,
                        last_seen_block_number: tip.number,
                        pending_basefee: pending_block_base_fee,
                        pending_blob_fee: pending_block_blob_fee,
                    };
                    pool.set_block_info(info);

                    // keep track of mined blob transactions
                    blob_store_tracker.add_new_chain_blocks(&blocks);

                    continue
                }

                let mut changed_accounts = Vec::with_capacity(state.state().len());
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
                    maintained_state = MaintainedPoolState::Drifted;
                }

                // Canonical update
                let update = CanonicalStateUpdate {
                    new_tip: &tip.block,
                    pending_block_base_fee,
                    pending_block_blob_fee,
                    changed_accounts,
                    mined_transactions,
                };
                pool.on_canonical_state_change(update);

                // keep track of mined blob transactions
                blob_store_tracker.add_new_chain_blocks(&blocks);
            }
        }
    }
}

struct FinalizedBlockTracker {
    last_finalized_block: Option<BlockNumber>,
}

impl FinalizedBlockTracker {
    fn new(last_finalized_block: Option<BlockNumber>) -> Self {
        Self { last_finalized_block }
    }

    /// Updates the tracked finalized block and returns the new finalized block if it changed
    fn update(&mut self, finalized_block: Option<BlockNumber>) -> Option<BlockNumber> {
        match (self.last_finalized_block, finalized_block) {
            (Some(last), Some(finalized)) => {
                self.last_finalized_block = Some(finalized);
                if last < finalized {
                    Some(finalized)
                } else {
                    None
                }
            }
            (None, Some(finalized)) => {
                self.last_finalized_block = Some(finalized);
                Some(finalized)
            }
            _ => None,
        }
    }
}

/// Keeps track of the pool's state, whether the accounts in the pool are in sync with the actual
/// state.
#[derive(Debug, Eq, PartialEq)]
enum MaintainedPoolState {
    /// Pool is assumed to be in sync with the current state
    InSync,
    /// Pool could be out of sync with the state
    Drifted,
}

impl MaintainedPoolState {
    /// Returns `true` if the pool is assumed to be out of sync with the current state.
    fn is_drifted(&self) -> bool {
        matches!(self, MaintainedPoolState::Drifted)
    }
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
    client: Client,
    at: BlockHash,
    addresses: I,
) -> Result<LoadedAccounts, Box<(HashSet<Address>, RethError)>>
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

/// Extracts all changed accounts from the BundleState
fn changed_accounts_iter(
    state: &BundleStateWithReceipts,
) -> impl Iterator<Item = ChangedAccount> + '_ {
    state
        .accounts_iter()
        .filter_map(|(addr, acc)| acc.map(|acc| (addr, acc)))
        .map(|(address, acc)| ChangedAccount { address, nonce: acc.nonce, balance: acc.balance })
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
