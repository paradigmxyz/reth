//! Ethereum Node types config.

use std::sync::Arc;

use reth_auto_seal_consensus::AutoSealConsensus;
use reth_basic_payload_builder::{BasicPayloadJobGenerator, BasicPayloadJobGeneratorConfig};
use reth_beacon_consensus::EthBeaconConsensus;
use reth_ethereum_engine_primitives::{
    EthBuiltPayload, EthPayloadAttributes, EthPayloadBuilderAttributes,
};
use reth_evm_ethereum::execute::EthExecutorProvider;
use reth_network::NetworkHandle;
use reth_node_api::{FullNodeComponents, NodeAddOns};
use reth_node_builder::{
    components::{
        ComponentsBuilder, ConsensusBuilder, ExecutorBuilder, NetworkBuilder,
        PayloadServiceBuilder, PoolBuilder,
    },
    node::{FullNodeTypes, NodeTypes},
    BuilderContext, ConfigureEvm, Node, PayloadBuilderConfig, PayloadTypes,
};
use reth_payload_builder::{PayloadBuilderHandle, PayloadBuilderService};
use reth_provider::CanonStateSubscriptions;
use reth_rpc::EthApi;
use reth_tracing::tracing::{debug, info};
use reth_transaction_pool::{
    blobstore::DiskFileBlobStore, EthTransactionPool, TransactionPool,
    TransactionValidationTaskExecutor,
};

use crate::{EthEngineTypes, EthEvmConfig};

/// Type configuration for a regular Ethereum node.
#[derive(Debug, Default, Clone, Copy)]
#[non_exhaustive]
pub struct EthereumNode;

impl EthereumNode {
    /// Returns a [`ComponentsBuilder`] configured for a regular Ethereum node.
    pub fn components<Node>() -> ComponentsBuilder<
        Node,
        EthereumPoolBuilder,
        EthereumPayloadBuilder,
        EthereumNetworkBuilder,
        EthereumExecutorBuilder,
        EthereumConsensusBuilder,
    >
    where
        Node: FullNodeTypes,
        <Node as NodeTypes>::Engine: PayloadTypes<
            BuiltPayload = EthBuiltPayload,
            PayloadAttributes = EthPayloadAttributes,
            PayloadBuilderAttributes = EthPayloadBuilderAttributes,
        >,
    {
        ComponentsBuilder::default()
            .node_types::<Node>()
            .pool(EthereumPoolBuilder::default())
            .payload(EthereumPayloadBuilder::new(EthEvmConfig::default()))
            .network(EthereumNetworkBuilder::default())
            .executor(EthereumExecutorBuilder::default())
            .consensus(EthereumConsensusBuilder::default())
    }
}

impl NodeTypes for EthereumNode {
    type Primitives = ();
    type Engine = EthEngineTypes;
}

/// Add-ons w.r.t. l1 ethereum.
#[derive(Debug, Clone)]
pub struct EthereumAddOns;

impl<N: FullNodeComponents> NodeAddOns<N> for EthereumAddOns {
    type EthApi = EthApi<N::Provider, N::Pool, NetworkHandle, N::Evm>;
}

impl<N> Node<N> for EthereumNode
where
    N: FullNodeTypes<Engine = EthEngineTypes>,
{
    type ComponentsBuilder = ComponentsBuilder<
        N,
        EthereumPoolBuilder,
        EthereumPayloadBuilder,
        EthereumNetworkBuilder,
        EthereumExecutorBuilder,
        EthereumConsensusBuilder,
    >;

    type AddOns = EthereumAddOns;

    fn components_builder(&self) -> Self::ComponentsBuilder {
        Self::components()
    }
}

/// A regular ethereum evm and executor builder.
#[derive(Debug, Default, Clone, Copy)]
#[non_exhaustive]
pub struct EthereumExecutorBuilder;

impl<Node> ExecutorBuilder<Node> for EthereumExecutorBuilder
where
    Node: FullNodeTypes,
{
    type EVM = EthEvmConfig;
    type Executor = EthExecutorProvider<Self::EVM>;

    async fn build_evm(
        self,
        ctx: &BuilderContext<Node>,
    ) -> eyre::Result<(Self::EVM, Self::Executor)> {
        let chain_spec = ctx.chain_spec();
        let evm_config = EthEvmConfig::default();
        let executor = EthExecutorProvider::new(chain_spec, evm_config);

        Ok((evm_config, executor))
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
        let data_dir = ctx.config().datadir();
        let pool_config = ctx.pool_config();
        let blob_store = DiskFileBlobStore::open(data_dir.blobstore(), Default::default())?;
        let validator = TransactionValidationTaskExecutor::eth_builder(ctx.chain_spec())
            .with_head_timestamp(ctx.head().timestamp)
            .kzg_settings(ctx.kzg_settings()?)
            .with_local_transactions_config(pool_config.local_transactions_config.clone())
            .with_additional_tasks(ctx.config().txpool.additional_validation_tasks)
            .build_with_tasks(
                ctx.provider().clone(),
                ctx.task_executor().clone(),
                blob_store.clone(),
            );

        let transaction_pool =
            reth_transaction_pool::Pool::eth_pool(validator, blob_store, pool_config);
        info!(target: "reth::cli", "Transaction pool initialized");
        let transactions_path = data_dir.txpool_transactions();

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
pub struct EthereumPayloadBuilder<Evm = EthEvmConfig> {
    /// The EVM configuration to use for the payload builder.
    pub evm_config: Evm,
}

impl<EVM> EthereumPayloadBuilder<EVM> {
    /// Create a new instance with the given evm config.
    pub const fn new(evm_config: EVM) -> Self {
        Self { evm_config }
    }
}

impl<Node, Evm, Pool> PayloadServiceBuilder<Node, Pool> for EthereumPayloadBuilder<Evm>
where
    Node: FullNodeTypes,
    Evm: ConfigureEvm,
    Pool: TransactionPool + Unpin + 'static,
    <Node as NodeTypes>::Engine: PayloadTypes<
        BuiltPayload = EthBuiltPayload,
        PayloadAttributes = EthPayloadAttributes,
        PayloadBuilderAttributes = EthPayloadBuilderAttributes,
    >,
{
    async fn spawn_payload_service(
        self,
        ctx: &BuilderContext<Node>,
        pool: Pool,
    ) -> eyre::Result<PayloadBuilderHandle<Node::Engine>> {
        let payload_builder =
            reth_ethereum_payload_builder::EthereumPayloadBuilder::new(self.evm_config);
        let conf = ctx.payload_builder_config();

        let payload_job_config = BasicPayloadJobGeneratorConfig::default()
            .interval(conf.interval())
            .deadline(conf.deadline())
            .max_payload_tasks(conf.max_payload_tasks())
            .extradata(conf.extradata_bytes());

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

/// A basic ethereum consensus builder.
#[derive(Debug, Default, Clone, Copy)]
pub struct EthereumConsensusBuilder {
    // TODO add closure to modify consensus
}

impl<Node> ConsensusBuilder<Node> for EthereumConsensusBuilder
where
    Node: FullNodeTypes,
{
    type Consensus = Arc<dyn reth_consensus::Consensus>;

    async fn build_consensus(self, ctx: &BuilderContext<Node>) -> eyre::Result<Self::Consensus> {
        if ctx.is_dev() {
            Ok(Arc::new(AutoSealConsensus::new(ctx.chain_spec())))
        } else {
            Ok(Arc::new(EthBeaconConsensus::new(ctx.chain_spec())))
        }
    }
}
