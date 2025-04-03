use crate::chainspec::BscChainSpec;
use reth::{
    api::{FullNodeTypes, NodeTypes},
    builder::{components::PoolBuilder, BuilderContext},
    transaction_pool::{
        blobstore::InMemoryBlobStore,
        maintain::{
            backup_local_transactions_task, maintain_transaction_pool_future,
            LocalTransactionBackupConfig, MaintainPoolConfig,
        },
        EthTransactionPool, Pool, PoolConfig, TransactionValidationTaskExecutor,
    },
};
use reth_primitives::EthPrimitives;
use reth_provider::CanonStateSubscriptions;
use tracing::{debug, info};

/// A bsc pool builder
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct BscPoolBuilder {
    /// Use custom pool config
    pool_config: PoolConfig,
}

/// Implement the [`PoolBuilder`] trait for the bsc pool builder
///
/// This will be used to build the transaction pool and its maintenance tasks during launch.
impl<Node> PoolBuilder<Node> for BscPoolBuilder
where
    Node: FullNodeTypes<Types: NodeTypes<ChainSpec = BscChainSpec, Primitives = EthPrimitives>>,
{
    type Pool = EthTransactionPool<Node::Provider, InMemoryBlobStore>; // TODO: no blob store for bscuse reth::transaction_pool::Pool;

    async fn build_pool(self, ctx: &BuilderContext<Node>) -> eyre::Result<Self::Pool> {
        let data_dir = ctx.config().datadir();
        let blob_store = InMemoryBlobStore::default();
        let validator = TransactionValidationTaskExecutor::eth_builder(ctx.provider().clone())
            .with_head_timestamp(ctx.head().timestamp)
            .kzg_settings(ctx.kzg_settings()?)
            .with_additional_tasks(ctx.config().txpool.additional_validation_tasks)
            .build_with_tasks(ctx.task_executor().clone(), blob_store.clone());

        let transaction_pool = Pool::eth_pool(validator, blob_store, self.pool_config);
        info!(target: "reth::cli", "Transaction pool initialized");
        let transactions_path = data_dir.txpool_transactions();

        // spawn txpool maintenance task
        {
            let pool = transaction_pool.clone();
            let chain_events = ctx.provider().canonical_state_stream();
            let client = ctx.provider().clone();
            let transactions_backup_config =
                LocalTransactionBackupConfig::with_local_txs_backup(transactions_path);

            ctx.task_executor().spawn_critical_with_graceful_shutdown_signal(
                "local transactions backup task",
                |shutdown| {
                    backup_local_transactions_task(
                        shutdown,
                        pool.clone(),
                        transactions_backup_config,
                    )
                },
            );

            // spawn the maintenance task
            ctx.task_executor().spawn_critical(
                "txpool maintenance task",
                maintain_transaction_pool_future(
                    client,
                    pool,
                    chain_events,
                    ctx.task_executor().clone(),
                    MaintainPoolConfig {
                        max_tx_lifetime: transaction_pool.config().max_queued_lifetime,
                        ..Default::default()
                    },
                ),
            );
            debug!(target: "reth::cli", "Spawned txpool maintenance task");
        }

        Ok(transaction_pool)
    }
}
