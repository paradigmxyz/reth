//! Builder support for rpc components.

use std::{
    fmt::{self, Debug},
    future::Future,
    marker::PhantomData,
    ops::{Deref, DerefMut},
};

use alloy_rpc_types::engine::ClientVersionV1;
use futures::TryFutureExt;
use reth_node_api::{
    AddOnsContext, EngineValidator, FullNodeComponents, NodeAddOns, NodeTypes, NodeTypesWithEngine,
};
use reth_node_core::{
    node_config::NodeConfig,
    version::{CARGO_PKG_VERSION, CLIENT_CODE, NAME_CLIENT, VERGEN_GIT_SHA},
};
use reth_payload_builder::PayloadBuilderHandle;
use reth_provider::providers::ProviderNodeTypes;
use reth_rpc::{
    eth::{EthApiTypes, FullEthApiServer},
    EthApi,
};
use reth_rpc_api::eth::helpers::AddDevSigners;
use reth_rpc_builder::{
    auth::{AuthRpcModule, AuthServerHandle},
    config::RethRpcServerConfig,
    RpcModuleBuilder, RpcRegistryInner, RpcServerHandle, TransportRpcModules,
};
use reth_rpc_engine_api::{capabilities::EngineCapabilities, EngineApi};
use reth_tasks::TaskExecutor;
use reth_tracing::tracing::{debug, info};

use crate::EthApiBuilderCtx;

/// Contains the handles to the spawned RPC servers.
///
/// This can be used to access the endpoints of the servers.
#[derive(Debug, Clone)]
pub struct RethRpcServerHandles {
    /// The regular RPC server handle to all configured transports.
    pub rpc: RpcServerHandle,
    /// The handle to the auth server (engine API)
    pub auth: AuthServerHandle,
}

/// Contains hooks that are called during the rpc setup.
pub struct RpcHooks<Node: FullNodeComponents, EthApi> {
    /// Hooks to run once RPC server is running.
    pub on_rpc_started: Box<dyn OnRpcStarted<Node, EthApi>>,
    /// Hooks to run to configure RPC server API.
    pub extend_rpc_modules: Box<dyn ExtendRpcModules<Node, EthApi>>,
}

impl<Node, EthApi> Default for RpcHooks<Node, EthApi>
where
    Node: FullNodeComponents,
    EthApi: EthApiTypes,
{
    fn default() -> Self {
        Self { on_rpc_started: Box::<()>::default(), extend_rpc_modules: Box::<()>::default() }
    }
}

impl<Node, EthApi> RpcHooks<Node, EthApi>
where
    Node: FullNodeComponents,
    EthApi: EthApiTypes,
{
    /// Sets the hook that is run once the rpc server is started.
    pub(crate) fn set_on_rpc_started<F>(&mut self, hook: F) -> &mut Self
    where
        F: OnRpcStarted<Node, EthApi> + 'static,
    {
        self.on_rpc_started = Box::new(hook);
        self
    }

    /// Sets the hook that is run once the rpc server is started.
    #[allow(unused)]
    pub(crate) fn on_rpc_started<F>(mut self, hook: F) -> Self
    where
        F: OnRpcStarted<Node, EthApi> + 'static,
    {
        self.set_on_rpc_started(hook);
        self
    }

    /// Sets the hook that is run to configure the rpc modules.
    pub(crate) fn set_extend_rpc_modules<F>(&mut self, hook: F) -> &mut Self
    where
        F: ExtendRpcModules<Node, EthApi> + 'static,
    {
        self.extend_rpc_modules = Box::new(hook);
        self
    }

    /// Sets the hook that is run to configure the rpc modules.
    #[allow(unused)]
    pub(crate) fn extend_rpc_modules<F>(mut self, hook: F) -> Self
    where
        F: ExtendRpcModules<Node, EthApi> + 'static,
    {
        self.set_extend_rpc_modules(hook);
        self
    }
}

impl<Node, EthApi> fmt::Debug for RpcHooks<Node, EthApi>
where
    Node: FullNodeComponents,
    EthApi: EthApiTypes,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RpcHooks")
            .field("on_rpc_started", &"...")
            .field("extend_rpc_modules", &"...")
            .finish()
    }
}

