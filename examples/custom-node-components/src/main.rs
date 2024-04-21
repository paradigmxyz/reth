//! This example shows how to configure custom components for a reth node.

#![cfg_attr(not(test), warn(unused_crate_dependencies))]

use reth::{
    builder::{components::PoolBuilder, BuilderContext, FullNodeTypes},
    cli::Cli,
    providers::CanonStateSubscriptions,
    transaction_pool::{
        blobstore::InMemoryBlobStore, EthTransactionPool, TransactionValidationTaskExecutor,
    },
};
use reth_node_ethereum::EthereumNode;
use reth_tracing::tracing::{debug, info};
use reth_transaction_pool::PoolConfig;

fn main() {
    Cli::parse_args()
        .run(|builder, _| async move {
            let handle = builder
                // use the default ethereum node types
                .with_types(EthereumNode::default())
                // Configure the components of the node
                // use default ethereum components but use our custom pool
                .with_components(EthereumNode::components().pool(CustomPoolBuilder::default()))
                .launch()
                .await?;

            handle.wait_for_node_exit().await
        })
        .unwrap();
}

/// A custom pool builder
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct CustomPoolBuilder {
    /// Use custom pool config
    pool_config: PoolConfig,
}

/// Implement the `PoolBuilder` trait for the custom pool builder
///
/// This will be used to build the transaction pool and its maintenance tasks during launch.
impl<Node> PoolBuilder<Node> for CustomPoolBuilder
where
    Node: FullNodeTypes,
{
    type Pool = EthTransactionPool<Node::Provider, InMemoryBlobStore>;

    async fn build_pool(self, ctx: &BuilderContext<Node>) -> eyre::Result<Self::Pool> {
        let data_dir = ctx.data_dir();
        let blob_store = InMemoryBlobStore::default();
        let validator = TransactionValidationTaskExecutor::eth_builder(ctx.chain_spec())
            .with_head_timestamp(ctx.head().timestamp)
            .kzg_settings(ctx.kzg_settings()?)
            .with_additional_tasks(5)
            .build_with_tasks(
                ctx.provider().clone(),
                ctx.task_executor().clone(),
                blob_store.clone(),
            );

        let transaction_pool =
            reth_transaction_pool::Pool::eth_pool(validator, blob_store, self.pool_config);
        info!(target: "reth::cli", "Transaction pool initialized");
        let transactions_path = data_dir.txpool_transactions_path();

        // spawn txpool maintenance task
        {
            let pool = transaction_pool.clone();
            let chain_events = ctx.provider().canonical_state_stream();
            let client = ctx.provider().clone();
            let transactions_backup_config =
                reth_transaction_pool::maintain::LocalTransactionBackupConfig::with_local_txs_backup(transactions_path);

            ctx.task_executor().spawn_critical_with_graceful_shutdown_signal(
                "local transactions backup task",
                |shutdown| {
                    reth_transaction_pool::maintain::backup_local_transactions_task(
                        shutdown,
                        pool.clone(),
                        transactions_backup_config,
                    )
                },
            );

            // spawn the maintenance task
            ctx.task_executor().spawn_critical(
                "txpool maintenance task",
                reth_transaction_pool::maintain::maintain_transaction_pool_future(
                    client,
                    pool,
                    chain_events,
                    ctx.task_executor().clone(),
                    Default::default(),
                ),
            );
            debug!(target: "reth::cli", "Spawned txpool maintenance task");
        }

        Ok(transaction_pool)
    }
}
