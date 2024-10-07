//! Optimism Node types config.

use std::sync::Arc;

use reth_basic_payload_builder::{BasicPayloadJobGenerator, BasicPayloadJobGeneratorConfig};
use reth_chainspec::{EthChainSpec, Hardforks};
use reth_evm::ConfigureEvm;
use reth_network::{NetworkConfig, NetworkHandle, NetworkManager};
use reth_node_api::{EngineValidator, FullNodeComponents, NodeAddOns};
use reth_node_builder::{
    components::{
        ComponentsBuilder, ConsensusBuilder, EngineValidatorBuilder, ExecutorBuilder,
        NetworkBuilder, PayloadServiceBuilder, PoolBuilder, PoolBuilderConfigOverrides,
    },
    node::{FullNodeTypes, NodeTypes, NodeTypesWithEngine},
    BuilderContext, Node, PayloadBuilderConfig,
};
use reth_optimism_chainspec::OpChainSpec;
use reth_optimism_consensus::OptimismBeaconConsensus;
use reth_optimism_evm::{OpExecutorProvider, OptimismEvmConfig};
use reth_optimism_rpc::OpEthApi;
use reth_payload_builder::{PayloadBuilderHandle, PayloadBuilderService};
use reth_primitives::Header;
use reth_provider::CanonStateSubscriptions;
use reth_tracing::tracing::{debug, info};
use reth_transaction_pool::{
    blobstore::DiskFileBlobStore, CoinbaseTipOrdering, TransactionPool,
    TransactionValidationTaskExecutor,
};

use crate::{
    args::RollupArgs,
    engine::OptimismEngineValidator,
    txpool::{OpTransactionPool, OpTransactionValidator},
    OptimismEngineTypes,
};

/// Type configuration for a regular Optimism node.
#[derive(Debug, Default, Clone)]
#[non_exhaustive]
pub struct OptimismNode {
    /// Additional Optimism args
    pub args: RollupArgs,
}

impl OptimismNode {
    /// Creates a new instance of the Optimism node type.
    pub const fn new(args: RollupArgs) -> Self {
        Self { args }
    }

    /// Returns the components for the given [`RollupArgs`].
    pub fn components<Node>(
        args: RollupArgs,
    ) -> ComponentsBuilder<
        Node,
        OptimismPoolBuilder,
        OptimismPayloadBuilder,
        OptimismNetworkBuilder,
        OptimismExecutorBuilder,
        OptimismConsensusBuilder,
        OptimismEngineValidatorBuilder,
    >
    where
        Node: FullNodeTypes<
            Types: NodeTypesWithEngine<Engine = OptimismEngineTypes, ChainSpec = OpChainSpec>,
        >,
    {
        let RollupArgs { disable_txpool_gossip, compute_pending_block, discovery_v4, .. } = args;
        ComponentsBuilder::default()
            .node_types::<Node>()
            .pool(OptimismPoolBuilder::default())
            .payload(OptimismPayloadBuilder::new(compute_pending_block))
            .network(OptimismNetworkBuilder {
                disable_txpool_gossip,
                disable_discovery_v4: !discovery_v4,
            })
            .executor(OptimismExecutorBuilder::default())
            .consensus(OptimismConsensusBuilder::default())
            .engine_validator(OptimismEngineValidatorBuilder::default())
    }
}

impl<N> Node<N> for OptimismNode
where
    N: FullNodeTypes<
        Types: NodeTypesWithEngine<Engine = OptimismEngineTypes, ChainSpec = OpChainSpec>,
    >,
{
    type ComponentsBuilder = ComponentsBuilder<
        N,
        OptimismPoolBuilder,
        OptimismPayloadBuilder,
        OptimismNetworkBuilder,
        OptimismExecutorBuilder,
        OptimismConsensusBuilder,
        OptimismEngineValidatorBuilder,
    >;

    type AddOns = OptimismAddOns;

    fn components_builder(&self) -> Self::ComponentsBuilder {
        let Self { args } = self;
        Self::components(args.clone())
    }

    fn add_ons(&self) -> Self::AddOns {
        OptimismAddOns::new(self.args.sequencer_http.clone())
    }
}

impl NodeTypes for OptimismNode {
    type Primitives = ();
    type ChainSpec = OpChainSpec;
}

impl NodeTypesWithEngine for OptimismNode {
    type Engine = OptimismEngineTypes;
}

/// Add-ons w.r.t. optimism.
#[derive(Debug, Clone)]
pub struct OptimismAddOns {
    sequencer_http: Option<String>,
}

impl OptimismAddOns {
    /// Create a new instance with the given `sequencer_http` URL.
    pub const fn new(sequencer_http: Option<String>) -> Self {
        Self { sequencer_http }
    }