/// Event hook that is called once the rpc server is started.
pub trait OnRpcStarted<Node: FullNodeComponents, EthApi: EthApiTypes>: Send {
    /// The hook that is called once the rpc server is started.
    fn on_rpc_started(
        self: Box<Self>,
        ctx: RpcContext<'_, Node, EthApi>,
        handles: RethRpcServerHandles,
    ) -> eyre::Result<()>;
}

impl<Node, EthApi, F> OnRpcStarted<Node, EthApi> for F
where
    F: FnOnce(RpcContext<'_, Node, EthApi>, RethRpcServerHandles) -> eyre::Result<()> + Send,
    Node: FullNodeComponents,
    EthApi: EthApiTypes,
{
    fn on_rpc_started(
        self: Box<Self>,
        ctx: RpcContext<'_, Node, EthApi>,
        handles: RethRpcServerHandles,
    ) -> eyre::Result<()> {
        (*self)(ctx, handles)
    }
}

impl<Node, EthApi> OnRpcStarted<Node, EthApi> for ()
where
    Node: FullNodeComponents,
    EthApi: EthApiTypes,
{
    fn on_rpc_started(
        self: Box<Self>,
        _: RpcContext<'_, Node, EthApi>,
        _: RethRpcServerHandles,
    ) -> eyre::Result<()> {
        Ok(())
    }
}

/// Event hook that is called when the rpc server is started.
pub trait ExtendRpcModules<Node: FullNodeComponents, EthApi: EthApiTypes>: Send {
    /// The hook that is called once the rpc server is started.
    fn extend_rpc_modules(self: Box<Self>, ctx: RpcContext<'_, Node, EthApi>) -> eyre::Result<()>;
}

impl<Node, EthApi, F> ExtendRpcModules<Node, EthApi> for F
where
    F: FnOnce(RpcContext<'_, Node, EthApi>) -> eyre::Result<()> + Send,
    Node: FullNodeComponents,
    EthApi: EthApiTypes,
{
    fn extend_rpc_modules(self: Box<Self>, ctx: RpcContext<'_, Node, EthApi>) -> eyre::Result<()> {
        (*self)(ctx)
    }
}

impl<Node, EthApi> ExtendRpcModules<Node, EthApi> for ()
where
    Node: FullNodeComponents,
    EthApi: EthApiTypes,
{
    fn extend_rpc_modules(self: Box<Self>, _: RpcContext<'_, Node, EthApi>) -> eyre::Result<()> {
        Ok(())
    }
}

/// Helper wrapper type to encapsulate the [`RpcRegistryInner`] over components trait.
#[derive(Debug, Clone)]
#[allow(clippy::type_complexity)]
pub struct RpcRegistry<Node: FullNodeComponents, EthApi: EthApiTypes> {
    pub(crate) registry: RpcRegistryInner<
        Node::Provider,
        Node::Pool,
        Node::Network,
        TaskExecutor,
        Node::Provider,
        EthApi,
        Node::Executor,
        Node::Consensus,
    >,
}

impl<Node, EthApi> Deref for RpcRegistry<Node, EthApi>
where
    Node: FullNodeComponents,
    EthApi: EthApiTypes,
{
    type Target = RpcRegistryInner<
        Node::Provider,
        Node::Pool,
        Node::Network,
        TaskExecutor,
        Node::Provider,
        EthApi,
        Node::Executor,
        Node::Consensus,
    >;

    fn deref(&self) -> &Self::Target {
        &self.registry
    }
}

impl<Node, EthApi> DerefMut for RpcRegistry<Node, EthApi>
where
    Node: FullNodeComponents,
    EthApi: EthApiTypes,
{
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.registry
    }
}

/// Helper container to encapsulate [`RpcRegistryInner`], [`TransportRpcModules`] and
/// [`AuthRpcModule`].
///
/// This can be used to access installed modules, or create commonly used handlers like
/// [`reth_rpc::eth::EthApi`], and ultimately merge additional rpc handler into the configured
/// transport modules [`TransportRpcModules`] as well as configured authenticated methods
/// [`AuthRpcModule`].
#[allow(missing_debug_implementations)]
pub struct RpcContext<'a, Node: FullNodeComponents, EthApi: EthApiTypes> {
    /// The node components.
    pub(crate) node: Node,

    /// Gives access to the node configuration.
    pub(crate) config: &'a NodeConfig<<Node::Types as NodeTypes>::ChainSpec>,

    /// A Helper type the holds instances of the configured modules.
    ///
    /// This provides easy access to rpc handlers, such as [`RpcRegistryInner::eth_api`].
    pub registry: &'a mut RpcRegistry<Node, EthApi>,
    /// Holds installed modules per transport type.
    ///
    /// This can be used to merge additional modules into the configured transports (http, ipc,
    /// ws). See [`TransportRpcModules::merge_configured`]
    pub modules: &'a mut TransportRpcModules,
    /// Holds jwt authenticated rpc module.
    ///
    /// This can be used to merge additional modules into the configured authenticated methods
    pub auth_module: &'a mut AuthRpcModule,
}

impl<Node, EthApi> RpcContext<'_, Node, EthApi>
where
    Node: FullNodeComponents,
    EthApi: EthApiTypes,
{
    /// Returns the config of the node.
    pub const fn config(&self) -> &NodeConfig<<Node::Types as NodeTypes>::ChainSpec> {
        self.config
    }

    /// Returns a reference to the configured node.
    pub const fn node(&self) -> &Node {
        &self.node
    }

    /// Returns the transaction pool instance.
    pub fn pool(&self) -> &Node::Pool {
        self.node.pool()
    }

    /// Returns provider to interact with the node.
    pub fn provider(&self) -> &Node::Provider {
        self.node.provider()
    }

    /// Returns the handle to the network
    pub fn network(&self) -> &Node::Network {
        self.node.network()
    }

    /// Returns the handle to the payload builder service
    pub fn payload_builder(
        &self,
    ) -> &PayloadBuilderHandle<<Node::Types as NodeTypesWithEngine>::Engine> {
        self.node.payload_builder()
    }
}

/// Handle to the launched RPC servers.
#[derive(Clone)]
pub struct RpcHandle<Node: FullNodeComponents, EthApi: EthApiTypes> {
    /// Handles to launched servers.
    pub rpc_server_handles: RethRpcServerHandles,
    /// Configured RPC modules.
    pub rpc_registry: RpcRegistry<Node, EthApi>,
}

impl<Node: FullNodeComponents, EthApi: EthApiTypes> Deref for RpcHandle<Node, EthApi> {
    type Target = RpcRegistry<Node, EthApi>;

    fn deref(&self) -> &Self::Target {
        &self.rpc_registry
    }
}

impl<Node: FullNodeComponents, EthApi: EthApiTypes> Debug for RpcHandle<Node, EthApi>
where
    RpcRegistry<Node, EthApi>: Debug,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RpcHandle")
            .field("rpc_server_handles", &self.rpc_server_handles)
            .field("rpc_registry", &self.rpc_registry)
            .finish()
    }
}

