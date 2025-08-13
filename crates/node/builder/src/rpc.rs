//! Builder support for rpc components.

pub use jsonrpsee::server::middleware::rpc::{RpcService, RpcServiceBuilder};
pub use reth_engine_tree::tree::{BasicEngineValidator, EngineValidator};
pub use reth_rpc_builder::{middleware::RethRpcMiddleware, Identity, Stack};

use crate::{
    invalid_block_hook::InvalidBlockHookExt, BeaconConsensusEngineEvent,
    BeaconConsensusEngineHandle, ConfigureEngineEvm,
};
use alloy_rpc_types::engine::ClientVersionV1;
use alloy_rpc_types_engine::ExecutionData;
use jsonrpsee::{core::middleware::layer::Either, RpcModule};
use reth_chain_state::CanonStateSubscriptions;
use reth_chainspec::{ChainSpecProvider, EthChainSpec, EthereumHardforks};
use reth_node_api::{
    AddOnsContext, BlockTy, EngineApiValidator, EngineTypes, FullNodeComponents, FullNodeTypes,
    NodeAddOns, NodeTypes, PayloadTypes, PayloadValidator, PrimitivesTy, TreeConfig,
};
use reth_node_core::{
    node_config::NodeConfig,
    version::{CARGO_PKG_VERSION, CLIENT_CODE, NAME_CLIENT, VERGEN_GIT_SHA},
};
use reth_payload_builder::{PayloadBuilderHandle, PayloadStore};
use reth_rpc::eth::{core::EthRpcConverterFor, EthApiTypes, FullEthApiServer};
use reth_rpc_api::{eth::helpers::AddDevSigners, IntoEngineApiRpcModule};
use reth_rpc_builder::{
    auth::{AuthRpcModule, AuthServerHandle},
    config::RethRpcServerConfig,
    RpcModuleBuilder, RpcRegistryInner, RpcServerConfig, RpcServerHandle, TransportRpcModules,
};
use reth_rpc_engine_api::{capabilities::EngineCapabilities, EngineApi};
use reth_rpc_eth_types::{cache::cache_new_blocks_task, EthConfig, EthStateCache};
use reth_tokio_util::EventSender;
use reth_tracing::tracing::{debug, info};
use std::{
    fmt::{self, Debug},
    future::Future,
    ops::{Deref, DerefMut},
};

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
    #[expect(unused)]
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
    #[expect(unused)]
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
#[expect(clippy::type_complexity)]
pub struct RpcRegistry<Node: FullNodeComponents, EthApi: EthApiTypes> {
    pub(crate) registry: RpcRegistryInner<
        Node::Provider,
        Node::Pool,
        Node::Network,
        EthApi,
        Node::Evm,
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
        EthApi,
        Node::Evm,
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

/// Helper container for the parameters commonly passed to RPC module extension functions.
#[expect(missing_debug_implementations)]
pub struct RpcModuleContainer<'a, Node: FullNodeComponents, EthApi: EthApiTypes> {
    /// Holds installed modules per transport type.
    pub modules: &'a mut TransportRpcModules,
    /// Holds jwt authenticated rpc module.
    pub auth_module: &'a mut AuthRpcModule,
    /// A Helper type the holds instances of the configured modules.
    pub registry: &'a mut RpcRegistry<Node, EthApi>,
}

/// Helper container to encapsulate [`RpcRegistryInner`], [`TransportRpcModules`] and
/// [`AuthRpcModule`].
///
/// This can be used to access installed modules, or create commonly used handlers like
/// [`reth_rpc::eth::EthApi`], and ultimately merge additional rpc handler into the configured
/// transport modules [`TransportRpcModules`] as well as configured authenticated methods
/// [`AuthRpcModule`].
#[expect(missing_debug_implementations)]
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
    ///
    /// This gives access to the node's components.
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
    pub fn payload_builder_handle(
        &self,
    ) -> &PayloadBuilderHandle<<Node::Types as NodeTypes>::Payload> {
        self.node.payload_builder_handle()
    }
}

/// Handle to the launched RPC servers.
pub struct RpcHandle<Node: FullNodeComponents, EthApi: EthApiTypes> {
    /// Handles to launched servers.
    pub rpc_server_handles: RethRpcServerHandles,
    /// Configured RPC modules.
    pub rpc_registry: RpcRegistry<Node, EthApi>,
    /// Notification channel for engine API events
    ///
    /// Caution: This is a multi-producer, multi-consumer broadcast and allows grants access to
    /// dispatch events
    pub engine_events:
        EventSender<BeaconConsensusEngineEvent<<Node::Types as NodeTypes>::Primitives>>,
    /// Handle to the beacon consensus engine.
    pub beacon_engine_handle: BeaconConsensusEngineHandle<<Node::Types as NodeTypes>::Payload>,
}

