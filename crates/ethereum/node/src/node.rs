//! Ethereum Node types config.

use crate::{EthEngineTypes, EthEvmConfig};
use alloy_eips::{eip7840::BlobParams, merge::EPOCH_SLOTS};
use alloy_network::Ethereum;
use alloy_rpc_types_engine::ExecutionData;
use reth_chainspec::{ChainSpec, EthChainSpec, EthereumHardforks, Hardforks};
use reth_engine_local::LocalPayloadAttributesBuilder;
use reth_engine_primitives::EngineTypes;
use reth_ethereum_consensus::EthBeaconConsensus;
use reth_ethereum_engine_primitives::{EthBuiltPayload, EthPayloadAttributes};
use reth_ethereum_primitives::{EthPrimitives, TransactionSigned};
use reth_evm::{
    eth::spec::EthExecutorSpec, ConfigureEvm, EvmFactory, EvmFactoryFor, NextBlockEnvAttributes,
};
use reth_evm_ethereum::factory::RethEvmFactory;
#[cfg(feature = "jit")]
use reth_evm_ethereum::factory::{JitBackend, JitMode, RevmcMetrics, RuntimeConfig, RuntimeTuning};
use reth_network::{primitives::BasicNetworkPrimitives, NetworkHandle, PeersInfo};
use reth_node_api::{
    AddOnsContext, FullNodeComponents, HeaderTy, NodeAddOns, NodePrimitives,
    PayloadAttributesBuilder, PrimitivesTy, TxTy,
};
use reth_node_builder::{
    components::{
        BasicPayloadServiceBuilder, ComponentsBuilder, ConsensusBuilder, ExecutorBuilder,
        NetworkBuilder, PoolBuilder, TxPoolBuilder,
    },
    node::{FullNodeTypes, NodeTypes},
    rpc::{
        BasicEngineApiBuilder, BasicEngineValidatorBuilder, Either, EngineApiBuilder,
        EngineValidatorAddOn, EngineValidatorBuilder, EthApiBuilder, EthApiCtx, Identity,
        PayloadValidatorBuilder, RethAuthHttpMiddleware, RethRpcAddOns, RethRpcMiddleware,
        RpcAddOns, RpcHandle, Stack,
    },
    BuilderContext, DebugNode, Node, NodeAdapter, PayloadBuilderConfig,
};
use reth_node_core::args::JitArgs;
use reth_payload_primitives::PayloadTypes;
use reth_provider::{providers::ProviderFactoryBuilder, EthStorage};
use reth_rpc::{
    eth::core::{EthApiFor, EthRpcConverterFor},
    TestingApi, ValidationApi,
};
use reth_rpc_api::servers::{BlockSubmissionValidationApiServer, TestingApiServer};
use reth_rpc_builder::config::RethRpcServerConfig;
use reth_rpc_eth_api::{
    helpers::{
        config::{EthConfigApiServer, EthConfigHandler},
        pending_block::BuildPendingEnv,
    },
    RpcConvert, RpcTypes, SignableTxRequest,
};
use reth_rpc_eth_types::{error::FromEvmError, EthApiError};
use reth_rpc_server_types::RethRpcModule;
use reth_tracing::tracing::{debug, info};
use reth_transaction_pool::{
    blobstore::DiskFileBlobStore, EthTransactionPool, PoolPooledTx, PoolTransaction,
    TransactionPool, TransactionValidationTaskExecutor,
};
use revm::context::TxEnv;
use std::{marker::PhantomData, sync::Arc, time::SystemTime};

pub use crate::{payload::EthereumPayloadBuilder, EthereumEngineValidator};
#[cfg(feature = "jit")]
pub use reth_evm_ethereum::factory::maybe_run_jit_helper;

/// Type configuration for a regular Ethereum node.
#[derive(Debug, Default, Clone, Copy)]
#[non_exhaustive]
pub struct EthereumNode;

