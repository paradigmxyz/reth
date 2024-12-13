//! Ethereum Node types config.

use reth_basic_payload_builder::{BasicPayloadJobGenerator, BasicPayloadJobGeneratorConfig};
use reth_beacon_consensus::EthBeaconConsensus;
use reth_chainspec::ChainSpec;
use reth_ethereum_engine_primitives::{
    EthBuiltPayload, EthPayloadAttributes, EthPayloadBuilderAttributes,
};
use reth_ethereum_payload_builder::EthereumBuilderConfig;
use reth_evm::execute::BasicBlockExecutorProvider;
use reth_evm_ethereum::execute::EthExecutionStrategyFactory;
use reth_network::{EthNetworkPrimitives, NetworkHandle, PeersInfo};
use reth_node_api::{
    AddOnsContext, ConfigureEvm, FullNodeComponents, HeaderTy, NodeTypesWithDB, TxTy,
};
use reth_node_builder::{
    components::{
        ComponentsBuilder, ConsensusBuilder, ExecutorBuilder, NetworkBuilder,
        PayloadServiceBuilder, PoolBuilder,
    },
    node::{FullNodeTypes, NodeTypes, NodeTypesWithEngine},
    rpc::{EngineValidatorBuilder, RpcAddOns},
    BuilderContext, Node, NodeAdapter, NodeComponentsBuilder, PayloadBuilderConfig, PayloadTypes,
};
use reth_payload_builder::{PayloadBuilderHandle, PayloadBuilderService};
use reth_primitives::{EthPrimitives, PooledTransactionsElement};
use reth_provider::{CanonStateSubscriptions, EthStorage};
use reth_rpc::EthApi;
use reth_tracing::tracing::{debug, info};
use reth_transaction_pool::{
    blobstore::DiskFileBlobStore, EthTransactionPool, PoolTransaction, TransactionPool,
    TransactionValidationTaskExecutor,
};
use reth_trie_db::MerklePatriciaTrie;
use std::sync::Arc;

use crate::{EthEngineTypes, EthEvmConfig};

pub use reth_ethereum_engine_primitives::EthereumEngineValidator;

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
        Node: FullNodeTypes<Types: NodeTypes<ChainSpec = ChainSpec, Primitives = EthPrimitives>>,
        <Node::Types as NodeTypesWithEngine>::Engine: PayloadTypes<
            BuiltPayload = EthBuiltPayload,
            PayloadAttributes = EthPayloadAttributes,
            PayloadBuilderAttributes = EthPayloadBuilderAttributes,
        >,
    {
        ComponentsBuilder::default()
            .node_types::<Node>()
            .pool(EthereumPoolBuilder::default())
            .payload(EthereumPayloadBuilder::default())
            .network(EthereumNetworkBuilder::default())
            .executor(EthereumExecutorBuilder::default())
            .consensus(EthereumConsensusBuilder::default())
    }
}

impl NodeTypes for EthereumNode {
    type Primitives = EthPrimitives;
    type ChainSpec = ChainSpec;
    type StateCommitment = MerklePatriciaTrie;
    type Storage = EthStorage;
}

impl NodeTypesWithEngine for EthereumNode {
    type Engine = EthEngineTypes;
}

/// Add-ons w.r.t. l1 ethereum.
pub type EthereumAddOns<N> = RpcAddOns<
    N,
    EthApi<
        <N as FullNodeTypes>::Provider,
        <N as FullNodeComponents>::Pool,
        NetworkHandle,
        <N as FullNodeComponents>::Evm,
    >,
    EthereumEngineValidatorBuilder,
>;

impl<Types, N> Node<N> for EthereumNode
where
    Types: NodeTypesWithDB
        + NodeTypesWithEngine<
            Engine = EthEngineTypes,
            ChainSpec = ChainSpec,
            Primitives = EthPrimitives,
            Storage = EthStorage,
        >,
    N: FullNodeTypes<Types = Types>,
{
    type ComponentsBuilder = ComponentsBuilder<
        N,
        EthereumPoolBuilder,
        EthereumPayloadBuilder,
        EthereumNetworkBuilder,
        EthereumExecutorBuilder,
        EthereumConsensusBuilder,
    >;

    type AddOns = EthereumAddOns<
        NodeAdapter<N, <Self::ComponentsBuilder as NodeComponentsBuilder<N>>::Components>,
    >;

    fn components_builder(&self) -> Self::ComponentsBuilder {
        Self::components()
    }

    fn add_ons(&self) -> Self::AddOns {
        EthereumAddOns::default()
    }
}

/// A regular ethereum evm and executor builder.
#[derive(Debug, Default, Clone, Copy)]
#[non_exhaustive]
pub struct EthereumExecutorBuilder;

