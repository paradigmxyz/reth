//! Pool component for the node builder.

use alloy_primitives::Address;
use reth_chain_state::CanonStateSubscriptions;
use reth_node_api::TxTy;
use reth_transaction_pool::{PoolConfig, PoolTransaction, SubPoolLimit, TransactionPool};
use std::{collections::HashSet, future::Future};

use crate::{BuilderContext, FullNodeTypes};

/// A type that knows how to build the transaction pool.
pub trait PoolBuilder<Node: FullNodeTypes>: Send {
    /// The transaction pool to build.
    type Pool: TransactionPool<Transaction: PoolTransaction<Consensus = TxTy<Node::Types>>>
        + Unpin
        + 'static;

    /// Creates the transaction pool.
    fn build_pool(
        self,
        ctx: &BuilderContext<Node>,
    ) -> impl Future<Output = eyre::Result<Self::Pool>> + Send;
}

impl<Node, F, Fut, Pool> PoolBuilder<Node> for F
where
    Node: FullNodeTypes,
    Pool: TransactionPool<Transaction: PoolTransaction<Consensus = TxTy<Node::Types>>>
        + Unpin
        + 'static,
    F: FnOnce(&BuilderContext<Node>) -> Fut + Send,
    Fut: Future<Output = eyre::Result<Pool>> + Send,
{
    type Pool = Pool;

    fn build_pool(
        self,
        ctx: &BuilderContext<Node>,
    ) -> impl Future<Output = eyre::Result<Self::Pool>> {
        self(ctx)
    }
}

/// Convenience type to override cli or default pool configuration during build.
#[derive(Debug, Clone, Default)]
pub struct PoolBuilderConfigOverrides {
    /// Max number of transaction in the pending sub-pool
    pub pending_limit: Option<SubPoolLimit>,
    /// Max number of transaction in the basefee sub-pool
    pub basefee_limit: Option<SubPoolLimit>,
    /// Max number of transaction in the queued sub-pool
    pub queued_limit: Option<SubPoolLimit>,
    /// Max number of transactions in the blob sub-pool
    pub blob_limit: Option<SubPoolLimit>,
    /// Max number of executable transaction slots guaranteed per account
    pub max_account_slots: Option<usize>,
    /// Minimum base fee required by the protocol.
    pub minimal_protocol_basefee: Option<u64>,
    /// Addresses that will be considered as local. Above exemptions apply.
    pub local_addresses: HashSet<Address>,
    /// Additional tasks to validate new transactions.
    pub additional_validation_tasks: Option<usize>,
}

impl PoolBuilderConfigOverrides {
    /// Applies the configured overrides to the given [`PoolConfig`].
    pub fn apply(self, mut config: PoolConfig) -> PoolConfig {
        let Self {
            pending_limit,
            basefee_limit,
            queued_limit,
            blob_limit,
            max_account_slots,
            minimal_protocol_basefee,
            local_addresses,
            additional_validation_tasks: _,
        } = self;

        if let Some(pending_limit) = pending_limit {
            config.pending_limit = pending_limit;
        }
        if let Some(basefee_limit) = basefee_limit {
            config.basefee_limit = basefee_limit;
        }
        if let Some(queued_limit) = queued_limit {
            config.queued_limit = queued_limit;
        }
        if let Some(blob_limit) = blob_limit {
            config.blob_limit = blob_limit;
        }
        if let Some(max_account_slots) = max_account_slots {
            config.max_account_slots = max_account_slots;
        }
        if let Some(minimal_protocol_basefee) = minimal_protocol_basefee {
            config.minimal_protocol_basefee = minimal_protocol_basefee;
        }
        config.local_transactions_config.local_addresses.extend(local_addresses);

        config
    }
}

/// Common pool builder setup logic that can be used by different chain implementations
#[derive(Debug)]
pub struct PoolSetupHelper;

impl PoolSetupHelper {
    /// Create blob store with default configuration
    pub fn create_blob_store<Node>(
        ctx: &BuilderContext<Node>,
    ) -> eyre::Result<reth_transaction_pool::blobstore::DiskFileBlobStore>
    where
        Node: FullNodeTypes,
    {
        let data_dir = ctx.config().datadir();
        Ok(reth_transaction_pool::blobstore::DiskFileBlobStore::open(
            data_dir.blobstore(),
            Default::default(),
        )?)
    }

