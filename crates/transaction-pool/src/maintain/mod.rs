//! Support for maintaining the state of the transaction pool

use crate::{
    maintain::pool_maintainer::PoolMaintainerBuilder, traits::TransactionPoolExt, PoolTransaction,
};
use futures_util::{future::BoxFuture, Stream};
use reth_chain_state::CanonStateNotification;
use reth_chainspec::ChainSpecProvider;
use reth_primitives_traits::NodePrimitives;
use reth_storage_api::{BlockReaderIdExt, StateProviderFactory};
use reth_tasks::TaskSpawner;
use std::path::PathBuf;
use tokio::time::Duration;

mod backup;
mod canon_processor;
mod drift_monitor;
mod interfaces;
mod pool_maintainer;
#[cfg(test)]
mod tests;

// Re-export key components
pub use backup::backup_local_transactions_task;
pub use canon_processor::{CanonEventProcessor, CanonEventProcessorConfig, FinalizedBlockTracker};
pub use drift_monitor::{DriftMonitor, LoadedAccounts, PoolDriftState};
pub use interfaces::{CanonProcessing, DefaultComponentFactory, PoolMaintainerComponentFactory};

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
    Client: StateProviderFactory + BlockReaderIdExt + ChainSpecProvider + Clone + Unpin + 'static,
    P: TransactionPoolExt<Transaction: PoolTransaction<Consensus = N::SignedTx>> + Unpin + 'static,
    St: Stream<Item = CanonStateNotification<N>> + Send + Unpin + 'static,
    Tasks: TaskSpawner + 'static,
{
    let maintainer = PoolMaintainerBuilder::new(client, pool, events, task_spawner, config).build();

    Box::pin(maintainer)
}