impl<Node: FullNodeComponents, EthApi: EthApiTypes> Clone for RpcHandle<Node, EthApi> {
    fn clone(&self) -> Self {
        Self {
            rpc_server_handles: self.rpc_server_handles.clone(),
            rpc_registry: self.rpc_registry.clone(),
            engine_events: self.engine_events.clone(),
            beacon_engine_handle: self.beacon_engine_handle.clone(),
        }
    }
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

/// Handle returned when only the regular RPC server (HTTP/WS/IPC) is launched.
///
/// This handle provides access to the RPC server endpoints and registry, but does not
/// include an authenticated Engine API server. Use this when you only need regular
/// RPC functionality.
#[derive(Debug, Clone)]
pub struct RpcServerOnlyHandle<Node: FullNodeComponents, EthApi: EthApiTypes> {
    /// Handle to the RPC server
    pub rpc_server_handle: RpcServerHandle,
    /// Configured RPC modules.
    pub rpc_registry: RpcRegistry<Node, EthApi>,
    /// Notification channel for engine API events
    pub engine_events:
        EventSender<BeaconConsensusEngineEvent<<Node::Types as NodeTypes>::Primitives>>,
    /// Handle to the consensus engine.
    pub engine_handle: BeaconConsensusEngineHandle<<Node::Types as NodeTypes>::Payload>,
}

/// Handle returned when only the authenticated Engine API server is launched.
///
/// This handle provides access to the Engine API server and registry, but does not
/// include the regular RPC servers (HTTP/WS/IPC). Use this for specialized setups
/// that only need Engine API functionality.
#[derive(Debug, Clone)]
pub struct AuthServerOnlyHandle<Node: FullNodeComponents, EthApi: EthApiTypes> {
    /// Handle to the auth server (engine API)
    pub auth_server_handle: AuthServerHandle,
    /// Configured RPC modules.
    pub rpc_registry: RpcRegistry<Node, EthApi>,
    /// Notification channel for engine API events
    pub engine_events:
        EventSender<BeaconConsensusEngineEvent<<Node::Types as NodeTypes>::Primitives>>,
    /// Handle to the consensus engine.
    pub engine_handle: BeaconConsensusEngineHandle<<Node::Types as NodeTypes>::Payload>,
}

/// Internal context struct for RPC setup shared between different launch methods
struct RpcSetupContext<'a, Node: FullNodeComponents, EthApi: EthApiTypes> {
    node: Node,
    config: &'a NodeConfig<<Node::Types as NodeTypes>::ChainSpec>,
    modules: TransportRpcModules,
    auth_module: AuthRpcModule,
    auth_config: reth_rpc_builder::auth::AuthServerConfig,
    registry: RpcRegistry<Node, EthApi>,
    on_rpc_started: Box<dyn OnRpcStarted<Node, EthApi>>,
    engine_events: EventSender<BeaconConsensusEngineEvent<<Node::Types as NodeTypes>::Primitives>>,
    engine_handle: BeaconConsensusEngineHandle<<Node::Types as NodeTypes>::Payload>,
}

/// Node add-ons containing RPC server configuration, with customizable eth API handler.
///
/// This struct can be used to provide the RPC server functionality. It is responsible for launching
/// the regular RPC and the authenticated RPC server (engine API). It is intended to be used and
/// modified as part of the [`NodeAddOns`] see for example `OpRpcAddons`, `EthereumAddOns`.
///
/// It can be modified to register RPC API handlers, see [`RpcAddOns::launch_add_ons_with`] which
/// takes a closure that provides access to all the configured modules (namespaces), and is invoked
/// just before the servers are launched. This can be used to extend the node with custom RPC
/// methods or even replace existing method handlers, see also [`TransportRpcModules`].
pub struct RpcAddOns<
    Node: FullNodeComponents,
    EthB: EthApiBuilder<Node>,
    PVB,
    EB = BasicEngineApiBuilder<PVB>,
    EVB = BasicEngineValidatorBuilder<PVB>,
    RpcMiddleware = Identity,
> {
    /// Additional RPC add-ons.
    pub hooks: RpcHooks<Node, EthB::EthApi>,
    /// Builder for `EthApi`
    eth_api_builder: EthB,
    /// Payload validator builder
    payload_validator_builder: PVB,
    /// Builder for `EngineApi`
    engine_api_builder: EB,
    /// Builder for tree validator
    engine_validator_builder: EVB,
    /// Configurable RPC middleware stack.
    ///
    /// This middleware is applied to all RPC requests across all transports (HTTP, WS, IPC).
    /// See [`RpcAddOns::with_rpc_middleware`] for more details.
    rpc_middleware: RpcMiddleware,
}

impl<Node, EthB, PVB, EB, EVB, RpcMiddleware> Debug
    for RpcAddOns<Node, EthB, PVB, EB, EVB, RpcMiddleware>
