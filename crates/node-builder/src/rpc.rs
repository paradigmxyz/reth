//! Builder support for rpc components.

use crate::components::FullNodeComponents;
use futures::TryFutureExt;
use reth_network::NetworkHandle;
use reth_node_core::{
    cli::config::RethRpcConfig,
    node_config::NodeConfig,
    rpc::{
        api::EngineApiServer,
        builder::{
            auth::{AuthRpcModule, AuthServerHandle},
            RethModuleRegistry, RpcModuleBuilder, RpcServerHandle, TransportRpcModules,
        },
    },
};
use reth_rpc::JwtSecret;
use reth_tasks::TaskExecutor;
use reth_tracing::tracing::{debug, info};
use std::{
    fmt,
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
pub(crate) struct RpcHooks<Node: FullNodeComponents> {
    pub(crate) on_rpc_started: Box<dyn OnRpcStarted<Node>>,
    pub(crate) extend_rpc_modules: Box<dyn ExtendRpcModules<Node>>,
}

impl<Node: FullNodeComponents> RpcHooks<Node> {
    /// Creates a new, empty [RpcHooks] instance for the given node type.
    pub(crate) fn new() -> Self {
        Self { on_rpc_started: Box::<()>::default(), extend_rpc_modules: Box::<()>::default() }
    }

    /// Sets the hook that is run once the rpc server is started.
    pub(crate) fn set_on_rpc_started<F>(&mut self, hook: F) -> &mut Self
    where
        F: OnRpcStarted<Node> + 'static,
    {
        self.on_rpc_started = Box::new(hook);
        self
    }

    /// Sets the hook that is run once the rpc server is started.
    #[allow(unused)]
    pub(crate) fn on_rpc_started<F>(mut self, hook: F) -> Self
    where
        F: OnRpcStarted<Node> + 'static,
    {
        self.set_on_rpc_started(hook);
        self
    }

    /// Sets the hook that is run to configure the rpc modules.
    pub(crate) fn set_extend_rpc_modules<F>(&mut self, hook: F) -> &mut Self
    where
        F: ExtendRpcModules<Node> + 'static,
    {
        self.extend_rpc_modules = Box::new(hook);
        self
    }

    /// Sets the hook that is run to configure the rpc modules.
    #[allow(unused)]
    pub(crate) fn extend_rpc_modules<F>(mut self, hook: F) -> Self
    where
        F: ExtendRpcModules<Node> + 'static,
    {
        self.set_extend_rpc_modules(hook);
        self
    }
}

impl<Node: FullNodeComponents> fmt::Debug for RpcHooks<Node> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RpcHooks")
            .field("on_rpc_started", &"...")
            .field("extend_rpc_modules", &"...")
            .finish()
    }
}

/// Event hook that is called once the rpc server is started.
pub trait OnRpcStarted<Node: FullNodeComponents> {
    /// The hook that is called once the rpc server is started.
    fn on_rpc_started(
        &self,
        ctx: RpcContext<'_, Node>,
        handles: RethRpcServerHandles,
    ) -> eyre::Result<()>;
}

