//! Support for maintaining the state of the transaction pool

use crate::{
    error::PoolError,
    traits::{TransactionPool, TransactionPoolExt},
    PoolTransaction,
};
use alloy_rlp::Encodable;
use futures_util::{future::BoxFuture, Stream};
use reth_chain_state::CanonStateNotification;
use reth_chainspec::ChainSpecProvider;
use reth_fs_util::FsPathError;
use reth_primitives_traits::{transaction::signed::SignedTransaction, NodePrimitives};
use reth_storage_api::{BlockReaderIdExt, StateProviderFactory};
use reth_tasks::TaskSpawner;
use std::path::{Path, PathBuf};
use tokio::time::Duration;
use tracing::{debug, error, info, trace, warn};

mod canon_processor;
mod drift_monitor;
mod interfaces;
mod pool_maintainer;
#[cfg(test)]
mod tests;

// Re-export key components
pub use canon_processor::{CanonEventProcessor, CanonEventProcessorConfig, FinalizedBlockTracker};
pub use drift_monitor::{DriftMonitor, LoadedAccounts, PoolDriftState};
pub use interfaces::{
    CanonProcessing, DefaultComponentFactory, DriftMonitoring, PoolMaintainerComponentFactory,
};

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

// Re-export PoolMaintainer for public use
pub use pool_maintainer::{PoolMaintainer, PoolMaintainerBuilder};

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
    Client: StateProviderFactory + BlockReaderIdExt + ChainSpecProvider + Clone + Unpin + 'static,
    P: TransactionPoolExt<Transaction: PoolTransaction<Consensus = N::SignedTx>> + Unpin + 'static,
    St: Stream<Item = CanonStateNotification<N>> + Send + Unpin + 'static,
    Tasks: TaskSpawner + 'static,
{
    let maintainer = PoolMaintainerBuilder::new(client, pool, events, task_spawner, config).build();

    Box::pin(maintainer)
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