impl EthereumNode {
    /// Returns a [`ComponentsBuilder`] configured for a regular Ethereum node.
    pub fn components<Node>() -> ComponentsBuilder<
        Node,
        EthereumPoolBuilder,
        BasicPayloadServiceBuilder<EthereumPayloadBuilder>,
        EthereumNetworkBuilder,
        EthereumExecutorBuilder,
        EthereumConsensusBuilder,
    >
    where
        Node: FullNodeTypes<
            Types: NodeTypes<
                ChainSpec: Hardforks + EthereumHardforks + EthExecutorSpec,
                Primitives = EthPrimitives,
            >,
        >,
        <Node::Types as NodeTypes>::Payload:
            PayloadTypes<BuiltPayload = EthBuiltPayload, PayloadAttributes = EthPayloadAttributes>,
    {
        ComponentsBuilder::default()
            .node_types::<Node>()
            .pool(EthereumPoolBuilder::default())
            .executor(EthereumExecutorBuilder::default())
            .payload(BasicPayloadServiceBuilder::default())
            .network(EthereumNetworkBuilder::default())
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
    /// fn demo(runtime: reth_tasks::Runtime) {
    ///     let factory = EthereumNode::provider_factory_builder()
    ///         .open_read_only(MAINNET.clone(), "datadir", runtime)
    ///         .unwrap();
    /// }
    /// ```
    ///
    /// See also [`ProviderFactory::new`](reth_provider::ProviderFactory::new) for constructing
    /// a [`ProviderFactory`](reth_provider::ProviderFactory) manually with all required
    /// components.
    pub fn provider_factory_builder() -> ProviderFactoryBuilder<Self> {
        ProviderFactoryBuilder::default()
    }
}

impl NodeTypes for EthereumNode {
    type Primitives = EthPrimitives;
    type ChainSpec = ChainSpec;
    type Storage = EthStorage;
    type Payload = EthEngineTypes;
}

/// Builds [`EthApi`](reth_rpc::EthApi) for Ethereum.
#[derive(Debug)]
pub struct EthereumEthApiBuilder<NetworkT = Ethereum>(PhantomData<NetworkT>);

impl<NetworkT> Default for EthereumEthApiBuilder<NetworkT> {
    fn default() -> Self {
        Self(Default::default())
    }
}

impl<N, NetworkT> EthApiBuilder<N> for EthereumEthApiBuilder<NetworkT>
where
    N: FullNodeComponents<
        Types: NodeTypes<ChainSpec: Hardforks + EthereumHardforks>,
        Evm: ConfigureEvm<NextBlockEnvCtx: BuildPendingEnv<HeaderTy<N::Types>>>,
    >,
    NetworkT: RpcTypes<TransactionRequest: SignableTxRequest<TxTy<N::Types>>>,
    EthRpcConverterFor<N, NetworkT>: RpcConvert<
        Primitives = PrimitivesTy<N::Types>,
        Error = EthApiError,
        Network = NetworkT,
        Evm = N::Evm,
    >,
    EthApiError: FromEvmError<N::Evm>,
{
    type EthApi = EthApiFor<N, NetworkT>;

    async fn build_eth_api(self, ctx: EthApiCtx<'_, N>) -> eyre::Result<Self::EthApi> {
        Ok(ctx.eth_api_builder().map_converter(|r| r.with_network()).build())
    }
}

/// Add-ons w.r.t. l1 ethereum.
#[derive(Debug)]
pub struct EthereumAddOns<
    N: FullNodeComponents,
    EthB: EthApiBuilder<N>,
    PVB,
    EB = BasicEngineApiBuilder<PVB>,
    EVB = BasicEngineValidatorBuilder<PVB>,
    RpcMiddleware = Identity,
    AuthHttpMiddleware = Identity,
> {
    inner: RpcAddOns<N, EthB, PVB, EB, EVB, RpcMiddleware, AuthHttpMiddleware>,
}

impl<N, EthB, PVB, EB, EVB, RpcMiddleware, AuthHttpMiddleware>
    EthereumAddOns<N, EthB, PVB, EB, EVB, RpcMiddleware, AuthHttpMiddleware>
where
    N: FullNodeComponents,
    EthB: EthApiBuilder<N>,
{
    /// Creates a new instance from the inner `RpcAddOns`.
    pub const fn new(
        inner: RpcAddOns<N, EthB, PVB, EB, EVB, RpcMiddleware, AuthHttpMiddleware>,
    ) -> Self {
        Self { inner }
    }
}

impl<N> Default for EthereumAddOns<N, EthereumEthApiBuilder, EthereumEngineValidatorBuilder>
where
    N: FullNodeComponents<
        Types: NodeTypes<
            ChainSpec: EthereumHardforks + Clone + 'static,
            Payload: EngineTypes<ExecutionData = ExecutionData>
                         + PayloadTypes<PayloadAttributes = EthPayloadAttributes>,
            Primitives = EthPrimitives,
        >,
    >,
    EthereumEthApiBuilder: EthApiBuilder<N>,
{
    fn default() -> Self {
        Self::new(RpcAddOns::new(
            EthereumEthApiBuilder::default(),
            EthereumEngineValidatorBuilder::default(),
            BasicEngineApiBuilder::default(),
            BasicEngineValidatorBuilder::default(),
            Default::default(),
            Identity::new(),
        ))
    }
}

impl<N, EthB, PVB, EB, EVB, RpcMiddleware, AuthHttpMiddleware>
    EthereumAddOns<N, EthB, PVB, EB, EVB, RpcMiddleware, AuthHttpMiddleware>
where
    N: FullNodeComponents,
    EthB: EthApiBuilder<N>,
{
    /// Replace the engine API builder.
    pub fn with_engine_api<T>(
        self,
        engine_api_builder: T,
    ) -> EthereumAddOns<N, EthB, PVB, T, EVB, RpcMiddleware, AuthHttpMiddleware>
    where
        T: Send,
    {
        let Self { inner } = self;
        EthereumAddOns::new(inner.with_engine_api(engine_api_builder))
    }

    /// Replace the payload validator builder.
    pub fn with_payload_validator<V, T>(
        self,
        payload_validator_builder: T,
    ) -> EthereumAddOns<N, EthB, T, EB, EVB, RpcMiddleware, AuthHttpMiddleware> {
        let Self { inner } = self;
        EthereumAddOns::new(inner.with_payload_validator(payload_validator_builder))
    }

    /// Sets rpc middleware
    pub fn with_rpc_middleware<T>(
        self,
        rpc_middleware: T,
    ) -> EthereumAddOns<N, EthB, PVB, EB, EVB, T, AuthHttpMiddleware>
    where
        T: Send,
    {
        let Self { inner } = self;
        EthereumAddOns::new(inner.with_rpc_middleware(rpc_middleware))
    }

    /// Configures the HTTP transport middleware for the auth / Engine API server.
    pub fn with_auth_http_middleware<T>(
        self,
        auth_http_middleware: T,
    ) -> EthereumAddOns<N, EthB, PVB, EB, EVB, RpcMiddleware, T>
    where
        T: Send,
    {
        let Self { inner } = self;
        EthereumAddOns::new(inner.with_auth_http_middleware(auth_http_middleware))
    }

    /// Stacks an additional HTTP transport middleware layer for the auth / Engine API server.
    pub fn layer_auth_http_middleware<T>(
        self,
        layer: T,
    ) -> EthereumAddOns<N, EthB, PVB, EB, EVB, RpcMiddleware, Stack<AuthHttpMiddleware, T>> {
        let Self { inner } = self;
        EthereumAddOns::new(inner.layer_auth_http_middleware(layer))
    }

    /// Conditionally stacks an HTTP transport middleware layer for the auth / Engine API server.
    #[expect(clippy::type_complexity)]
    pub fn option_layer_auth_http_middleware<T>(
        self,
        layer: Option<T>,
    ) -> EthereumAddOns<
        N,
        EthB,
        PVB,
        EB,
        EVB,
        RpcMiddleware,
        Stack<AuthHttpMiddleware, Either<T, Identity>>,
    > {
        let Self { inner } = self;
        EthereumAddOns::new(inner.option_layer_auth_http_middleware(layer))
    }

    /// Sets the tokio runtime for the RPC servers.
    ///
    /// Caution: This runtime must not be created from within asynchronous context.
    pub fn with_tokio_runtime(self, tokio_runtime: Option<tokio::runtime::Handle>) -> Self {
        let Self { inner } = self;
        Self { inner: inner.with_tokio_runtime(tokio_runtime) }
    }
}

impl<N, EthB, PVB, EB, EVB, RpcMiddleware, AuthHttpMiddleware> NodeAddOns<N>
    for EthereumAddOns<N, EthB, PVB, EB, EVB, RpcMiddleware, AuthHttpMiddleware>
where
    N: FullNodeComponents<
        Types: NodeTypes<
            ChainSpec: EthChainSpec + Hardforks + EthereumHardforks,
            Primitives = EthPrimitives,
            Payload: EngineTypes<ExecutionData = ExecutionData>,
        >,
        Evm: ConfigureEvm<NextBlockEnvCtx = NextBlockEnvAttributes>,
    >,
    EthB: EthApiBuilder<N>,
    PVB: Send,
    EB: EngineApiBuilder<N>,
    EVB: EngineValidatorBuilder<N>,
    EthApiError: FromEvmError<N::Evm>,
    EvmFactoryFor<N::Evm>: EvmFactory<Tx = TxEnv>,
    RpcMiddleware: RethRpcMiddleware,
    AuthHttpMiddleware: RethAuthHttpMiddleware<Identity>,
{
    type Handle = RpcHandle<N, EthB::EthApi>;

    async fn launch_add_ons(
        self,
        ctx: reth_node_api::AddOnsContext<'_, N>,
    ) -> eyre::Result<Self::Handle> {
        let validation_api = ValidationApi::<_, _, <N::Types as NodeTypes>::Payload>::new(
            ctx.node.provider().clone(),
            Arc::new(ctx.node.consensus().clone()),
            ctx.node.evm_config().clone(),
            ctx.config.rpc.flashbots_config(),
            ctx.node.task_executor().clone(),
            Arc::new(EthereumEngineValidator::new(ctx.config.chain.clone())),
        );

        let eth_config =
            EthConfigHandler::new(ctx.node.provider().clone(), ctx.node.evm_config().clone());

        let testing_skip_invalid_transactions = ctx.config.rpc.testing_skip_invalid_transactions;
        let testing_gas_limit_override = ctx.config.rpc.testing_gas_limit;
        let testing_desired_gas_limit = ctx.config.builder.gas_limit_for(ctx.config.chain.chain());
        let testing_engine_handle = ctx.beacon_engine_handle.clone();

        self.inner
            .launch_add_ons_with(ctx, move |container| {
                container.modules.merge_if_module_configured(
                    RethRpcModule::Flashbots,
                    validation_api.into_rpc(),
                )?;

                container
                    .modules
                    .merge_if_module_configured(RethRpcModule::Eth, eth_config.into_rpc())?;

                // testing_buildBlockV1: only wire when the hidden testing module is explicitly
                // requested on any transport. Default stays disabled to honor security guidance.
                let mut testing_api = TestingApi::new(
                    container.registry.eth_api().clone(),
                    container.registry.evm_config().clone(),
                    testing_desired_gas_limit,
                    testing_engine_handle,
                );
                if testing_skip_invalid_transactions {
                    testing_api = testing_api.with_skip_invalid_transactions();
                }
                if let Some(gas_limit) = testing_gas_limit_override {
                    testing_api = testing_api.with_gas_limit_override(gas_limit);
                }
                container
                    .modules
                    .merge_if_module_configured(RethRpcModule::Testing, testing_api.into_rpc())?;

                Ok(())
            })
            .await
    }
}

impl<N, EthB, PVB, EB, EVB, RpcMiddleware, AuthHttpMiddleware> RethRpcAddOns<N>
    for EthereumAddOns<N, EthB, PVB, EB, EVB, RpcMiddleware, AuthHttpMiddleware>
where
    N: FullNodeComponents<
        Types: NodeTypes<
            ChainSpec: Hardforks + EthereumHardforks,
            Primitives = EthPrimitives,
            Payload: EngineTypes<ExecutionData = ExecutionData>,
        >,
        Evm: ConfigureEvm<NextBlockEnvCtx = NextBlockEnvAttributes>,
    >,
    EthB: EthApiBuilder<N>,
    PVB: PayloadValidatorBuilder<N>,
    EB: EngineApiBuilder<N>,
    EVB: EngineValidatorBuilder<N>,
    EthApiError: FromEvmError<N::Evm>,
    EvmFactoryFor<N::Evm>: EvmFactory<Tx = TxEnv>,
    RpcMiddleware: RethRpcMiddleware,
    AuthHttpMiddleware: RethAuthHttpMiddleware<Identity>,
{
    type EthApi = EthB::EthApi;

    fn hooks_mut(&mut self) -> &mut reth_node_builder::rpc::RpcHooks<N, Self::EthApi> {
        self.inner.hooks_mut()
    }
}

impl<N, EthB, PVB, EB, EVB, RpcMiddleware, AuthHttpMiddleware> EngineValidatorAddOn<N>
    for EthereumAddOns<N, EthB, PVB, EB, EVB, RpcMiddleware, AuthHttpMiddleware>
where
    N: FullNodeComponents<
        Types: NodeTypes<
            ChainSpec: EthChainSpec + EthereumHardforks,
            Primitives = EthPrimitives,
            Payload: EngineTypes<ExecutionData = ExecutionData>,
        >,
        Evm: ConfigureEvm<NextBlockEnvCtx = NextBlockEnvAttributes>,
    >,
    EthB: EthApiBuilder<N>,
    PVB: Send,
    EB: EngineApiBuilder<N>,
    EVB: EngineValidatorBuilder<N>,
    EthApiError: FromEvmError<N::Evm>,
    EvmFactoryFor<N::Evm>: EvmFactory<Tx = TxEnv>,
    RpcMiddleware: Send,
    AuthHttpMiddleware: Send,
{
    type ValidatorBuilder = EVB;

    fn engine_validator_builder(&self) -> Self::ValidatorBuilder {
        self.inner.engine_validator_builder()
    }
}

impl<N> Node<N> for EthereumNode
where
    N: FullNodeTypes<Types = Self>,
{
    type ComponentsBuilder = ComponentsBuilder<
        N,
        EthereumPoolBuilder,
        BasicPayloadServiceBuilder<EthereumPayloadBuilder>,
        EthereumNetworkBuilder,
        EthereumExecutorBuilder,
        EthereumConsensusBuilder,
    >;

    type AddOns =
        EthereumAddOns<NodeAdapter<N>, EthereumEthApiBuilder, EthereumEngineValidatorBuilder>;

    fn components_builder(&self) -> Self::ComponentsBuilder {
        Self::components()
    }

    fn add_ons(&self) -> Self::AddOns {
        EthereumAddOns::default()
    }
}

impl<N: FullNodeComponents<Types = Self>> DebugNode<N> for EthereumNode {
    type RpcBlock = alloy_rpc_types_eth::Block;

