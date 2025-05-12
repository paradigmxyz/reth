//! Support for maintaining the state of the transaction pool

mod canon_processor;
mod drift_monitor;
#[cfg(test)]
mod tests;

use crate::{
    blobstore::BlobStoreUpdates,
    error::PoolError,
    metrics::MaintainPoolMetrics,
    traits::{TransactionPool, TransactionPoolExt},
    BlockInfo, PoolTransaction,
};
use alloy_consensus::BlockHeader;
use alloy_eips::BlockNumberOrTag;
use alloy_rlp::Encodable;
use futures_util::{future::BoxFuture, FutureExt, Stream, StreamExt};
use reth_chain_state::CanonStateNotification;
use reth_chainspec::{ChainSpecProvider, EthChainSpec};
use reth_fs_util::FsPathError;
use reth_primitives_traits::{
    transaction::signed::SignedTransaction, NodePrimitives, SealedHeader,
};
use reth_storage_api::{BlockReaderIdExt, StateProviderFactory};
use reth_tasks::TaskSpawner;
use std::path::{Path, PathBuf};
use tokio::time::{self, Duration};
use tracing::{debug, error, info, trace, warn};

// Re-export key components
pub use canon_processor::{CanonEventProcessor, CanonEventProcessorConfig, FinalizedBlockTracker};
pub use drift_monitor::{DriftMonitor, LoadedAccounts, PoolDriftState};

/// Maximum amount of time non-executable transaction are queued.
pub const MAX_QUEUED_TRANSACTION_LIFETIME: Duration = Duration::from_secs(3 * 60 * 60);

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

    /// Maximum amount of time non-executable, non local transactions are queued.
    /// Default: 3 hours
    pub max_tx_lifetime: Duration,

    /// Apply no exemptions to the locally received transactions.
    ///
    /// This includes:
    ///   - no price exemptions
    ///   - no eviction exemptions
    pub no_local_exemptions: bool,
}