impl<Types, Node> ExecutorBuilder<Node> for EthereumExecutorBuilder
where
    Types: NodeTypesWithEngine<ChainSpec = ChainSpec, Primitives = EthPrimitives>,
    Node: FullNodeTypes<Types = Types>,
{
    type EVM = EthEvmConfig;
    type Executor = BasicBlockExecutorProvider<EthExecutionStrategyFactory>;

    async fn build_evm(
        self,
        ctx: &BuilderContext<Node>,
    ) -> eyre::Result<(Self::EVM, Self::Executor)> {
        let chain_spec = ctx.chain_spec();
        let evm_config = EthEvmConfig::new(ctx.chain_spec());
        let strategy_factory = EthExecutionStrategyFactory::new(chain_spec, evm_config.clone());
        let executor = BasicBlockExecutorProvider::new(strategy_factory);

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

impl<Types, Node> PoolBuilder<Node> for EthereumPoolBuilder
where
    Types: NodeTypesWithEngine<ChainSpec = ChainSpec, Primitives = EthPrimitives>,
    Node: FullNodeTypes<Types = Types>,
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
#[derive(Clone, Default, Debug)]
#[non_exhaustive]
pub struct EthereumPayloadBuilder;

impl EthereumPayloadBuilder {
    /// A helper method initializing [`PayloadBuilderService`] with the given EVM config.
    pub fn spawn<Types, Node, Evm, Pool>(
        self,
        evm_config: Evm,
        ctx: &BuilderContext<Node>,
        pool: Pool,
    ) -> eyre::Result<PayloadBuilderHandle<Types::Engine>>
    where
        Types: NodeTypesWithEngine<ChainSpec = ChainSpec, Primitives = EthPrimitives>,
        Node: FullNodeTypes<Types = Types>,
        Evm: ConfigureEvm<Header = HeaderTy<Types>, Transaction = TxTy<Node::Types>>,
        Pool: TransactionPool<Transaction: PoolTransaction<Consensus = TxTy<Node::Types>>>
            + Unpin
            + 'static,
        Types::Engine: PayloadTypes<
            BuiltPayload = EthBuiltPayload,
            PayloadAttributes = EthPayloadAttributes,
            PayloadBuilderAttributes = EthPayloadBuilderAttributes,
        >,
    {
        let conf = ctx.payload_builder_config();
        let payload_builder = reth_ethereum_payload_builder::EthereumPayloadBuilder::new(
            evm_config,
            EthereumBuilderConfig::new(conf.extradata_bytes()).with_gas_limit(conf.gas_limit()),
        );

        let payload_job_config = BasicPayloadJobGeneratorConfig::default()
            .interval(conf.interval())
            .deadline(conf.deadline())
            .max_payload_tasks(conf.max_payload_tasks());

        let payload_generator = BasicPayloadJobGenerator::with_builder(
            ctx.provider().clone(),
            pool,
            ctx.task_executor().clone(),
            payload_job_config,
            payload_builder,
        );
        let (payload_service, payload_builder) =
            PayloadBuilderService::new(payload_generator, ctx.provider().canonical_state_stream());

        ctx.task_executor().spawn_critical("payload builder service", Box::pin(payload_service));

        Ok(payload_builder)
    }
}

impl<Types, Node, Pool> PayloadServiceBuilder<Node, Pool> for EthereumPayloadBuilder
where
    Types: NodeTypesWithEngine<ChainSpec = ChainSpec, Primitives = EthPrimitives>,
    Node: FullNodeTypes<Types = Types>,
    Pool: TransactionPool<Transaction: PoolTransaction<Consensus = TxTy<Node::Types>>>
        + Unpin
        + 'static,
    Types::Engine: PayloadTypes<
        BuiltPayload = EthBuiltPayload,
        PayloadAttributes = EthPayloadAttributes,
        PayloadBuilderAttributes = EthPayloadBuilderAttributes,
    >,
{
    async fn spawn_payload_service(
        self,
        ctx: &BuilderContext<Node>,
        pool: Pool,
    ) -> eyre::Result<PayloadBuilderHandle<Types::Engine>> {
        self.spawn(EthEvmConfig::new(ctx.chain_spec()), ctx, pool)
    }
}

/// A basic ethereum payload service.
#[derive(Debug, Default, Clone, Copy)]
pub struct EthereumNetworkBuilder {
    // TODO add closure to modify network
}

impl<Node, Pool> NetworkBuilder<Node, Pool> for EthereumNetworkBuilder
where
    Node: FullNodeTypes<Types: NodeTypes<ChainSpec = ChainSpec, Primitives = EthPrimitives>>,
    Pool: TransactionPool<
            Transaction: PoolTransaction<
                Consensus = TxTy<Node::Types>,
                Pooled = PooledTransactionsElement,
            >,
        > + Unpin
        + 'static,
{
    type Primitives = EthNetworkPrimitives;

    async fn build_network(
        self,
        ctx: &BuilderContext<Node>,
        pool: Pool,
    ) -> eyre::Result<NetworkHandle> {
        let network = ctx.network_builder().await?;
        let handle = ctx.start_network(network, pool);
        info!(target: "reth::cli", enode=%handle.local_node_record(), "P2P networking initialized");
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
    Node: FullNodeTypes<Types: NodeTypes<ChainSpec = ChainSpec, Primitives = EthPrimitives>>,
{
    type Consensus = Arc<dyn reth_consensus::FullConsensus>;

    async fn build_consensus(self, ctx: &BuilderContext<Node>) -> eyre::Result<Self::Consensus> {
        Ok(Arc::new(EthBeaconConsensus::new(ctx.chain_spec())))
    }
}

/// Builder for [`EthereumEngineValidator`].
#[derive(Debug, Default, Clone)]
#[non_exhaustive]
pub struct EthereumEngineValidatorBuilder;

impl<Node, Types> EngineValidatorBuilder<Node> for EthereumEngineValidatorBuilder
where
    Types: NodeTypesWithEngine<
        ChainSpec = ChainSpec,
        Engine = EthEngineTypes,
        Primitives = EthPrimitives,
    >,
    Node: FullNodeComponents<Types = Types>,
{
    type Validator = EthereumEngineValidator;

    async fn build(self, ctx: &AddOnsContext<'_, Node>) -> eyre::Result<Self::Validator> {
        Ok(EthereumEngineValidator::new(ctx.config.chain.clone()))
    }
}
