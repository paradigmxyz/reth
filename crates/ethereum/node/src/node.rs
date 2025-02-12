//! Ethereum Node types config.

pub use crate::payload::EthereumPayloadBuilder;
use crate::{EthEngineTypes, EthEvmConfig};
use reth_chainspec::ChainSpec;
use reth_consensus::{ConsensusError, FullConsensus};
use reth_ethereum_consensus::EthBeaconConsensus;
pub use reth_ethereum_engine_primitives::EthereumEngineValidator;
use reth_ethereum_engine_primitives::{
    EthBuiltPayload, EthPayloadAttributes, EthPayloadBuilderAttributes,
};
use reth_ethereum_primitives::{EthPrimitives, PooledTransaction};
use reth_evm::{execute::BasicBlockExecutorProvider, ConfigureEvm};
use reth_evm_ethereum::execute::EthExecutionStrategyFactory;
use reth_network::{EthNetworkPrimitives, NetworkHandle, PeersInfo};
use reth_node_api::{AddOnsContext, FullNodeComponents, NodeAddOns, TxTy};
use reth_node_builder::{
    components::{
        ComponentsBuilder, ConsensusBuilder, ExecutorBuilder, NetworkBuilder, PoolBuilder,
    },
    node::{FullNodeTypes, NodeTypes, NodeTypesWithEngine},
    rpc::{EngineValidatorAddOn, EngineValidatorBuilder, RethRpcAddOns, RpcAddOns, RpcHandle},
    BuilderContext, Node, NodeAdapter, NodeComponentsBuilder, PayloadTypes,
};
use reth_provider::{providers::ProviderFactoryBuilder, CanonStateSubscriptions, EthStorage};
use reth_rpc::{eth::core::EthApiFor, ValidationApi};
use reth_rpc_api::servers::BlockSubmissionValidationApiServer;
use reth_rpc_builder::config::RethRpcServerConfig;
use reth_rpc_eth_types::{error::FromEvmError, EthApiError};
use reth_rpc_server_types::RethRpcModule;
use reth_tracing::tracing::{debug, info};
use reth_transaction_pool::{
    blobstore::DiskFileBlobStore, EthTransactionPool, PoolTransaction, TransactionPool,
    TransactionValidationTaskExecutor,
};
use reth_trie_db::MerklePatriciaTrie;
use revm::primitives::TxEnv;
use std::sync::Arc;

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

    /// Instantiates the [`ProviderFactoryBuilder`] for an ethereum node.
    ///
    /// # Open a Providerfactory in read-only mode from a datadir
    ///
    /// See also: [`ProviderFactoryBuilder`] and
    /// [`ReadOnlyConfig`](reth_provider::providers::ReadOnlyConfig).
    ///
    /// ```no_run
    /// use reth_chainspec::MAINNET;
    /// use reth_node_ethereum::EthereumNode;
    ///
    /// let factory = EthereumNode::provider_factory_builder()
    ///     .open_read_only(MAINNET.clone(), "datadir")
    ///     .unwrap();
    /// ```
    ///
    /// # Open a Providerfactory manually with with all required components
    ///
    /// ```no_run
    /// use reth_chainspec::ChainSpecBuilder;
    /// use reth_db::open_db_read_only;
    /// use reth_node_ethereum::EthereumNode;
    /// use reth_provider::providers::StaticFileProvider;
    /// use std::sync::Arc;
    ///
    /// let factory = EthereumNode::provider_factory_builder()
    ///     .db(Arc::new(open_db_read_only("db", Default::default()).unwrap()))
    ///     .chainspec(ChainSpecBuilder::mainnet().build().into())
    ///     .static_file(StaticFileProvider::read_only("db/static_files", false).unwrap())
    ///     .build_provider_factory();
    /// ```
    pub fn provider_factory_builder() -> ProviderFactoryBuilder<Self> {
        ProviderFactoryBuilder::default()
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
#[derive(Debug)]
pub struct EthereumAddOns<N: FullNodeComponents> {
    inner: RpcAddOns<N, EthApiFor<N>, EthereumEngineValidatorBuilder>,
}

impl<N: FullNodeComponents> Default for EthereumAddOns<N> {
    fn default() -> Self {
        Self { inner: Default::default() }
    }
}

impl<N> NodeAddOns<N> for EthereumAddOns<N>
where
    N: FullNodeComponents<
        Types: NodeTypesWithEngine<
            ChainSpec = ChainSpec,
            Primitives = EthPrimitives,
            Engine = EthEngineTypes,
        >,
        Evm: ConfigureEvm<TxEnv = TxEnv>,
    >,
    EthApiError: FromEvmError<N::Evm>,
{
    type Handle = RpcHandle<N, EthApiFor<N>>;

    async fn launch_add_ons(
        self,
        ctx: reth_node_api::AddOnsContext<'_, N>,
    ) -> eyre::Result<Self::Handle> {
        let validation_api = ValidationApi::new(
            ctx.node.provider().clone(),
            Arc::new(ctx.node.consensus().clone()),
            ctx.node.block_executor().clone(),
            ctx.config.rpc.flashbots_config(),
            Box::new(ctx.node.task_executor().clone()),
            Arc::new(EthereumEngineValidator::new(ctx.config.chain.clone())),
        );

        self.inner
            .launch_add_ons_with(ctx, move |modules, _| {
                modules.merge_if_module_configured(
                    RethRpcModule::Flashbots,
                    validation_api.into_rpc(),
                )?;

                Ok(())
            })
            .await
    }
}

impl<N> RethRpcAddOns<N> for EthereumAddOns<N>
where
    N: FullNodeComponents<
        Types: NodeTypesWithEngine<
            ChainSpec = ChainSpec,
            Primitives = EthPrimitives,
            Engine = EthEngineTypes,
        >,
        Evm: ConfigureEvm<TxEnv = TxEnv>,
    >,
    EthApiError: FromEvmError<N::Evm>,
{
    type EthApi = EthApiFor<N>;

    fn hooks_mut(&mut self) -> &mut reth_node_builder::rpc::RpcHooks<N, Self::EthApi> {
        self.inner.hooks_mut()
    }
}

impl<N> EngineValidatorAddOn<N> for EthereumAddOns<N>
where
    N: FullNodeComponents<
        Types: NodeTypesWithEngine<
            ChainSpec = ChainSpec,
            Primitives = EthPrimitives,
            Engine = EthEngineTypes,
        >,
    >,
{
    type Validator = EthereumEngineValidator;

    async fn engine_validator(&self, ctx: &AddOnsContext<'_, N>) -> eyre::Result<Self::Validator> {
        EthereumEngineValidatorBuilder::default().build(ctx).await
    }
}

impl<N> Node<N> for EthereumNode
where
    N: FullNodeTypes<Types = Self>,
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
        let validator = TransactionValidationTaskExecutor::eth_builder(ctx.provider().clone())
            .with_head_timestamp(ctx.head().timestamp)
            .kzg_settings(ctx.kzg_settings()?)
            .with_local_transactions_config(pool_config.local_transactions_config.clone())
            .with_additional_tasks(ctx.config().txpool.additional_validation_tasks)
            .build_with_tasks(ctx.task_executor().clone(), blob_store.clone());

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
#[derive(Debug, Default, Clone, Copy)]
pub struct EthereumNetworkBuilder {
    // TODO add closure to modify network
}

impl<Node, Pool> NetworkBuilder<Node, Pool> for EthereumNetworkBuilder
where
    Node: FullNodeTypes<Types: NodeTypes<ChainSpec = ChainSpec, Primitives = EthPrimitives>>,
    Pool: TransactionPool<
            Transaction: PoolTransaction<Consensus = TxTy<Node::Types>, Pooled = PooledTransaction>,
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
    type Consensus = Arc<dyn FullConsensus<EthPrimitives, Error = ConsensusError>>;

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