/// Node add-ons containing RPC server configuration, with customizable eth API handler.
#[allow(clippy::type_complexity)]
pub struct RpcAddOns<Node: FullNodeComponents, EthApi: EthApiTypes, EV> {
    /// Additional RPC add-ons.
    pub hooks: RpcHooks<Node, EthApi>,
    /// Builder for `EthApi`
    eth_api_builder: Box<dyn FnOnce(&EthApiBuilderCtx<Node>) -> EthApi + Send + Sync>,
    /// Engine validator
    engine_validator_builder: EV,
    _pd: PhantomData<(Node, EthApi)>,
}

impl<Node: FullNodeComponents, EthApi: EthApiTypes, EV: Debug> Debug
    for RpcAddOns<Node, EthApi, EV>
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RpcAddOns")
            .field("hooks", &self.hooks)
            .field("eth_api_builder", &"...")
            .field("engine_validator_builder", &self.engine_validator_builder)
            .finish()
    }
}

impl<Node: FullNodeComponents, EthApi: EthApiTypes, EV> RpcAddOns<Node, EthApi, EV> {
    /// Creates a new instance of the RPC add-ons.
    pub fn new(
        eth_api_builder: impl FnOnce(&EthApiBuilderCtx<Node>) -> EthApi + Send + Sync + 'static,
        engine_validator_builder: EV,
    ) -> Self {
        Self {
            hooks: RpcHooks::default(),
            eth_api_builder: Box::new(eth_api_builder),
            engine_validator_builder,
            _pd: PhantomData,
        }
    }