impl<Node, F> OnRpcStarted<Node> for F
where
    F: Fn(RpcContext<'_, Node>, RethRpcServerHandles) -> eyre::Result<()>,
    Node: FullNodeComponents,
{
    fn on_rpc_started(
        &self,
        ctx: RpcContext<'_, Node>,
        handles: RethRpcServerHandles,
    ) -> eyre::Result<()> {
        self(ctx, handles)
    }
}

impl<Node: FullNodeComponents> OnRpcStarted<Node> for () {
    fn on_rpc_started(&self, _: RpcContext<'_, Node>, _: RethRpcServerHandles) -> eyre::Result<()> {
        Ok(())
    }
}

/// Event hook that is called when the rpc server is started.
pub trait ExtendRpcModules<Node: FullNodeComponents> {
    /// The hook that is called once the rpc server is started.
    fn extend_rpc_modules(&self, ctx: RpcContext<'_, Node>) -> eyre::Result<()>;
}

impl<Node, F> ExtendRpcModules<Node> for F
where
    F: Fn(RpcContext<'_, Node>) -> eyre::Result<()>,
    Node: FullNodeComponents,
{
    fn extend_rpc_modules(&self, ctx: RpcContext<'_, Node>) -> eyre::Result<()> {
        self(ctx)
    }
}

impl<Node: FullNodeComponents> ExtendRpcModules<Node> for () {
    fn extend_rpc_modules(&self, _: RpcContext<'_, Node>) -> eyre::Result<()> {
        Ok(())
    }
}

/// Helper wrapper type to encapsulate the [RethModuleRegistry] over components trait.
#[derive(Debug)]
pub struct RpcRegistry<Node: FullNodeComponents> {
    pub(crate) registry: RethModuleRegistry<
        Node::Provider,
        Node::Pool,
        NetworkHandle,
        TaskExecutor,
        Node::Provider,
        Node::Evm,
    >,
}

impl<Node: FullNodeComponents> Deref for RpcRegistry<Node> {
    type Target = RethModuleRegistry<
        Node::Provider,
        Node::Pool,
        NetworkHandle,
        TaskExecutor,
        Node::Provider,
        Node::Evm,
    >;

    fn deref(&self) -> &Self::Target {
        &self.registry
    }
}

impl<Node: FullNodeComponents> DerefMut for RpcRegistry<Node> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.registry
    }
}

impl<Node: FullNodeComponents> Clone for RpcRegistry<Node> {
    fn clone(&self) -> Self {
        Self { registry: self.registry.clone() }
    }
}

/// Helper container to encapsulate [RethModuleRegistry], [TransportRpcModules] and [AuthRpcModule].
///
/// This can be used to access installed modules, or create commonly used handlers like
/// [reth_rpc::EthApi], and ultimately merge additional rpc handler into the configured transport
/// modules [TransportRpcModules] as well as configured authenticated methods [AuthRpcModule].
#[allow(missing_debug_implementations)]
pub struct RpcContext<'a, Node: FullNodeComponents> {
    /// The node components.
    pub(crate) node: Node,

    /// Gives access to the node configuration.
    pub(crate) config: &'a NodeConfig,

    /// A Helper type the holds instances of the configured modules.
    ///
    /// This provides easy access to rpc handlers, such as [RethModuleRegistry::eth_api].
    pub registry: &'a mut RpcRegistry<Node>,
    /// Holds installed modules per transport type.
    ///
    /// This can be used to merge additional modules into the configured transports (http, ipc,
    /// ws). See [TransportRpcModules::merge_configured]
    pub modules: &'a mut TransportRpcModules,
    /// Holds jwt authenticated rpc module.
    ///
    /// This can be used to merge additional modules into the configured authenticated methods
    pub auth_module: &'a mut AuthRpcModule,
}

impl<'a, Node: FullNodeComponents> RpcContext<'a, Node> {
    /// Returns the config of the node.
    pub fn config(&self) -> &NodeConfig {
        self.config
    }

    /// Returns a reference to the configured node.
    pub fn node(&self) -> &Node {
        &self.node
    }
}

/// Launch the rpc servers.
pub(crate) async fn launch_rpc_servers<Node, Engine>(
    node: Node,
    engine_api: Engine,
    config: &NodeConfig,
    jwt_secret: JwtSecret,
    hooks: RpcHooks<Node>,
) -> eyre::Result<(RethRpcServerHandles, RpcRegistry<Node>)>
where
    Node: FullNodeComponents + Clone,
    Engine: EngineApiServer<Node::Engine>,
{
    let RpcHooks { on_rpc_started, extend_rpc_modules } = hooks;

    let auth_config = config.rpc.auth_server_config(jwt_secret)?;
    let module_config = config.rpc.transport_rpc_module_config();
    debug!(target: "reth::cli", http=?module_config.http(), ws=?module_config.ws(), "Using RPC module config");

    let (mut modules, mut auth_module, registry) = RpcModuleBuilder::default()
        .with_provider(node.provider().clone())
        .with_pool(node.pool().clone())
        .with_network(node.network().clone())
        .with_events(node.provider().clone())
        .with_executor(node.task_executor().clone())
        .with_evm_config(node.evm_config())
        .build_with_auth_server(module_config, engine_api);

    let mut registry = RpcRegistry { registry };
    let ctx = RpcContext {
        node: node.clone(),
        config,
        registry: &mut registry,
        modules: &mut modules,
        auth_module: &mut auth_module,
    };

    extend_rpc_modules.extend_rpc_modules(ctx)?;

    let server_config = config.rpc.rpc_server_config();
    let launch_rpc = modules.clone().start_server(server_config).map_ok(|handle| {
        if let Some(url) = handle.ipc_endpoint() {
            info!(target: "reth::cli", url=%url, "RPC IPC server started");
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
        info!(target: "reth::cli", url=%addr, "RPC auth server started");
        handle
    });

    // launch servers concurrently
    let (rpc, auth) = futures::future::try_join(launch_rpc, launch_auth).await?;
    let handles = RethRpcServerHandles { rpc, auth };

    let ctx = RpcContext {
        node,
        config,
        registry: &mut registry,
        modules: &mut modules,
        auth_module: &mut auth_module,
    };

    on_rpc_started.on_rpc_started(ctx, handles.clone())?;

    Ok((handles, registry))
}