    /// Returns the sequencer HTTP URL.
    pub fn sequencer_http(&self) -> Option<&str> {
        self.sequencer_http.as_deref()
    }
}

impl<N: FullNodeComponents> NodeAddOns<N> for OptimismAddOns {
    type EthApi = OpEthApi<N>;
}

/// A regular optimism evm and executor builder.
#[derive(Debug, Default, Clone, Copy)]
#[non_exhaustive]
pub struct OptimismExecutorBuilder;

impl<Node> ExecutorBuilder<Node> for OptimismExecutorBuilder
where
    Node: FullNodeTypes<Types: NodeTypes<ChainSpec = OpChainSpec>>,
{
    type EVM = OptimismEvmConfig;
    type Executor = OpExecutorProvider<Self::EVM>;

    async fn build_evm(
        self,
        ctx: &BuilderContext<Node>,
    ) -> eyre::Result<(Self::EVM, Self::Executor)> {
        let evm_config = OptimismEvmConfig::new(ctx.chain_spec());
        let executor = OpExecutorProvider::new(ctx.chain_spec(), evm_config.clone());

        Ok((evm_config, executor))
    }
}

/// A basic optimism transaction pool.
///
/// This contains various settings that can be configured and take precedence over the node's
/// config.
#[derive(Debug, Default, Clone)]
pub struct OptimismPoolBuilder {
    /// Enforced overrides that are applied to the pool config.
    pub pool_config_overrides: PoolBuilderConfigOverrides,
}