    fn rpc_to_primitive_block(rpc_block: Self::RpcBlock) -> reth_ethereum_primitives::Block {
        rpc_block.into_consensus().convert_transactions()
    }

    fn local_payload_attributes_builder(
        chain_spec: &Self::ChainSpec,
    ) -> impl PayloadAttributesBuilder<<Self::Payload as PayloadTypes>::PayloadAttributes> {
        LocalPayloadAttributesBuilder::new(Arc::new(chain_spec.clone()))
    }
}

/// Builds a [`RuntimeConfig`] from CLI [`JitArgs`].
#[cfg(feature = "jit")]
fn jit_runtime_config(jit: &JitArgs) -> RuntimeConfig {
    let default_tuning = RuntimeTuning::default();
    let tuning = RuntimeTuning {
        channel_capacity: jit.channel_capacity,
        jit_hot_threshold: jit.hot_threshold,
        jit_max_bytecode_len: jit.max_bytecode_len,
        jit_max_pending_jobs: jit.max_pending_jobs,
        jit_worker_count: jit.worker_count.unwrap_or(default_tuning.jit_worker_count),
        jit_timeout: default_tuning.jit_timeout,
        jit_helper_memory_limit_bytes: default_tuning.jit_helper_memory_limit_bytes,
        jit_helper_cpu_count: default_tuning.jit_helper_cpu_count,
        resident_code_cache_bytes: jit.code_cache_bytes,
        idle_evict_duration: Some(jit.idle_evict_duration),

        max_events_per_drain: default_tuning.max_events_per_drain,
        event_drain_interval: default_tuning.event_drain_interval,
        shutdown_timeout: default_tuning.shutdown_timeout,
        jit_worker_queue_capacity: default_tuning.jit_worker_queue_capacity,
        jit_opt_level: default_tuning.jit_opt_level,
        aot_opt_level: default_tuning.aot_opt_level,
        eviction_sweep_interval: default_tuning.eviction_sweep_interval,
        compiler_recycle_threshold: default_tuning.compiler_recycle_threshold,
    };

    let default_config = RuntimeConfig::default();
    RuntimeConfig {
        enabled: jit.enabled,
        thread_name: default_config.thread_name,
        store: default_config.store,
        tuning,
        dump_dir: default_config.dump_dir,
        debug_assertions: jit.debug,
        blocking: jit.blocking,
        single_error: default_config.single_error,
        no_dedup: default_config.no_dedup,
        no_dse: default_config.no_dse,
        gas_params: default_config.gas_params,
        aot: default_config.aot,
        jit_mode: JitMode::OutOfProcess,
        jit_helper_path: default_config.jit_helper_path,
        on_compilation: default_config.on_compilation,
    }
}

/// Builds an [`EthEvmConfig`] with revmc JIT from CLI [`JitArgs`].
///
/// This is the shared setup used by both [`EthereumExecutorBuilder`] and `reth re-execute`.
///
/// Returns the evm config and metrics recorder if JIT starts enabled.
#[cfg(feature = "jit")]
#[allow(clippy::type_complexity)]
pub fn build_evm_config<C: EthereumHardforks>(
    chain_spec: Arc<C>,
    jit: &JitArgs,
    dump_dir: Option<std::path::PathBuf>,
) -> eyre::Result<(EthEvmConfig<C, RethEvmFactory>, Option<Arc<RevmcMetrics>>)> {
    if !jit.enabled {
        let factory = RethEvmFactory::disabled();
        return Ok((EthEvmConfig::new_with_evm_factory(chain_spec, factory), None));
    }

    let mut config = jit_runtime_config(jit);
    config.dump_dir = dump_dir;

    let revmc_metrics = Arc::new(RevmcMetrics::default());
    let compilation_metrics = revmc_metrics.clone();
    config.on_compilation = Some(Arc::new(move |event| {
        compilation_metrics.record_compilation(&event);
    }));

    let tuning = config.tuning;
    let jit_mode = config.jit_mode;
    let backend = JitBackend::new(config)?;

    reth_tracing::tracing::warn!(target: "reth::cli",
        hot_threshold = tuning.jit_hot_threshold,
        workers = tuning.jit_worker_count,
        mode = ?jit_mode,
        blocking = jit.blocking,
        "Started experimental revmc JIT backend; this may cause instability",
    );

    let factory = RethEvmFactory::new_with_metrics(backend, revmc_metrics.as_ref().clone());
    let evm_config = EthEvmConfig::new_with_evm_factory(chain_spec, factory);

    Ok((evm_config, Some(revmc_metrics)))
}

/// Builds an [`EthEvmConfig`] from CLI [`JitArgs`].
///
/// This is the shared setup used by both [`EthereumExecutorBuilder`] and `reth re-execute`.
///
/// Compiled without the `jit` feature: errors if JIT was requested via [`JitArgs`] and otherwise
/// returns a plain interpreter-backed config.
#[cfg(not(feature = "jit"))]
#[allow(clippy::type_complexity)]
pub fn build_evm_config<C: EthereumHardforks>(
    chain_spec: Arc<C>,
    jit: &JitArgs,
    _dump_dir: Option<std::path::PathBuf>,
) -> eyre::Result<(EthEvmConfig<C, RethEvmFactory>, Option<()>)> {
    if jit.enabled {
        eyre::bail!(
            "JIT compilation was requested but this binary was compiled without the `jit` feature"
        );
    }
    let factory = RethEvmFactory::default();
    Ok((EthEvmConfig::new_with_evm_factory(chain_spec, factory), None))
}

/// A regular ethereum evm and executor builder.
///
/// Uses [`RethEvmFactory`].
#[derive(Debug, Default, Clone, Copy)]
#[non_exhaustive]
pub struct EthereumExecutorBuilder;

impl<Types, Node> ExecutorBuilder<Node> for EthereumExecutorBuilder
where
    Types: NodeTypes<
        ChainSpec: Hardforks + EthExecutorSpec + EthereumHardforks,
        Primitives = EthPrimitives,
    >,
    Node: FullNodeTypes<Types = Types>,
{
    type EVM = EthEvmConfig<Types::ChainSpec, RethEvmFactory>;

    async fn build_evm(self, ctx: &BuilderContext<Node>) -> eyre::Result<Self::EVM> {
        let jit = &ctx.config().jit;
        let dump_dir = jit.debug.then(|| ctx.config().datadir().data_dir().join("jit"));

        let (evm_config, revmc_metrics) = build_evm_config(ctx.chain_spec(), jit, dump_dir)?;
        let evm_config = evm_config.with_sender_recovery_cache(ctx.sender_recovery_cache().clone());

        #[cfg(not(feature = "jit"))]
        let _ = revmc_metrics;

        #[cfg(feature = "jit")]
        if let Some(revmc_metrics) = revmc_metrics {
            let metrics_backend = evm_config.executor_factory.evm_factory().backend().clone();
            ctx.task_executor().spawn_with_graceful_shutdown_signal(|shutdown| async move {
                let mut shutdown = std::pin::pin!(shutdown);
                loop {
                    tokio::select! {
                        _ = tokio::time::sleep(std::time::Duration::from_secs(5)) => {
                            revmc_metrics.record(&metrics_backend.stats());
                        }
                        _ = &mut shutdown => break,
                    }
                }
            });
        }

        Ok(evm_config)
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

impl<Types, Node, Evm> PoolBuilder<Node, Evm> for EthereumPoolBuilder
where
    Types: NodeTypes<
        ChainSpec: EthereumHardforks,
        Primitives: NodePrimitives<SignedTx = TransactionSigned>,
    >,
    Node: FullNodeTypes<Types = Types>,
    Evm: ConfigureEvm<Primitives = PrimitivesTy<Types>> + Clone + 'static,
{
    type Pool = EthTransactionPool<Node::Provider, DiskFileBlobStore, Evm>;

    async fn build_pool(
        self,
        ctx: &BuilderContext<Node>,
        evm_config: Evm,
    ) -> eyre::Result<Self::Pool> {
        let pool_config = ctx.pool_config();

        let blobs_disabled = ctx.config().txpool.disable_blobs_support ||
            ctx.config().txpool.blobpool_max_count == 0;

        let blob_cache_size = if let Some(blob_cache_size) = pool_config.blob_cache_size {
            Some(blob_cache_size)
        } else {
            // get the current blob params for the current timestamp, fallback to default Cancun
            // params
            let current_timestamp =
                SystemTime::now().duration_since(SystemTime::UNIX_EPOCH)?.as_secs();
            let blob_params = ctx
                .chain_spec()
                .blob_params_at_timestamp(current_timestamp)
                .unwrap_or_else(BlobParams::cancun);

            // Derive the blob cache size from the target blob count, to auto scale it by
            // multiplying it with the slot count for 2 epochs: 384 for pectra
            Some((blob_params.target_blob_count * EPOCH_SLOTS * 2) as u32)
        };

        let blob_store =
            reth_node_builder::components::create_blob_store_with_cache(ctx, blob_cache_size)?;

        let validator =
            TransactionValidationTaskExecutor::eth_builder(ctx.provider().clone(), evm_config)
                .set_eip4844(!blobs_disabled)
                .kzg_settings(ctx.kzg_settings()?)
                .with_max_tx_input_bytes(ctx.config().txpool.max_tx_input_bytes)
                .with_local_transactions_config(pool_config.local_transactions_config.clone())
                .set_tx_fee_cap(ctx.config().rpc.rpc_tx_fee_cap)
                .with_max_tx_gas_limit(ctx.config().txpool.max_tx_gas_limit)
                .with_minimum_priority_fee(ctx.config().txpool.minimum_priority_fee)
                .with_additional_tasks(ctx.config().txpool.additional_validation_tasks)
                .build_with_tasks(ctx.task_executor().clone(), blob_store.clone());

        if validator.validator().eip4844() {
            // initializing the KZG settings can be expensive, this should be done upfront so that
            // it doesn't impact the first block or the first gossiped blob transaction, so we
            // initialize this in the background
            let kzg_settings = validator.validator().kzg_settings().clone();
            ctx.task_executor().spawn_blocking_task(async move {
                let _ = kzg_settings.get();
                debug!(target: "reth::cli", "Initialized KZG settings");
            });
        }

        let transaction_pool = TxPoolBuilder::new(ctx)
            .with_validator(validator)
            .build_and_spawn_maintenance_task(blob_store, pool_config)?;

        info!(target: "reth::cli", "Transaction pool initialized");
        debug!(target: "reth::cli", "Spawned txpool maintenance task");

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
    Node: FullNodeTypes<Types: NodeTypes<ChainSpec: Hardforks>>,
    Pool: TransactionPool<Transaction: PoolTransaction<Consensus = TxTy<Node::Types>>>
        + Unpin
        + 'static,
{
    type Network =
        NetworkHandle<BasicNetworkPrimitives<PrimitivesTy<Node::Types>, PoolPooledTx<Pool>>>;

    async fn build_network(
        self,
        ctx: &BuilderContext<Node>,
        pool: Pool,
    ) -> eyre::Result<Self::Network> {
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
    Node: FullNodeTypes<
        Types: NodeTypes<ChainSpec: EthChainSpec + EthereumHardforks, Primitives = EthPrimitives>,
    >,
{
    type Consensus = Arc<EthBeaconConsensus<<Node::Types as NodeTypes>::ChainSpec>>;

    async fn build_consensus(self, ctx: &BuilderContext<Node>) -> eyre::Result<Self::Consensus> {
        Ok(Arc::new(EthBeaconConsensus::new(ctx.chain_spec())))
    }
}

/// Builder for [`EthereumEngineValidator`].
#[derive(Debug, Default, Clone)]
#[non_exhaustive]
pub struct EthereumEngineValidatorBuilder;

impl<Node, Types> PayloadValidatorBuilder<Node> for EthereumEngineValidatorBuilder
where
    Types: NodeTypes<
        ChainSpec: Hardforks + EthereumHardforks + Clone + 'static,
        Payload: EngineTypes<ExecutionData = ExecutionData>
                     + PayloadTypes<PayloadAttributes = EthPayloadAttributes>,
        Primitives = EthPrimitives,
    >,
    Node: FullNodeComponents<Types = Types>,
{
    type Validator = EthereumEngineValidator<Types::ChainSpec>;

    async fn build(self, ctx: &AddOnsContext<'_, Node>) -> eyre::Result<Self::Validator> {
        Ok(EthereumEngineValidator::new(ctx.config.chain.clone()))
    }
}
