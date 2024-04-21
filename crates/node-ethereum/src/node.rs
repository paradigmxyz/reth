//! Ethereum Node types config.

use crate::{EthEngineTypes, EthEvmConfig};
use reth_basic_payload_builder::{BasicPayloadJobGenerator, BasicPayloadJobGeneratorConfig};
use reth_network::NetworkHandle;
use reth_node_builder::{
    components::{ComponentsBuilder, NetworkBuilder, PayloadServiceBuilder, PoolBuilder},
    node::{FullNodeTypes, Node, NodeTypes},
    BuilderContext, PayloadBuilderConfig,
};
use reth_payload_builder::{PayloadBuilderHandle, PayloadBuilderService};
use reth_provider::CanonStateSubscriptions;
use reth_tracing::tracing::{debug, info};
use reth_transaction_pool::{
    blobstore::DiskFileBlobStore, EthTransactionPool, TransactionPool,
    TransactionValidationTaskExecutor,
};

/// Type configuration for a regular Ethereum node.
#[derive(Debug, Default, Clone, Copy)]
#[non_exhaustive]
pub struct EthereumNode;

impl EthereumNode {
    /// Returns a [ComponentsBuilder] configured for a regular Ethereum node.
    pub fn components<Node>(
    ) -> ComponentsBuilder<Node, EthereumPoolBuilder, EthereumPayloadBuilder, EthereumNetworkBuilder>
    where
        Node: FullNodeTypes<Engine = EthEngineTypes>,
    {
        ComponentsBuilder::default()
            .node_types::<Node>()
            .pool(EthereumPoolBuilder::default())
            .payload(EthereumPayloadBuilder::default())
            .network(EthereumNetworkBuilder::default())
    }
}

impl NodeTypes for EthereumNode {
    type Primitives = ();
    type Engine = EthEngineTypes;
    type Evm = EthEvmConfig;

    fn evm_config(&self) -> Self::Evm {
        EthEvmConfig::default()
    }
}

impl<N> Node<N> for EthereumNode
where
    N: FullNodeTypes<Engine = EthEngineTypes>,
{
    type PoolBuilder = EthereumPoolBuilder;
    type NetworkBuilder = EthereumNetworkBuilder;
    type PayloadBuilder = EthereumPayloadBuilder;

    fn components(
        self,
    ) -> ComponentsBuilder<N, Self::PoolBuilder, Self::PayloadBuilder, Self::NetworkBuilder> {
        ComponentsBuilder::default()
            .node_types::<N>()
            .pool(EthereumPoolBuilder::default())
            .payload(EthereumPayloadBuilder::default())
            .network(EthereumNetworkBuilder::default())
    }
}

/// A basic ethereum transaction pool.
///
/// This contains various settings that can be configured and take precedence over the node's
/// config.
#[derive(Debug, Default, Clone, Copy)]
#[non_exhaustive]
pub struct EthereumPoolBuilder {
    // TODO add options for txpool args
}

impl<Node> PoolBuilder<Node> for EthereumPoolBuilder
where
    Node: FullNodeTypes,
{
    type Pool = EthTransactionPool<Node::Provider, DiskFileBlobStore>;

    async fn build_pool(self, ctx: &BuilderContext<Node>) -> eyre::Result<Self::Pool> {
        let data_dir = ctx.data_dir();
        let blob_store = DiskFileBlobStore::open(data_dir.blobstore_path(), Default::default())?;
        let validator = TransactionValidationTaskExecutor::eth_builder(ctx.chain_spec())
            .with_head_timestamp(ctx.head().timestamp)
            .kzg_settings(ctx.kzg_settings()?)
            .with_additional_tasks(1)
            .build_with_tasks(
                ctx.provider().clone(),
                ctx.task_executor().clone(),
                blob_store.clone(),
            );

        let transaction_pool =
            reth_transaction_pool::Pool::eth_pool(validator, blob_store, ctx.pool_config());
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

/// A basic ethereum payload service.
#[derive(Debug, Default, Clone)]
#[non_exhaustive]
pub struct EthereumPayloadBuilder;

impl<Node, Pool> PayloadServiceBuilder<Node, Pool> for EthereumPayloadBuilder
where
    Node: FullNodeTypes<Engine = EthEngineTypes>,
    Pool: TransactionPool + Unpin + 'static,
{
    async fn spawn_payload_service(
        self,
        ctx: &BuilderContext<Node>,
        pool: Pool,
    ) -> eyre::Result<PayloadBuilderHandle<Node::Engine>> {
        let payload_builder = reth_ethereum_payload_builder::EthereumPayloadBuilder::default();
        let conf = ctx.payload_builder_config();

        let payload_job_config = BasicPayloadJobGeneratorConfig::default()
            .interval(conf.interval())
            .deadline(conf.deadline())
            .max_payload_tasks(conf.max_payload_tasks())
            .extradata(conf.extradata_bytes())
            .max_gas_limit(conf.max_gas_limit());

        let payload_generator = BasicPayloadJobGenerator::with_builder(
            ctx.provider().clone(),
            pool,
            ctx.task_executor().clone(),
            payload_job_config,
            ctx.chain_spec(),
            payload_builder,
        );
        let (payload_service, payload_builder) =
            PayloadBuilderService::new(payload_generator, ctx.provider().canonical_state_stream());

        ctx.task_executor().spawn_critical("payload builder service", Box::pin(payload_service));

        Ok(payload_builder)
    }
}

/// A basic ethereum payload service.
#[derive(Debug, Default, Clone, Copy)]
pub struct EthereumNetworkBuilder {
    // TODO add closure to modify network
}

impl<Node, Pool> NetworkBuilder<Node, Pool> for EthereumNetworkBuilder
where
    Node: FullNodeTypes,
    Pool: TransactionPool + Unpin + 'static,
{
    async fn build_network(
        self,
        ctx: &BuilderContext<Node>,
        pool: Pool,
    ) -> eyre::Result<NetworkHandle> {
        let network = ctx.network_builder().await?;
        let handle = ctx.start_network(network, pool);

        Ok(handle)
    }
}