    /// Sets the hook that is run once the rpc server is started.
    pub fn on_rpc_started<F>(mut self, hook: F) -> Self
    where
        F: FnOnce(RpcContext<'_, Node, EthApi>, RethRpcServerHandles) -> eyre::Result<()>
            + Send
            + 'static,
    {
        self.hooks.set_on_rpc_started(hook);
        self
    }

    /// Sets the hook that is run to configure the rpc modules.
    pub fn extend_rpc_modules<F>(mut self, hook: F) -> Self
    where
        F: FnOnce(RpcContext<'_, Node, EthApi>) -> eyre::Result<()> + Send + 'static,
    {
        self.hooks.set_extend_rpc_modules(hook);
        self
    }
}

impl<Node, EthApi, EV> Default for RpcAddOns<Node, EthApi, EV>
where
    Node: FullNodeComponents,
    EthApi: EthApiTypes + EthApiBuilder<Node>,
    EV: Default,
{
    fn default() -> Self {
        Self::new(EthApi::build, EV::default())
    }
}

impl<N, EthApi, EV> NodeAddOns<N> for RpcAddOns<N, EthApi, EV>
where
    N: FullNodeComponents<Types: ProviderNodeTypes>,
    EthApi: EthApiTypes + FullEthApiServer + AddDevSigners + Unpin + 'static,
    EV: EngineValidatorBuilder<N>,
{
    type Handle = RpcHandle<N, EthApi>;

    async fn launch_add_ons(self, ctx: AddOnsContext<'_, N>) -> eyre::Result<Self::Handle> {
        let Self { eth_api_builder, engine_validator_builder, hooks, _pd: _ } = self;

        let engine_validator = engine_validator_builder.build(&ctx).await?;
        let AddOnsContext { node, config, beacon_engine_handle, jwt_secret } = ctx;

        let client = ClientVersionV1 {
            code: CLIENT_CODE,
            name: NAME_CLIENT.to_string(),
            version: CARGO_PKG_VERSION.to_string(),
            commit: VERGEN_GIT_SHA.to_string(),
        };

        let engine_api = EngineApi::new(
            node.provider().clone(),
            config.chain.clone(),
            beacon_engine_handle,
            node.payload_builder().clone().into(),
            node.pool().clone(),
            Box::new(node.task_executor().clone()),
            client,
            EngineCapabilities::default(),
            engine_validator,
        );
        info!(target: "reth::cli", "Engine API handler initialized");

        let auth_config = config.rpc.auth_server_config(jwt_secret)?;
        let module_config = config.rpc.transport_rpc_module_config();
        debug!(target: "reth::cli", http=?module_config.http(), ws=?module_config.ws(), "Using RPC module config");

        let (mut modules, mut auth_module, registry) = RpcModuleBuilder::default()
            .with_provider(node.provider().clone())
            .with_pool(node.pool().clone())
            .with_network(node.network().clone())
            .with_events(node.provider().clone())
            .with_executor(node.task_executor().clone())
            .with_evm_config(node.evm_config().clone())
            .with_block_executor(node.block_executor().clone())
            .with_consensus(node.consensus().clone())
            .build_with_auth_server(module_config, engine_api, eth_api_builder);

        // in dev mode we generate 20 random dev-signer accounts
        if config.dev.dev {
            registry.eth_api().with_dev_accounts();
        }

        let mut registry = RpcRegistry { registry };
        let ctx = RpcContext {
            node: node.clone(),
            config,
            registry: &mut registry,
            modules: &mut modules,
            auth_module: &mut auth_module,
        };

        let RpcHooks { on_rpc_started, extend_rpc_modules } = hooks;

        extend_rpc_modules.extend_rpc_modules(ctx)?;

        let server_config = config.rpc.rpc_server_config();
        let cloned_modules = modules.clone();
        let launch_rpc = server_config.start(&cloned_modules).map_ok(|handle| {
            if let Some(path) = handle.ipc_endpoint() {
                info!(target: "reth::cli", %path, "RPC IPC server started");
            }
            if let Some(addr) = handle.http_local_addr() {
                info!(target: "reth::cli", url=%addr, "RPC HTTP server started");
            }
            if let Some(addr) = handle.ws_local_addr() {
                info!(target: "reth::cli", url=%addr, "RPC WS server started");
            }
            handle
        });

        let launch_auth = auth_module.clone().start_server(auth_config).map_ok(|handle| {
            let addr = handle.local_addr();
            if let Some(ipc_endpoint) = handle.ipc_endpoint() {
                info!(target: "reth::cli", url=%addr, ipc_endpoint=%ipc_endpoint,"RPC auth server started");
            } else {
                info!(target: "reth::cli", url=%addr, "RPC auth server started");
            }
            handle
        });

        // launch servers concurrently
        let (rpc, auth) = futures::future::try_join(launch_rpc, launch_auth).await?;

        let handles = RethRpcServerHandles { rpc, auth };

        let ctx = RpcContext {
            node: node.clone(),
            config,
            registry: &mut registry,
            modules: &mut modules,
            auth_module: &mut auth_module,
        };

        on_rpc_started.on_rpc_started(ctx, handles.clone())?;

        Ok(RpcHandle { rpc_server_handles: handles, rpc_registry: registry })
    }
}

/// Helper trait implemented for add-ons producing [`RpcHandle`]. Used by common node launcher
/// implementations.
pub trait RethRpcAddOns<N: FullNodeComponents>:
    NodeAddOns<N, Handle = RpcHandle<N, Self::EthApi>>
{
    /// eth API implementation.
    type EthApi: EthApiTypes;

    /// Returns a mutable reference to RPC hooks.
    fn hooks_mut(&mut self) -> &mut RpcHooks<N, Self::EthApi>;
}

impl<N: FullNodeComponents, EthApi: EthApiTypes, EV> RethRpcAddOns<N> for RpcAddOns<N, EthApi, EV>
where
    Self: NodeAddOns<N, Handle = RpcHandle<N, EthApi>>,
{
    type EthApi = EthApi;

    fn hooks_mut(&mut self) -> &mut RpcHooks<N, Self::EthApi> {
        &mut self.hooks
    }
}

/// A `EthApi` that knows how to build itself from [`EthApiBuilderCtx`].
pub trait EthApiBuilder<N: FullNodeComponents>: 'static {
    /// Builds the `EthApi` from the given context.
    fn build(ctx: &EthApiBuilderCtx<N>) -> Self;
}

impl<N: FullNodeComponents> EthApiBuilder<N> for EthApi<N::Provider, N::Pool, N::Network, N::Evm> {
    fn build(ctx: &EthApiBuilderCtx<N>) -> Self {
        Self::with_spawner(ctx)
    }
}

/// A type that knows how to build the engine validator.
pub trait EngineValidatorBuilder<Node: FullNodeComponents>: Send {
    /// The consensus implementation to build.
    type Validator: EngineValidator<<Node::Types as NodeTypesWithEngine>::Engine>
        + Clone
        + Unpin
        + 'static;

    /// Creates the engine validator.
    fn build(
        self,
        ctx: &AddOnsContext<'_, Node>,
    ) -> impl Future<Output = eyre::Result<Self::Validator>> + Send;
}

impl<Node, F, Fut, Validator> EngineValidatorBuilder<Node> for F
where
    Node: FullNodeComponents,
    Validator:
        EngineValidator<<Node::Types as NodeTypesWithEngine>::Engine> + Clone + Unpin + 'static,
    F: FnOnce(&AddOnsContext<'_, Node>) -> Fut + Send,
    Fut: Future<Output = eyre::Result<Validator>> + Send,
{
    type Validator = Validator;

    fn build(
        self,
        ctx: &AddOnsContext<'_, Node>,
    ) -> impl Future<Output = eyre::Result<Self::Validator>> {
        self(ctx)
    }
}
