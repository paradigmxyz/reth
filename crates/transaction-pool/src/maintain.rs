//! Support for maintaining the state of the transaction pool

use crate::{
    blobstore::{BlobStoreCanonTracker, BlobStoreUpdates},
    error::PoolError,
    metrics::MaintainPoolMetrics,
    traits::{CanonicalStateUpdate, ChangedAccount, TransactionPool, TransactionPoolExt},
    BlockInfo,
};
use futures_util::{
    future::{BoxFuture, Fuse, FusedFuture},
    FutureExt, Stream, StreamExt,
};
use reth_primitives::{
    fs::FsPathError, Address, BlockHash, BlockNumber, BlockNumberOrTag,
    FromRecoveredPooledTransaction, FromRecoveredTransaction, IntoRecoveredTransaction,
    PooledTransactionsElementEcRecovered, TransactionSigned,
};
use reth_provider::{
    BlockReaderIdExt, BundleStateWithReceipts, CanonStateNotification, ChainSpecProvider,
    ProviderError, StateProviderFactory,
};
use reth_tasks::TaskSpawner;
use std::{
    borrow::Borrow,
    collections::HashSet,
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
};
use tokio::sync::oneshot;
use tracing::{debug, error, info, trace, warn};

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
    /// Default: 100
    pub max_reload_accounts: usize,
}

impl Default for MaintainPoolConfig {
    fn default() -> Self {
        Self { max_update_depth: 64, max_reload_accounts: 100 }
    }
}

/// Settings for local transaction backup task
#[derive(Debug, Clone, Default)]
pub struct LocalTransactionBackupConfig {
    /// Path to transactions backup file
    pub transactions_path: Option<PathBuf>,
}