    /// Create blob store with custom cache size configuration for Ethereum
    pub fn create_blob_store_with_cache<Node>(
        ctx: &BuilderContext<Node>,
        cache_size: Option<u32>,
    ) -> eyre::Result<reth_transaction_pool::blobstore::DiskFileBlobStore>
    where
        Node: FullNodeTypes,
    {
        let data_dir = ctx.config().datadir();
        let config = if let Some(cache_size) = cache_size {
            reth_transaction_pool::blobstore::DiskFileBlobStoreConfig::default()
                .with_max_cached_entries(cache_size)
        } else {
            Default::default()
        };

        Ok(reth_transaction_pool::blobstore::DiskFileBlobStore::open(data_dir.blobstore(), config)?)
    }

    /// Spawn common local transaction backup task if enabled
    pub fn spawn_local_backup_task<Node, Pool>(
        ctx: &BuilderContext<Node>,
        pool: Pool,
    ) -> eyre::Result<()>
    where
        Node: FullNodeTypes,
        Pool: TransactionPool + Clone + 'static,
    {
        if !ctx.config().txpool.disable_transactions_backup {
            let data_dir = ctx.config().datadir();
            let transactions_path = ctx
                .config()
                .txpool
                .transactions_backup_path
                .clone()
                .unwrap_or_else(|| data_dir.txpool_transactions());

            let transactions_backup_config =
                reth_transaction_pool::maintain::LocalTransactionBackupConfig::with_local_txs_backup(transactions_path);

            ctx.task_executor().spawn_critical_with_graceful_shutdown_signal(
                "local transactions backup task",
                |shutdown| {
                    reth_transaction_pool::maintain::backup_local_transactions_task(
                        shutdown,
                        pool,
                        transactions_backup_config,
                    )
                },
            );
        }
        Ok(())
    }

    /// Spawn the main maintenance task for transaction pool
    pub fn spawn_maintenance_task_with_config<Node, Pool>(
        ctx: &BuilderContext<Node>,
        pool: Pool,
        pool_config: &PoolConfig,
    ) -> eyre::Result<()>
    where
        Node: FullNodeTypes,
        Node::Provider: reth_chain_state::CanonStateSubscriptions,
        Pool: reth_transaction_pool::TransactionPoolExt + Clone + 'static,
        Pool::Transaction:
            reth_transaction_pool::PoolTransaction<Consensus = reth_node_api::TxTy<Node::Types>>,
    {
        let chain_events = ctx.provider().canonical_state_stream();
        let client = ctx.provider().clone();

        ctx.task_executor().spawn_critical(
            "txpool maintenance task",
            reth_transaction_pool::maintain::maintain_transaction_pool_future(
                client,
                pool,
                chain_events,
                ctx.task_executor().clone(),
                reth_transaction_pool::maintain::MaintainPoolConfig {
                    max_tx_lifetime: pool_config.max_queued_lifetime,
                    no_local_exemptions: pool_config.local_transactions_config.no_exemptions,
                    ..Default::default()
                },
            ),
        );

        Ok(())
    }

    /// Spawn all common maintenance tasks (backup + main maintenance)
    pub fn spawn_all_maintenance_tasks_with_config<Node, Pool>(
        ctx: &BuilderContext<Node>,
        pool: Pool,
        pool_config: &PoolConfig,
    ) -> eyre::Result<()>
    where
        Node: FullNodeTypes,
        Node::Provider: reth_chain_state::CanonStateSubscriptions,
        Pool: reth_transaction_pool::TransactionPoolExt + Clone + 'static,
        Pool::Transaction:
            reth_transaction_pool::PoolTransaction<Consensus = reth_node_api::TxTy<Node::Types>>,
    {
        Self::spawn_local_backup_task(ctx, pool.clone())?;
        Self::spawn_maintenance_task_with_config(ctx, pool, pool_config)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reth_transaction_pool::PoolConfig;

    #[test]
    fn test_pool_builder_config_overrides_apply() {
        let base_config = PoolConfig::default();
        let overrides = PoolBuilderConfigOverrides {
            pending_limit: Some(SubPoolLimit::default()),
            max_account_slots: Some(100),
            minimal_protocol_basefee: Some(1000),
            ..Default::default()
        };

        let updated_config = overrides.apply(base_config);
        assert_eq!(updated_config.max_account_slots, 100);
        assert_eq!(updated_config.minimal_protocol_basefee, 1000);
    }

    #[test]
    fn test_pool_builder_config_overrides_default() {
        let overrides = PoolBuilderConfigOverrides::default();
        assert!(overrides.pending_limit.is_none());
        assert!(overrides.max_account_slots.is_none());
        assert!(overrides.local_addresses.is_empty());
    }

    #[test]
    fn test_pool_setup_helper_spawn_local_backup_task_compiles() {
        // this is a compile test to ensure the helper methods have correct signatures
        // we can't easily test the actual spawning without setting up a full context
        let helper = PoolSetupHelper;
        assert!(format!("{helper:?}").contains("PoolSetupHelper"));
    }
}