where
    Node: FullNodeComponents,
    EthB: EthApiBuilder<Node>,
    PVB: Debug,
    EB: Debug,
    EVB: Debug,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RpcAddOns")
            .field("hooks", &self.hooks)
            .field("eth_api_builder", &"...")
            .field("payload_validator_builder", &self.payload_validator_builder)
            .field("engine_api_builder", &self.engine_api_builder)
            .field("engine_validator_builder", &self.engine_validator_builder)
            .field("rpc_middleware", &"...")
            .finish()
    }
}

impl<Node, EthB, PVB, EB, EVB, RpcMiddleware> RpcAddOns<Node, EthB, PVB, EB, EVB, RpcMiddleware>
where
    Node: FullNodeComponents,
    EthB: EthApiBuilder<Node>,
{
    /// Creates a new instance of the RPC add-ons.
    pub fn new(
        eth_api_builder: EthB,
        payload_validator_builder: PVB,
        engine_api_builder: EB,
        engine_validator_builder: EVB,
        rpc_middleware: RpcMiddleware,
    ) -> Self {
        Self {
            hooks: RpcHooks::default(),
            eth_api_builder,
            payload_validator_builder,
            engine_api_builder,
            engine_validator_builder,
            rpc_middleware,
        }
    }

    /// Maps the [`EngineApiBuilder`] builder type.
    pub fn with_engine_api<T>(
        self,
        engine_api_builder: T,
    ) -> RpcAddOns<Node, EthB, PVB, T, EVB, RpcMiddleware> {
        let Self {
            hooks,
            eth_api_builder,
            payload_validator_builder,
            engine_validator_builder,
            rpc_middleware,
            ..
        } = self;
        RpcAddOns {
            hooks,
            eth_api_builder,
            payload_validator_builder,
            engine_api_builder,
            engine_validator_builder,
            rpc_middleware,
        }
    }

    /// Maps the [`PayloadValidatorBuilder`] builder type.
    pub fn with_payload_validator<T>(
        self,
        payload_validator_builder: T,
    ) -> RpcAddOns<Node, EthB, T, EB, EVB, RpcMiddleware> {
        let Self {
            hooks,
            eth_api_builder,
            engine_api_builder,
            engine_validator_builder,
            rpc_middleware,
            ..
        } = self;
        RpcAddOns {
            hooks,
            eth_api_builder,
            payload_validator_builder,
            engine_api_builder,
            engine_validator_builder,
            rpc_middleware,
        }
    }

    /// Maps the [`EngineValidatorBuilder`] builder type.
    pub fn with_engine_validator<T>(
        self,
        engine_validator_builder: T,
    ) -> RpcAddOns<Node, EthB, PVB, EB, T, RpcMiddleware> {
        let Self {
            hooks,
            eth_api_builder,
            payload_validator_builder,
            engine_api_builder,
            rpc_middleware,
            ..
        } = self;
        RpcAddOns {
            hooks,
            eth_api_builder,
            payload_validator_builder,
            engine_api_builder,
            engine_validator_builder,
            rpc_middleware,
        }
    }

    /// Sets the RPC middleware stack for processing RPC requests.
    ///
    /// This method configures a custom middleware stack that will be applied to all RPC requests
    /// across HTTP, `WebSocket`, and IPC transports. The middleware is applied to the RPC service
    /// layer, allowing you to intercept, modify, or enhance RPC request processing.
    ///
    ///
    /// # How It Works
    ///
    /// The middleware uses the Tower ecosystem's `Layer` pattern. When an RPC server is started,
    /// the configured middleware stack is applied to create a layered service that processes
    /// requests in the order the layers were added.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// use reth_rpc_builder::{RpcServiceBuilder, RpcRequestMetrics};
    /// use tower::Layer;
    ///
    /// // Simple example with metrics
    /// let metrics_layer = RpcRequestMetrics::new(metrics_recorder);
    /// let with_metrics = rpc_addons.with_rpc_middleware(
    ///     RpcServiceBuilder::new().layer(metrics_layer)
    /// );
    ///
    /// // Composing multiple middleware layers
    /// let middleware_stack = RpcServiceBuilder::new()
    ///     .layer(rate_limit_layer)
    ///     .layer(logging_layer)
    ///     .layer(metrics_layer);
    /// let with_full_stack = rpc_addons.with_rpc_middleware(middleware_stack);
    /// ```
    ///
    /// # Notes
    ///
    /// - Middleware is applied to the RPC service layer, not the HTTP transport layer
    /// - The default middleware is `Identity` (no-op), which passes through requests unchanged
    /// - Middleware layers are applied in the order they are added via `.layer()`
    pub fn with_rpc_middleware<T>(
        self,
        rpc_middleware: T,
    ) -> RpcAddOns<Node, EthB, PVB, EB, EVB, T> {
        let Self {
            hooks,
            eth_api_builder,
            payload_validator_builder,
            engine_api_builder,
            engine_validator_builder,
            ..
        } = self;
        RpcAddOns {
            hooks,
            eth_api_builder,
            payload_validator_builder,
            engine_api_builder,
            engine_validator_builder,
            rpc_middleware,
        }
    }

    /// Add a new layer `T` to the configured [`RpcServiceBuilder`].
    pub fn layer_rpc_middleware<T>(
        self,
        layer: T,
    ) -> RpcAddOns<Node, EthB, PVB, EB, EVB, Stack<RpcMiddleware, T>> {
        let Self {
            hooks,
            eth_api_builder,
            payload_validator_builder,
            engine_api_builder,
            engine_validator_builder,
            rpc_middleware,
        } = self;
        let rpc_middleware = Stack::new(rpc_middleware, layer);
        RpcAddOns {
            hooks,
            eth_api_builder,
            payload_validator_builder,
            engine_api_builder,
            engine_validator_builder,
            rpc_middleware,
        }
    }

    /// Optionally adds a new layer `T` to the configured [`RpcServiceBuilder`].
    #[expect(clippy::type_complexity)]
    pub fn option_layer_rpc_middleware<T>(
        self,
        layer: Option<T>,
    ) -> RpcAddOns<Node, EthB, PVB, EB, EVB, Stack<RpcMiddleware, Either<T, Identity>>> {
        let layer = layer.map(Either::Left).unwrap_or(Either::Right(Identity::new()));
        self.layer_rpc_middleware(layer)
    }

    /// Sets the hook that is run once the rpc server is started.
    pub fn on_rpc_started<F>(mut self, hook: F) -> Self
    where
        F: FnOnce(RpcContext<'_, Node, EthB::EthApi>, RethRpcServerHandles) -> eyre::Result<()>
            + Send
            + 'static,
    {
        self.hooks.set_on_rpc_started(hook);
        self
    }

    /// Sets the hook that is run to configure the rpc modules.
    pub fn extend_rpc_modules<F>(mut self, hook: F) -> Self
    where
        F: FnOnce(RpcContext<'_, Node, EthB::EthApi>) -> eyre::Result<()> + Send + 'static,
    {
        self.hooks.set_extend_rpc_modules(hook);
        self
    }
}

impl<Node, EthB, EV, EB, Engine> Default for RpcAddOns<Node, EthB, EV, EB, Engine, Identity>
where
    Node: FullNodeComponents,
    EthB: EthApiBuilder<Node>,
    EV: Default,
    EB: Default,
    Engine: Default,
{
    fn default() -> Self {
        Self::new(
            EthB::default(),
            EV::default(),
            EB::default(),
            Engine::default(),
            Default::default(),
        )
    }
}

impl<N, EthB, PVB, EB, EVB, RpcMiddleware> RpcAddOns<N, EthB, PVB, EB, EVB, RpcMiddleware>
where
    N: FullNodeComponents,
    N::Provider: ChainSpecProvider<ChainSpec: EthereumHardforks>,
    EthB: EthApiBuilder<N>,
    EB: EngineApiBuilder<N>,
    EVB: EngineValidatorBuilder<N>,
    RpcMiddleware: RethRpcMiddleware,
{
    /// Launches only the regular RPC server (HTTP/WS/IPC), without the authenticated Engine API
    /// server.
    ///
    /// This is useful when you only need the regular RPC functionality and want to avoid
    /// starting the auth server.
    pub async fn launch_rpc_server<F>(
        self,
        ctx: AddOnsContext<'_, N>,
        ext: F,
    ) -> eyre::Result<RpcServerOnlyHandle<N, EthB::EthApi>>
    where
        F: FnOnce(RpcModuleContainer<'_, N, EthB::EthApi>) -> eyre::Result<()>,
    {
        let rpc_middleware = self.rpc_middleware.clone();
        let setup_ctx = self.setup_rpc_components(ctx, ext).await?;
        let RpcSetupContext {
            node,
            config,
            mut modules,
            mut auth_module,
            auth_config: _,
            mut registry,
            on_rpc_started,
            engine_events,
            engine_handle,
        } = setup_ctx;

        let server_config = config.rpc.rpc_server_config().set_rpc_middleware(rpc_middleware);
        let rpc_server_handle = Self::launch_rpc_server_internal(server_config, &modules).await?;

        let handles =
            RethRpcServerHandles { rpc: rpc_server_handle.clone(), auth: AuthServerHandle::noop() };
        Self::finalize_rpc_setup(
            &mut registry,
            &mut modules,
            &mut auth_module,
            &node,
            config,
            on_rpc_started,
            handles,
        )?;

        Ok(RpcServerOnlyHandle {
            rpc_server_handle,
            rpc_registry: registry,
            engine_events,
            engine_handle,
        })
    }

    /// Launches the RPC servers with the given context and an additional hook for extending
    /// modules. Whether the auth server is launched depends on the CLI configuration.
    pub async fn launch_add_ons_with<F>(
        self,
        ctx: AddOnsContext<'_, N>,
        ext: F,
    ) -> eyre::Result<RpcHandle<N, EthB::EthApi>>
    where
        F: FnOnce(RpcModuleContainer<'_, N, EthB::EthApi>) -> eyre::Result<()>,
    {
        // Check CLI config to determine if auth server should be disabled
        let disable_auth = ctx.config.rpc.disable_auth_server;
        self.launch_add_ons_with_opt_engine(ctx, ext, disable_auth).await
    }

    /// Launches the RPC servers with the given context and an additional hook for extending
    /// modules. Optionally disables the auth server based on the `disable_auth` parameter.
    ///
    /// When `disable_auth` is true, the auth server will not be started and a noop handle
    /// will be used instead.
    pub async fn launch_add_ons_with_opt_engine<F>(
        self,
        ctx: AddOnsContext<'_, N>,
        ext: F,
        disable_auth: bool,
    ) -> eyre::Result<RpcHandle<N, EthB::EthApi>>
    where
        F: FnOnce(RpcModuleContainer<'_, N, EthB::EthApi>) -> eyre::Result<()>,
    {
        let rpc_middleware = self.rpc_middleware.clone();
        let setup_ctx = self.setup_rpc_components(ctx, ext).await?;
        let RpcSetupContext {
            node,
            config,
            mut modules,
            mut auth_module,
            auth_config,
            mut registry,
            on_rpc_started,
            engine_events,
            engine_handle,
        } = setup_ctx;

        let server_config = config.rpc.rpc_server_config().set_rpc_middleware(rpc_middleware);

        let (rpc, auth) = if disable_auth {
            // Only launch the RPC server, use a noop auth handle
            let rpc = Self::launch_rpc_server_internal(server_config, &modules).await?;
            (rpc, AuthServerHandle::noop())
        } else {
            let auth_module_clone = auth_module.clone();
            // launch servers concurrently
            let (rpc, auth) = futures::future::try_join(
                Self::launch_rpc_server_internal(server_config, &modules),
                Self::launch_auth_server_internal(auth_module_clone, auth_config),
            )
            .await?;
            (rpc, auth)
        };

        let handles = RethRpcServerHandles { rpc, auth };

        Self::finalize_rpc_setup(
            &mut registry,
            &mut modules,
            &mut auth_module,
            &node,
            config,
            on_rpc_started,
            handles.clone(),
        )?;

        Ok(RpcHandle {
            rpc_server_handles: handles,
            rpc_registry: registry,
            engine_events,
            beacon_engine_handle: engine_handle,
        })
    }

    /// Common setup for RPC server initialization
    async fn setup_rpc_components<'a, F>(
        self,
        ctx: AddOnsContext<'a, N>,
        ext: F,
    ) -> eyre::Result<RpcSetupContext<'a, N, EthB::EthApi>>
    where
        F: FnOnce(RpcModuleContainer<'_, N, EthB::EthApi>) -> eyre::Result<()>,
    {
        let Self { eth_api_builder, engine_api_builder, hooks, .. } = self;

        let engine_api = engine_api_builder.build_engine_api(&ctx).await?;
        let AddOnsContext { node, config, beacon_engine_handle, jwt_secret, engine_events } = ctx;

        info!(target: "reth::cli", "Engine API handler initialized");

        let cache = EthStateCache::spawn_with(
            node.provider().clone(),
            config.rpc.eth_config().cache,
            node.task_executor().clone(),
        );

        let new_canonical_blocks = node.provider().canonical_state_stream();
        let c = cache.clone();
        node.task_executor().spawn_critical(
            "cache canonical blocks task",
            Box::pin(async move {
                cache_new_blocks_task(c, new_canonical_blocks).await;
            }),
        );

        let ctx = EthApiCtx { components: &node, config: config.rpc.eth_config(), cache };
        let eth_api = eth_api_builder.build_eth_api(ctx).await?;

        let auth_config = config.rpc.auth_server_config(jwt_secret)?;
        let module_config = config.rpc.transport_rpc_module_config();
        debug!(target: "reth::cli", http=?module_config.http(), ws=?module_config.ws(), "Using RPC module config");

        let (mut modules, mut auth_module, registry) = RpcModuleBuilder::default()
            .with_provider(node.provider().clone())
            .with_pool(node.pool().clone())
            .with_network(node.network().clone())
            .with_executor(Box::new(node.task_executor().clone()))
            .with_evm_config(node.evm_config().clone())
            .with_consensus(node.consensus().clone())
            .build_with_auth_server(module_config, engine_api, eth_api);

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

        ext(RpcModuleContainer {
            modules: ctx.modules,
            auth_module: ctx.auth_module,
            registry: ctx.registry,
        })?;
        extend_rpc_modules.extend_rpc_modules(ctx)?;

        Ok(RpcSetupContext {
            node,
            config,
            modules,
            auth_module,
            auth_config,
            registry,
            on_rpc_started,
            engine_events,
            engine_handle: beacon_engine_handle,
        })
    }

    /// Helper to launch the RPC server
    async fn launch_rpc_server_internal<M>(
        server_config: RpcServerConfig<M>,
        modules: &TransportRpcModules,
    ) -> eyre::Result<RpcServerHandle>
    where
        M: RethRpcMiddleware,
    {
        let handle = server_config.start(modules).await?;

        if let Some(path) = handle.ipc_endpoint() {
            info!(target: "reth::cli", %path, "RPC IPC server started");
        }
        if let Some(addr) = handle.http_local_addr() {
            info!(target: "reth::cli", url=%addr, "RPC HTTP server started");
        }
        if let Some(addr) = handle.ws_local_addr() {
            info!(target: "reth::cli", url=%addr, "RPC WS server started");
        }

        Ok(handle)
    }

    /// Helper to launch the auth server
    async fn launch_auth_server_internal(
        auth_module: AuthRpcModule,
        auth_config: reth_rpc_builder::auth::AuthServerConfig,
    ) -> eyre::Result<AuthServerHandle> {
        auth_module.start_server(auth_config)
            .await
            .map_err(Into::into)
            .inspect(|handle| {
                let addr = handle.local_addr();
                if let Some(ipc_endpoint) = handle.ipc_endpoint() {
                    info!(target: "reth::cli", url=%addr, ipc_endpoint=%ipc_endpoint, "RPC auth server started");
                } else {
                    info!(target: "reth::cli", url=%addr, "RPC auth server started");
                }
            })
    }

    /// Helper to finalize RPC setup by creating context and calling hooks
    fn finalize_rpc_setup(
        registry: &mut RpcRegistry<N, EthB::EthApi>,
        modules: &mut TransportRpcModules,
        auth_module: &mut AuthRpcModule,
        node: &N,
        config: &NodeConfig<<N::Types as NodeTypes>::ChainSpec>,
        on_rpc_started: Box<dyn OnRpcStarted<N, EthB::EthApi>>,
        handles: RethRpcServerHandles,
    ) -> eyre::Result<()> {
        let ctx = RpcContext { node: node.clone(), config, registry, modules, auth_module };

        on_rpc_started.on_rpc_started(ctx, handles)?;
        Ok(())
    }
}

impl<N, EthB, PVB, EB, EVB, RpcMiddleware> NodeAddOns<N>
    for RpcAddOns<N, EthB, PVB, EB, EVB, RpcMiddleware>
where
    N: FullNodeComponents,
    <N as FullNodeTypes>::Provider: ChainSpecProvider<ChainSpec: EthereumHardforks>,
    EthB: EthApiBuilder<N>,
    PVB: PayloadValidatorBuilder<N>,
    EB: EngineApiBuilder<N>,
    EVB: EngineValidatorBuilder<N>,
    RpcMiddleware: RethRpcMiddleware,
{
    type Handle = RpcHandle<N, EthB::EthApi>;

    async fn launch_add_ons(self, ctx: AddOnsContext<'_, N>) -> eyre::Result<Self::Handle> {
        self.launch_add_ons_with(ctx, |_| Ok(())).await
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

impl<N: FullNodeComponents, EthB, EV, EB, Engine, RpcMiddleware> RethRpcAddOns<N>
    for RpcAddOns<N, EthB, EV, EB, Engine, RpcMiddleware>
where
    Self: NodeAddOns<N, Handle = RpcHandle<N, EthB::EthApi>>,
    EthB: EthApiBuilder<N>,
{
    type EthApi = EthB::EthApi;

    fn hooks_mut(&mut self) -> &mut RpcHooks<N, Self::EthApi> {
        &mut self.hooks
    }
}

/// `EthApiCtx` struct
/// This struct is used to pass the necessary context to the `EthApiBuilder` to build the `EthApi`.
#[derive(Debug)]
pub struct EthApiCtx<'a, N: FullNodeTypes> {
    /// Reference to the node components
    pub components: &'a N,
    /// Eth API configuration
    pub config: EthConfig,
    /// Cache for eth state
    pub cache: EthStateCache<PrimitivesTy<N::Types>>,
}

impl<'a, N: FullNodeComponents<Types: NodeTypes<ChainSpec: EthereumHardforks>>> EthApiCtx<'a, N> {
    /// Provides a [`EthApiBuilder`] with preconfigured config and components.
    pub fn eth_api_builder(self) -> reth_rpc::EthApiBuilder<N, EthRpcConverterFor<N>> {
        reth_rpc::EthApiBuilder::new_with_components(self.components.clone())
            .eth_cache(self.cache)
            .task_spawner(self.components.task_executor().clone())
            .gas_cap(self.config.rpc_gas_cap.into())
            .max_simulate_blocks(self.config.rpc_max_simulate_blocks)
            .eth_proof_window(self.config.eth_proof_window)
            .fee_history_cache_config(self.config.fee_history_cache)
            .proof_permits(self.config.proof_permits)
            .gas_oracle_config(self.config.gas_oracle)
    }
}

/// A `EthApi` that knows how to build `eth` namespace API from [`FullNodeComponents`].
pub trait EthApiBuilder<N: FullNodeComponents>: Default + Send + 'static {
    /// The Ethapi implementation this builder will build.
    type EthApi: EthApiTypes
        + FullEthApiServer<Provider = N::Provider, Pool = N::Pool>
        + AddDevSigners
        + Unpin
        + 'static;

    /// Builds the [`EthApiServer`](reth_rpc_api::eth::EthApiServer) from the given context.
    fn build_eth_api(
        self,
        ctx: EthApiCtx<'_, N>,
    ) -> impl Future<Output = eyre::Result<Self::EthApi>> + Send;
}

/// Helper trait that provides the validator builder for the engine API
pub trait EngineValidatorAddOn<Node: FullNodeComponents>: Send {
    /// The validator builder type to use.
    type ValidatorBuilder: EngineValidatorBuilder<Node>;

    /// Returns the validator builder.
    fn engine_validator_builder(&self) -> Self::ValidatorBuilder;
}

impl<N, EthB, PVB, EB, EVB> EngineValidatorAddOn<N> for RpcAddOns<N, EthB, PVB, EB, EVB>
where
    N: FullNodeComponents,
    EthB: EthApiBuilder<N>,
    PVB: Send,
    EB: EngineApiBuilder<N>,
    EVB: EngineValidatorBuilder<N>,
{
    type ValidatorBuilder = EVB;

    fn engine_validator_builder(&self) -> Self::ValidatorBuilder {
        self.engine_validator_builder.clone()
    }
}

/// Builder for engine API RPC module.
///
/// This builder type is responsible for providing an instance of [`IntoEngineApiRpcModule`], which
/// is effectively a helper trait that provides the type erased [`jsonrpsee::RpcModule`] instance
/// that contains the method handlers for the engine API. See [`EngineApi`] for an implementation of
/// [`IntoEngineApiRpcModule`].
pub trait EngineApiBuilder<Node: FullNodeComponents>: Send + Sync {
    /// The engine API RPC module. Only required to be convertible to an [`jsonrpsee::RpcModule`].
    type EngineApi: IntoEngineApiRpcModule + Send + Sync;

    /// Builds the engine API instance given the provided [`AddOnsContext`].
    ///
    /// [`Self::EngineApi`] will be converted into the method handlers of the authenticated RPC
    /// server (engine API).
    fn build_engine_api(
        self,
        ctx: &AddOnsContext<'_, Node>,
    ) -> impl Future<Output = eyre::Result<Self::EngineApi>> + Send;
}

/// Builder trait for creating payload validators specifically for the Engine API.
///
/// This trait is responsible for building validators that the Engine API will use
/// to validate payloads.
pub trait PayloadValidatorBuilder<Node: FullNodeComponents>: Send + Sync + Clone {
    /// The validator type that will be used by the Engine API.
    type Validator: PayloadValidator<<Node::Types as NodeTypes>::Payload>;

    /// Builds the engine API validator.
    ///
    /// Returns a validator that validates engine API version-specific fields and payload
    /// attributes.
    fn build(
        self,
        ctx: &AddOnsContext<'_, Node>,
    ) -> impl Future<Output = eyre::Result<Self::Validator>> + Send;
}

/// Builder trait for creating engine validators for the consensus engine.
///
/// This trait is responsible for building validators that the consensus engine will use
/// for block execution, state validation, and fork handling.
pub trait EngineValidatorBuilder<Node: FullNodeComponents>: Send + Sync + Clone {
    /// The tree validator type that will be used by the consensus engine.
    type EngineValidator: EngineValidator<
        <Node::Types as NodeTypes>::Payload,
        <Node::Types as NodeTypes>::Primitives,
    >;

    /// Builds the tree validator for the consensus engine.
    ///
    /// Returns a validator that handles block execution, state validation, and fork handling.
    fn build_tree_validator(
        self,
        ctx: &AddOnsContext<'_, Node>,
        tree_config: TreeConfig,
    ) -> impl Future<Output = eyre::Result<Self::EngineValidator>> + Send;
}

/// Basic implementation of [`EngineValidatorBuilder`].
///
/// This builder creates a [`BasicEngineValidator`] using the provided payload validator builder.
#[derive(Debug, Clone)]
pub struct BasicEngineValidatorBuilder<EV> {
    /// The payload validator builder used to create the engine validator.
    payload_validator_builder: EV,
}

impl<EV> BasicEngineValidatorBuilder<EV> {
    /// Creates a new instance with the given payload validator builder.
    pub const fn new(payload_validator_builder: EV) -> Self {
        Self { payload_validator_builder }
    }
}

impl<EV> Default for BasicEngineValidatorBuilder<EV>
where
    EV: Default,
{
    fn default() -> Self {
        Self::new(EV::default())
    }
}

impl<Node, EV> EngineValidatorBuilder<Node> for BasicEngineValidatorBuilder<EV>
where
    Node: FullNodeComponents<
        Evm: ConfigureEngineEvm<
            <<Node::Types as NodeTypes>::Payload as PayloadTypes>::ExecutionData,
        >,
    >,
    EV: PayloadValidatorBuilder<Node>,
    EV::Validator: reth_engine_primitives::PayloadValidator<
        <Node::Types as NodeTypes>::Payload,
        Block = BlockTy<Node::Types>,
    >,
{
    type EngineValidator = BasicEngineValidator<Node::Provider, Node::Evm, EV::Validator>;

    async fn build_tree_validator(
        self,
        ctx: &AddOnsContext<'_, Node>,
        tree_config: TreeConfig,
    ) -> eyre::Result<Self::EngineValidator> {
        let validator = self.payload_validator_builder.build(ctx).await?;
        let data_dir = ctx.config.datadir.clone().resolve_datadir(ctx.config.chain.chain());
        let invalid_block_hook = ctx.create_invalid_block_hook(&data_dir).await?;
        Ok(BasicEngineValidator::new(
            ctx.node.provider().clone(),
            std::sync::Arc::new(ctx.node.consensus().clone()),
            ctx.node.evm_config().clone(),
            validator,
            tree_config,
            invalid_block_hook,
        ))
    }
}

/// Builder for basic [`EngineApi`] implementation.
///
/// This provides a basic default implementation for opstack and ethereum engine API via
/// [`EngineTypes`] and uses the general purpose [`EngineApi`] implementation as the builder's
/// output.
#[derive(Debug, Default)]
pub struct BasicEngineApiBuilder<PVB> {
    payload_validator_builder: PVB,
}

impl<N, PVB> EngineApiBuilder<N> for BasicEngineApiBuilder<PVB>
where
    N: FullNodeComponents<
        Types: NodeTypes<
            ChainSpec: EthereumHardforks,
            Payload: PayloadTypes<ExecutionData = ExecutionData> + EngineTypes,
        >,
    >,
    PVB: PayloadValidatorBuilder<N>,
    PVB::Validator: EngineApiValidator<<N::Types as NodeTypes>::Payload>,
{
    type EngineApi = EngineApi<
        N::Provider,
        <N::Types as NodeTypes>::Payload,
        N::Pool,
        PVB::Validator,
        <N::Types as NodeTypes>::ChainSpec,
    >;

    async fn build_engine_api(self, ctx: &AddOnsContext<'_, N>) -> eyre::Result<Self::EngineApi> {
        let Self { payload_validator_builder } = self;

        let engine_validator = payload_validator_builder.build(ctx).await?;
        let client = ClientVersionV1 {
            code: CLIENT_CODE,
            name: NAME_CLIENT.to_string(),
            version: CARGO_PKG_VERSION.to_string(),
            commit: VERGEN_GIT_SHA.to_string(),
        };
        Ok(EngineApi::new(
            ctx.node.provider().clone(),
            ctx.config.chain.clone(),
            ctx.beacon_engine_handle.clone(),
            PayloadStore::new(ctx.node.payload_builder_handle().clone()),
            ctx.node.pool().clone(),
            Box::new(ctx.node.task_executor().clone()),
            client,
            EngineCapabilities::default(),
            engine_validator,
            ctx.config.engine.accept_execution_requests_hash,
        ))
    }
}

/// A noop Builder that satisfies the [`EngineApiBuilder`] trait without actually configuring an
/// engine API module
///
/// This is intended to be used as a workaround for reusing all the existing ethereum node launch
/// utilities which require an engine API.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct NoopEngineApiBuilder;

impl<N: FullNodeComponents> EngineApiBuilder<N> for NoopEngineApiBuilder {
    type EngineApi = NoopEngineApi;

    async fn build_engine_api(self, _ctx: &AddOnsContext<'_, N>) -> eyre::Result<Self::EngineApi> {
        Ok(NoopEngineApi::default())
    }
}

/// Represents an empty Engine API [`RpcModule`].
///
/// This is only intended to be used in combination with the [`NoopEngineApiBuilder`] in order to
/// satisfy trait bounds in the regular ethereum launch routine that mandate an engine API instance.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct NoopEngineApi;

impl IntoEngineApiRpcModule for NoopEngineApi {
    fn into_rpc_module(self) -> RpcModule<()> {
        RpcModule::new(())
    }
}