impl LocalTransactionBackupConfig {
    /// Receive path to transactions backup and return initialized config
    pub const fn with_local_txs_backup(transactions_path: PathBuf) -> Self {
        Self { transactions_path: Some(transactions_path) }
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
    let MaintainPoolConfig { max_update_depth, max_reload_accounts, .. } = config;
    // ensure the pool points to latest state
    if let Ok(Some(latest)) = client.header_by_number_or_tag(BlockNumberOrTag::Latest) {
        let latest = latest.seal_slow();
        let chain_spec = client.chain_spec();
        let info = BlockInfo {
            last_seen_block_hash: latest.hash(),
            last_seen_block_number: latest.number,
            pending_basefee: latest
                .next_block_base_fee(chain_spec.base_fee_params(latest.timestamp + 12))
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
            metrics.inc_drift();
            // assuming all senders are dirty
            dirty_addresses = pool.unique_senders();
            // make sure we toggle the state back to in sync
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
                    metrics.inc_deleted_tracked_blobs(blobs.len());
                    // remove all finalized blobs from the blob store
                    pool.delete_blobs(blobs);
                }
            }
            // also do periodic cleanup of the blob store
            let pool = pool.clone();
            task_spawner.spawn_blocking(Box::pin(async move {
                debug!(target: "txpool", finalized_block = %finalized, "cleaning up blob store");
                pool.cleanup_blobs();
            }));
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
                debug!(target: "txpool", %err, "failed to load accounts");
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
                let pending_block_base_fee = new_tip
                    .next_block_base_fee(chain_spec.base_fee_params(new_tip.timestamp + 12))
                    .unwrap_or_default();
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
                    match load_accounts(client.clone(), new_tip.hash(), missing_changed_acc) {
                        Ok(LoadedAccounts { accounts, failed_to_load }) => {
                            // extend accounts we failed to load from database
                            dirty_addresses.extend(failed_to_load);

                            accounts
                        }
                        Err(err) => {
                            let (addresses, err) = *err;
                            debug!(
                                target: "txpool",
                                %err,
                                "failed to load missing changed accounts at new tip: {:?}",
                                new_tip.hash()
                            );
                            dirty_addresses.extend(addresses);
                            vec![]
                        }
                    };

                // also include all accounts from new chain
                // we can use extend here because they are unique
                changed_accounts.extend(new_changed_accounts.into_iter().map(|entry| entry.0));

                // all transactions mined in the new chain
                let new_mined_transactions: HashSet<_> = new_blocks.transaction_hashes().collect();

                // update the pool then re-inject the pruned transactions
                // find all transactions that were mined in the old chain but not in the new chain
                let pruned_old_transactions = old_blocks
                    .transactions_ecrecovered()
                    .filter(|tx| !new_mined_transactions.contains(&tx.hash))
                    .filter_map(|tx| {
                        if tx.is_eip4844() {
                            // reorged blobs no longer include the blob, which is necessary for
                            // validating the transaction. Even though the transaction could have
                            // been validated previously, we still need the blob in order to
                            // accurately set the transaction's
                            // encoded-length which is propagated over the network.
                            pool.get_blob(tx.hash)
                                .ok()
                                .flatten()
                                .and_then(|sidecar| {
                                    PooledTransactionsElementEcRecovered::try_from_blob_transaction(
                                        tx, sidecar,
                                    )
                                    .ok()
                                })
                                .map(
                                    <P as TransactionPool>::Transaction::from_recovered_pooled_transaction,
                                )
                        } else {
                            Some(<P as TransactionPool>::Transaction::from_recovered_transaction(
                                tx,
                            ))
                        }
                    })
                    .collect::<Vec<_>>();

                // update the pool first
                let update = CanonicalStateUpdate {
                    new_tip: &new_tip.block,
                    pending_block_base_fee,
                    pending_block_blob_fee,
                    changed_accounts,
                    // all transactions mined in the new chain need to be removed from the pool
                    mined_transactions: new_blocks.transaction_hashes().collect(),
                };
                pool.on_canonical_state_change(update);

                // all transactions that were mined in the old chain but not in the new chain need
                // to be re-injected
                //
                // Note: we no longer know if the tx was local or external
                // Because the transactions are not finalized, the corresponding blobs are still in
                // blob store (if we previously received them from the network)
                metrics.inc_reinserted_transactions(pruned_old_transactions.len());
                let _ = pool.add_external_transactions(pruned_old_transactions).await;

                // keep track of new mined blob transactions
                blob_store_tracker.add_new_chain_blocks(&new_blocks);
            }
            CanonStateNotification::Commit { new } => {
                let (blocks, state) = new.inner();
                let tip = blocks.tip();
                let chain_spec = client.chain_spec();

                // fees for the next block: `tip+1`
                let pending_block_base_fee = tip
                    .next_block_base_fee(chain_spec.base_fee_params(tip.timestamp + 12))
                    .unwrap_or_default();
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
                        last_seen_block_hash: tip.hash(),
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

                let mined_transactions = blocks.transaction_hashes().collect();

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
    const fn new(last_finalized_block: Option<BlockNumber>) -> Self {
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
#[derive(Debug, PartialEq, Eq)]
enum MaintainedPoolState {
    /// Pool is assumed to be in sync with the current state
    InSync,
    /// Pool could be out of sync with the state
    Drifted,
}

impl MaintainedPoolState {
    /// Returns `true` if the pool is assumed to be out of sync with the current state.
    #[inline]
    const fn is_drifted(&self) -> bool {
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
) -> Result<LoadedAccounts, Box<(HashSet<Address>, ProviderError)>>
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

/// Loads transactions from a file, decodes them from the RLP format, and inserts them
/// into the transaction pool on node boot up.
/// The file is removed after the transactions have been successfully processed.
async fn load_and_reinsert_transactions<P>(
    pool: P,
    file_path: &Path,
) -> Result<(), TransactionsBackupError>
where
    P: TransactionPool,
{
    if !file_path.exists() {
        return Ok(())
    }

    debug!(target: "txpool", txs_file =?file_path, "Check local persistent storage for saved transactions");
    let data = reth_primitives::fs::read(file_path)?;

    if data.is_empty() {
        return Ok(())
    }

    let txs_signed: Vec<TransactionSigned> = alloy_rlp::Decodable::decode(&mut data.as_slice())?;

    let pool_transactions = txs_signed
        .into_iter()
        .filter_map(|tx| tx.try_ecrecovered().map(<P::Transaction>::from_recovered_transaction))
        .collect::<Vec<_>>();
    let outcome = pool.add_transactions(crate::TransactionOrigin::Local, pool_transactions).await;

    info!(target: "txpool", txs_file =?file_path, num_txs=%outcome.len(), "Successfully reinserted local transactions from file");
    reth_primitives::fs::remove_file(file_path)?;
    Ok(())
}

fn save_local_txs_backup<P>(pool: P, file_path: &Path)
where
    P: TransactionPool,
{
    let local_transactions = pool.get_local_transactions();
    if local_transactions.is_empty() {
        trace!(target: "txpool", "no local transactions to save");
        return
    }

    let local_transactions = local_transactions
        .into_iter()
        .map(|tx| tx.to_recovered_transaction().into_signed())
        .collect::<Vec<_>>();

    let num_txs = local_transactions.len();
    let mut buf = alloy_rlp::BytesMut::new();
    alloy_rlp::encode_list(&local_transactions, &mut buf);
    info!(target: "txpool", txs_file =?file_path, num_txs=%num_txs, "Saving current local transactions");
    let parent_dir = file_path.parent().map(std::fs::create_dir_all).transpose();

    match parent_dir.map(|_| reth_primitives::fs::write(file_path, buf)) {
        Ok(_) => {
            info!(target: "txpool", txs_file=?file_path, "Wrote local transactions to file");
        }
        Err(err) => {
            warn!(target: "txpool", %err, txs_file=?file_path, "Failed to write local transactions to file");
        }
    }
}

/// Errors possible during txs backup load and decode
#[derive(thiserror::Error, Debug)]
pub enum TransactionsBackupError {
    /// Error during RLP decoding of transactions
    #[error("failed to apply transactions backup. Encountered RLP decode error: {0}")]
    Decode(#[from] alloy_rlp::Error),
    /// Error during file upload
    #[error("failed to apply transactions backup. Encountered file error: {0}")]
    FsPath(#[from] FsPathError),
    /// Error adding transactions to the transaction pool
    #[error("failed to insert transactions to the transactions pool. Encountered pool error: {0}")]
    Pool(#[from] PoolError),
}

/// Task which manages saving local transactions to the persistent file in case of shutdown.
/// Reloads the transactions from the file on the boot up and inserts them into the pool.
pub async fn backup_local_transactions_task<P>(
    shutdown: reth_tasks::shutdown::GracefulShutdown,
    pool: P,
    config: LocalTransactionBackupConfig,
) where
    P: TransactionPool + Clone,
{
    let Some(transactions_path) = config.transactions_path else {
        // nothing to do
        return
    };

    if let Err(err) = load_and_reinsert_transactions(pool.clone(), &transactions_path).await {
        error!(target: "txpool", "{}", err)
    }

    let graceful_guard = shutdown.await;

    // write transactions to disk
    save_local_txs_backup(pool, &transactions_path);

    drop(graceful_guard)
}

#[cfg(not(feature = "optimism"))]
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        blobstore::InMemoryBlobStore, validate::EthTransactionValidatorBuilder,
        CoinbaseTipOrdering, EthPooledTransaction, Pool, PoolTransaction, TransactionOrigin,
    };
    use reth_primitives::{fs, hex, PooledTransactionsElement, MAINNET, U256};
    use reth_provider::test_utils::{ExtendedAccount, MockEthProvider};
    use reth_tasks::TaskManager;

    #[test]
    fn changed_acc_entry() {
        let changed_acc = ChangedAccountEntry(ChangedAccount::empty(Address::random()));
        let mut copy = changed_acc.0;
        copy.nonce = 10;
        assert!(changed_acc.eq(&ChangedAccountEntry(copy)));
    }

    const EXTENSION: &str = "rlp";
    const FILENAME: &str = "test_transactions_backup";

    #[tokio::test(flavor = "multi_thread")]
    async fn test_save_local_txs_backup() {
        let temp_dir = tempfile::tempdir().unwrap();
        let transactions_path = temp_dir.path().join(FILENAME).with_extension(EXTENSION);
        let tx_bytes = hex!("02f87201830655c2808505ef61f08482565f94388c818ca8b9251b393131c08a736a67ccb192978801049e39c4b5b1f580c001a01764ace353514e8abdfb92446de356b260e3c1225b73fc4c8876a6258d12a129a04f02294aa61ca7676061cd99f29275491218b4754b46a0248e5e42bc5091f507");
        let tx = PooledTransactionsElement::decode_enveloped(tx_bytes.into()).unwrap();
        let provider = MockEthProvider::default();
        let transaction = EthPooledTransaction::from_recovered_pooled_transaction(
            tx.try_into_ecrecovered().unwrap(),
        );
        let tx_to_cmp = transaction.clone();
        let sender = hex!("1f9090aaE28b8a3dCeaDf281B0F12828e676c326").into();
        provider.add_account(sender, ExtendedAccount::new(42, U256::MAX));
        let blob_store = InMemoryBlobStore::default();
        let validator = EthTransactionValidatorBuilder::new(MAINNET.clone())
            .build(provider, blob_store.clone());

        let txpool = Pool::new(
            validator.clone(),
            CoinbaseTipOrdering::default(),
            blob_store.clone(),
            Default::default(),
        );

        txpool.add_transaction(TransactionOrigin::Local, transaction.clone()).await.unwrap();

        let handle = tokio::runtime::Handle::current();
        let manager = TaskManager::new(handle);
        let config = LocalTransactionBackupConfig::with_local_txs_backup(transactions_path.clone());
        manager.executor().spawn_critical_with_graceful_shutdown_signal("test task", |shutdown| {
            backup_local_transactions_task(shutdown, txpool.clone(), config)
        });

        let mut txns = txpool.get_local_transactions();
        let tx_on_finish = txns.pop().expect("there should be 1 transaction");

        assert_eq!(*tx_to_cmp.hash(), *tx_on_finish.hash());

        // shutdown the executor
        manager.graceful_shutdown();

        let data = fs::read(transactions_path).unwrap();

        let txs: Vec<TransactionSigned> =
            alloy_rlp::Decodable::decode(&mut data.as_slice()).unwrap();
        assert_eq!(txs.len(), 1);

        temp_dir.close().unwrap();
    }
}