impl<Node> PoolBuilder<Node> for OptimismPoolBuilder
where
    Node: FullNodeTypes<Types: NodeTypes<ChainSpec = OpChainSpec>>,
{
    type Pool = OpTransactionPool<Node::Provider, DiskFileBlobStore>;

    async fn build_pool(self, ctx: &BuilderContext<Node>) -> eyre::Result<Self::Pool> {
        let Self { pool_config_overrides } = self;
        let data_dir = ctx.config().datadir();
        let blob_store = DiskFileBlobStore::open(data_dir.blobstore(), Default::default())?;

        let validator = TransactionValidationTaskExecutor::eth_builder(Arc::new(
            ctx.chain_spec().inner.clone(),
        ))
        .with_head_timestamp(ctx.head().timestamp)
        .kzg_settings(ctx.kzg_settings()?)
        .with_additional_tasks(ctx.config().txpool.additional_validation_tasks)
        .build_with_tasks(ctx.provider().clone(), ctx.task_executor().clone(), blob_store.clone())
        .map(|validator| {
            OpTransactionValidator::new(validator)
                // In --dev mode we can't require gas fees because we're unable to decode
                // the L1 block info
                .require_l1_data_gas_fee(!ctx.config().dev.dev)
        });

        let transaction_pool = reth_transaction_pool::Pool::new(
            validator,
            CoinbaseTipOrdering::default(),
            blob_store,
            pool_config_overrides.apply(ctx.pool_config()),
        );
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

/// A basic optimism payload service builder
#[derive(Debug, Default, Clone)]
pub struct OptimismPayloadBuilder {
    /// By default the pending block equals the latest block
    /// to save resources and not leak txs from the tx-pool,
    /// this flag enables computing of the pending block
    /// from the tx-pool instead.
    ///
    /// If `compute_pending_block` is not enabled, the payload builder
    /// will use the payload attributes from the latest block. Note
    /// that this flag is not yet functional.
    pub compute_pending_block: bool,
}

impl OptimismPayloadBuilder {
    /// Create a new instance with the given `compute_pending_block` flag.
    pub const fn new(compute_pending_block: bool) -> Self {
        Self { compute_pending_block }
    }

    /// A helper method to initialize [`PayloadBuilderService`] with the given EVM config.
    pub fn spawn<Node, Evm, Pool>(
        self,
        evm_config: Evm,
        ctx: &BuilderContext<Node>,
        pool: Pool,
    ) -> eyre::Result<PayloadBuilderHandle<OptimismEngineTypes>>
    where
        Node: FullNodeTypes<
            Types: NodeTypesWithEngine<Engine = OptimismEngineTypes, ChainSpec = OpChainSpec>,
        >,
        Pool: TransactionPool + Unpin + 'static,
        Evm: ConfigureEvm<Header = Header>,
    {
        let payload_builder =
            reth_optimism_payload_builder::OptimismPayloadBuilder::new(evm_config)
                .set_compute_pending_block(self.compute_pending_block);
        let conf = ctx.payload_builder_config();

        let payload_job_config = BasicPayloadJobGeneratorConfig::default()
            .interval(conf.interval())
            .deadline(conf.deadline())
            .max_payload_tasks(conf.max_payload_tasks())
            // no extradata for OP
            .extradata(Default::default());

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

impl<Node, Pool> PayloadServiceBuilder<Node, Pool> for OptimismPayloadBuilder
where
    Node: FullNodeTypes<
        Types: NodeTypesWithEngine<Engine = OptimismEngineTypes, ChainSpec = OpChainSpec>,
    >,
    Pool: TransactionPool + Unpin + 'static,
{
    async fn spawn_payload_service(
        self,
        ctx: &BuilderContext<Node>,
        pool: Pool,
    ) -> eyre::Result<PayloadBuilderHandle<OptimismEngineTypes>> {
        self.spawn(OptimismEvmConfig::new(ctx.chain_spec()), ctx, pool)
    }
}

/// A basic optimism network builder.
#[derive(Debug, Default, Clone)]
pub struct OptimismNetworkBuilder {
    /// Disable transaction pool gossip
    pub disable_txpool_gossip: bool,
    /// Disable discovery v4
    pub disable_discovery_v4: bool,
}

impl OptimismNetworkBuilder {
    /// Returns the [`NetworkConfig`] that contains the settings to launch the p2p network.
    ///
    /// This applies the configured [`OptimismNetworkBuilder`] settings.
    pub fn network_config<Node>(
        &self,
        ctx: &BuilderContext<Node>,
    ) -> eyre::Result<NetworkConfig<<Node as FullNodeTypes>::Provider>>
    where
        Node: FullNodeTypes<Types: NodeTypes<ChainSpec: Hardforks>>,
    {
        let Self { disable_txpool_gossip, disable_discovery_v4 } = self.clone();
        let args = &ctx.config().network;
        let network_builder = ctx
            .network_config_builder()?
            // apply discovery settings
            .apply(|mut builder| {
                let rlpx_socket = (args.addr, args.port).into();
                if disable_discovery_v4 || args.discovery.disable_discovery {
                    builder = builder.disable_discv4_discovery();
                }
                if !args.discovery.disable_discovery {
                    builder = builder.discovery_v5(
                        args.discovery.discovery_v5_builder(
                            rlpx_socket,
                            ctx.config()
                                .network
                                .resolved_bootnodes()
                                .or_else(|| ctx.chain_spec().bootnodes())
                                .unwrap_or_default(),
                        ),
                    );
                }

                builder
            });

        let mut network_config = ctx.build_network_config(network_builder);

        // When `sequencer_endpoint` is configured, the node will forward all transactions to a
        // Sequencer node for execution and inclusion on L1, and disable its own txpool
        // gossip to prevent other parties in the network from learning about them.
        network_config.tx_gossip_disabled = disable_txpool_gossip;

        Ok(network_config)
    }
}

impl<Node, Pool> NetworkBuilder<Node, Pool> for OptimismNetworkBuilder
where
    Node: FullNodeTypes<Types: NodeTypes<ChainSpec = OpChainSpec>>,
    Pool: TransactionPool + Unpin + 'static,
{
    async fn build_network(
        self,
        ctx: &BuilderContext<Node>,
        pool: Pool,
    ) -> eyre::Result<NetworkHandle> {
        let network_config = self.network_config(ctx)?;
        let network = NetworkManager::builder(network_config).await?;
        let handle = ctx.start_network(network, pool);

        Ok(handle)
    }
}

/// A basic optimism consensus builder.
#[derive(Debug, Default, Clone)]
#[non_exhaustive]
pub struct OptimismConsensusBuilder;

impl<Node> ConsensusBuilder<Node> for OptimismConsensusBuilder
where
    Node: FullNodeTypes<Types: NodeTypes<ChainSpec = OpChainSpec>>,
{
    type Consensus = Arc<dyn reth_consensus::Consensus>;

    async fn build_consensus(self, ctx: &BuilderContext<Node>) -> eyre::Result<Self::Consensus> {
        if ctx.is_dev() {
            Ok(Arc::new(reth_auto_seal_consensus::AutoSealConsensus::new(ctx.chain_spec())))
        } else {
            Ok(Arc::new(OptimismBeaconConsensus::new(ctx.chain_spec())))
        }
    }
}

/// Builder for [`OptimismEngineValidator`].
#[derive(Debug, Default, Clone)]
#[non_exhaustive]
pub struct OptimismEngineValidatorBuilder;

impl<Node, Types> EngineValidatorBuilder<Node> for OptimismEngineValidatorBuilder
where
    Types: NodeTypesWithEngine<ChainSpec = OpChainSpec>,
    Node: FullNodeTypes<Types = Types>,
    OptimismEngineValidator: EngineValidator<Types::Engine>,
{
    type Validator = OptimismEngineValidator;

    async fn build_validator(self, ctx: &BuilderContext<Node>) -> eyre::Result<Self::Validator> {
        Ok(OptimismEngineValidator::new(ctx.chain_spec()))
    }
}