impl Default for MaintainPoolConfig {
    fn default() -> Self {
        Self {
            max_update_depth: 64,
            max_reload_accounts: 100,
            max_tx_lifetime: MAX_QUEUED_TRANSACTION_LIFETIME,
            no_local_exemptions: false,
        }
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
pub fn maintain_transaction_pool_future<N, Client, P, St, Tasks>(
    client: Client,
    pool: P,
    events: St,
    task_spawner: Tasks,
    config: MaintainPoolConfig,
) -> BoxFuture<'static, ()>
where
    N: NodePrimitives,
    Client: StateProviderFactory + BlockReaderIdExt + ChainSpecProvider + Clone + 'static,
    P: TransactionPoolExt<Transaction: PoolTransaction<Consensus = N::SignedTx>> + 'static,
    St: Stream<Item = CanonStateNotification<N>> + Send + Unpin + 'static,
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
pub async fn maintain_transaction_pool<N, Client, P, St, Tasks>(
    client: Client,
    pool: P,
    mut events: St,
    task_spawner: Tasks,
    config: MaintainPoolConfig,
) where
    N: NodePrimitives,
    Client: StateProviderFactory + BlockReaderIdExt + ChainSpecProvider + Clone + 'static,
    P: TransactionPoolExt<Transaction: PoolTransaction<Consensus = N::SignedTx>> + 'static,
    St: Stream<Item = CanonStateNotification<N>> + Send + Unpin + 'static,
    Tasks: TaskSpawner + 'static,
{
    let metrics = MaintainPoolMetrics::default();
    let MaintainPoolConfig { max_update_depth, max_reload_accounts, .. } = config;

    // ensure the pool points to latest state
    if let Ok(Some(latest)) = client.header_by_number_or_tag(BlockNumberOrTag::Latest) {
        let latest = SealedHeader::seal_slow(latest);
        let chain_spec = client.chain_spec();
        let info = BlockInfo {
            block_gas_limit: latest.gas_limit(),
            last_seen_block_hash: latest.hash(),
            last_seen_block_number: latest.number(),
            pending_basefee: latest
                .next_block_base_fee(chain_spec.base_fee_params_at_timestamp(latest.timestamp()))
                .unwrap_or_default(),
            pending_blob_fee: latest
                .maybe_next_block_blob_fee(chain_spec.blob_params_at_timestamp(latest.timestamp())),
        };
        pool.set_block_info(info);
    }

    // Create our new components
    let mut drift_monitor = DriftMonitor::new(max_reload_accounts, metrics.clone());

    let canon_processor_config = CanonEventProcessorConfig { max_update_depth };

    let mut canon_processor = CanonEventProcessor::new(
        client.finalized_block_number().ok().flatten(),
        canon_processor_config,
        metrics.clone(),
    );

    // eviction interval for stale non local txs
    let mut stale_eviction_interval = time::interval(config.max_tx_lifetime);

    // The update loop that waits for new blocks and reorgs and performs pool updates
    // Listen for new chain events and derive the update action for the pool
    loop {
        trace!(target: "txpool", state=?drift_monitor.state(), "awaiting new block or reorg");

        drift_monitor.update_metrics();
        let pool_info = pool.block_info();

        // after performing a pool update after a new block we have some time to properly update
        // dirty accounts and correct if the pool drifted from current state, for example after
        // restart or a pipeline run
        if drift_monitor.state().is_drifted() {
            // assuming all senders are dirty
            drift_monitor.set_dirty_addresses(pool.unique_senders());
            // make sure we toggle the state back to in sync
            drift_monitor.set_state(PoolDriftState::InSync);
        }

        // if we have accounts that are out of sync with the pool, we reload them in chunks
        if drift_monitor.has_dirty_addresses() && !drift_monitor.is_reloading() {
            drift_monitor.start_reload_accounts(
                client.clone(),
                pool_info.last_seen_block_hash,
                &task_spawner,
            );
        }

        // check if we have a new finalized block
        if let Some(BlobStoreUpdates::Finalized(blobs)) = canon_processor.update_finalized(&client)
        {
            metrics.inc_deleted_tracked_blobs(blobs.len());
            // remove all finalized blobs from the blob store
            pool.delete_blobs(blobs);
            // and also do periodic cleanup
            let pool = pool.clone();
            task_spawner.spawn_blocking(Box::pin(async move {
                debug!(target: "txpool", "cleaning up blob store");
                pool.cleanup_blobs();
            }));
        }

        // select of account reloads and new canonical state updates which should arrive at the rate
        // of the block time
        tokio::select! {
            // handle reloaded accounts
            _ = async { futures_util::future::pending::<()>().await }, if !drift_monitor.is_reloading() => {
                // this branch will never execute, it's just a placeholder
                // for when we don't have account reloads running
            }
            _ = async {}, if drift_monitor.is_reloading() => {
                if let drift_monitor::DriftMonitorResult::AccountsLoaded(LoadedAccounts { accounts, .. }) = drift_monitor.process_reload_result() {
                     // update the pool with the loaded accounts
                     pool.update_accounts(accounts);
                 }
            }

            // handle new canonical events
            ev = events.next() => {
                if ev.is_none() {
                    // the stream ended, we are done
                    break;
                }

                // on receiving the first event on start up, mark the pool as drifted to explicitly
                // trigger revalidation and clear out outdated txs.
                drift_monitor.on_first_event();

                // process the event
                if let Some(event) = ev {
                    // try processing as a reorg first
                    if !canon_processor.process_reorg(event.clone(), &client, &pool, &mut drift_monitor).await {
                        // if not a reorg, try as a commit
                        canon_processor.process_commit(event, &client, &pool, &mut drift_monitor);
                    }
                }
            }

            // handle stale transaction eviction
            _ = stale_eviction_interval.tick() => {
                let stale_txs: Vec<_> = pool
                    .queued_transactions()
                    .into_iter()
                    .filter(|tx| {
                        // filter stale transactions based on config
                        (tx.origin.is_external() || config.no_local_exemptions) && tx.timestamp.elapsed() > config.max_tx_lifetime
                    })
                    .map(|tx| *tx.hash())
                    .collect();
                debug!(target: "txpool", count=%stale_txs.len(), "removing stale transactions");
                pool.remove_transactions(stale_txs);
            }
        }
    }
}

/// Loads transactions from a file, decodes them from the RLP format, and inserts them
/// into the transaction pool on node boot up.
/// The file is removed after the transactions have been successfully processed.
async fn load_and_reinsert_transactions<P>(
    pool: P,
    file_path: &Path,
) -> Result<(), TransactionsBackupError>
where
    P: TransactionPool<Transaction: PoolTransaction<Consensus: SignedTransaction>>,
{
    if !file_path.exists() {
        return Ok(())
    }

    debug!(target: "txpool", txs_file =?file_path, "Check local persistent storage for saved transactions");
    let data = reth_fs_util::read(file_path)?;

    if data.is_empty() {
        return Ok(())
    }

    let txs_signed: Vec<<P::Transaction as PoolTransaction>::Consensus> =
        alloy_rlp::Decodable::decode(&mut data.as_slice())?;

    let pool_transactions = txs_signed
        .into_iter()
        .filter_map(|tx| tx.try_clone_into_recovered().ok())
        .filter_map(|tx| {
            // Filter out errors
            <P::Transaction as PoolTransaction>::try_from_consensus(tx).ok()
        })
        .collect();

    let outcome = pool.add_transactions(crate::TransactionOrigin::Local, pool_transactions).await;

    info!(target: "txpool", txs_file =?file_path, num_txs=%outcome.len(), "Successfully reinserted local transactions from file");
    reth_fs_util::remove_file(file_path)?;
    Ok(())
}

fn save_local_txs_backup<P>(pool: P, file_path: &Path)
where
    P: TransactionPool<Transaction: PoolTransaction<Consensus: Encodable>>,
{
    let local_transactions = pool.get_local_transactions();
    if local_transactions.is_empty() {
        trace!(target: "txpool", "no local transactions to save");
        return
    }

    let local_transactions = local_transactions
        .into_iter()
        .map(|tx| tx.transaction.clone_into_consensus().into_inner())
        .collect::<Vec<_>>();

    let num_txs = local_transactions.len();
    let mut buf = Vec::new();
    alloy_rlp::encode_list(&local_transactions, &mut buf);
    info!(target: "txpool", txs_file =?file_path, num_txs=%num_txs, "Saving current local transactions");
    let parent_dir = file_path.parent().map(std::fs::create_dir_all).transpose();

    match parent_dir.map(|_| reth_fs_util::write(file_path, buf)) {
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
    P: TransactionPool<Transaction: PoolTransaction<Consensus: SignedTransaction>> + Clone,
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
