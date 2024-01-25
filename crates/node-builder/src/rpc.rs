//! Builder support for rpc components.

use crate::{components::FullNodeComponents, hooks::NodeHooks, BuilderContext};
use reth_network::NetworkHandle;
use reth_node_core::rpc::builder::{
    auth::{AuthRpcModule, AuthServerHandle},
    RethModuleRegistry, RpcServerHandle, TransportRpcModules,
};
use reth_tasks::TaskExecutor;
use std::fmt;

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
    on_rpc_started: Box<dyn OnRpcStarted<Node>>,
    extend_rpc_modules: Box<dyn ExtendRpcModules<Node>>,
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
    pub(crate) fn on_rpc_started<F>(mut self, hook: F) -> Self
    where
        F: OnRpcStarted<Node> + 'static,
    {
        self.set_on_rpc_started(hook);
        self
    }

    /// Sets the hook that is run once the rpc server is started.
    pub(crate) fn set_extend_rpc_modules<F>(&mut self, hook: F) -> &mut Self
    where
        F: ExtendRpcModules<Node> + 'static,
    {
        self.extend_rpc_modules = Box::new(hook);
        self
    }

    /// Sets the hook that is run once the rpc server is started.
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
        self,
        ctx: RpcContext<'_, Node>,
        handles: RethRpcServerHandles,
    ) -> eyre::Result<()>;
}

impl<Node, F> OnRpcStarted<Node> for F
where
    F: FnOnce(RpcContext<'_, Node>, RethRpcServerHandles) -> eyre::Result<()>,
    Node: FullNodeComponents,
{
    fn on_rpc_started(
        self,
        ctx: RpcContext<'_, Node>,
        handles: RethRpcServerHandles,
    ) -> eyre::Result<()> {
        self(ctx, handles)
    }
}

impl<Node: FullNodeComponents> OnRpcStarted<Node> for () {
    fn on_rpc_started(self, _: RpcContext<'_, Node>, _: RethRpcServerHandles) -> eyre::Result<()> {
        Ok(())
    }
}

/// Event hook that is called when the rpc server is started.
pub trait ExtendRpcModules<Node: FullNodeComponents> {
    /// The hook that is called once the rpc server is started.
    fn extend_rpc_modules(self, ctx: RpcContext<'_, Node>) -> eyre::Result<()>;
}

impl<Node, F> ExtendRpcModules<Node> for F
where
    F: FnOnce(RpcContext<'_, Node>) -> eyre::Result<()>,
    Node: FullNodeComponents,
{
    fn extend_rpc_modules(self, ctx: RpcContext<'_, Node>) -> eyre::Result<()> {
        self(ctx)
    }
}

impl<Node: FullNodeComponents> ExtendRpcModules<Node> for () {
    fn extend_rpc_modules(self, _: RpcContext<'_, Node>) -> eyre::Result<()> {
        Ok(())
    }
}

/// Helper container to encapsulate [RethModuleRegistry],[TransportRpcModules] and [AuthRpcModule].
///
/// This can be used to access installed modules, or create commonly used handlers like
/// [reth_rpc::EthApi], and ultimately merge additional rpc handler into the configured transport
/// modules [TransportRpcModules] as well as configured authenticated methods [AuthRpcModule].
pub(crate) struct RpcContext<'a, Node: FullNodeComponents> {
    /// The node components.
    pub(crate) node: Node,

    /// The config of the node.
    pub(crate) inner: &'a BuilderContext<Node>,

    /// A Helper type the holds instances of the configured modules.
    ///
    /// This provides easy access to rpc handlers, such as [RethModuleRegistry::eth_api].
    // TODO simplify registry trait bounds
    pub(crate) registry: &'a mut RethModuleRegistry<
        Node::Provider,
        Node::Pool,
        NetworkHandle,
        TaskExecutor,
        Node::Provider,
    >,
    /// Holds installed modules per transport type.
    ///
    /// This can be used to merge additional modules into the configured transports (http, ipc,
    /// ws). See [TransportRpcModules::merge_configured]
    pub(crate) modules: &'a mut TransportRpcModules,
    /// Holds jwt authenticated rpc module.
    ///
    /// This can be used to merge additional modules into the configured authenticated methods
    pub(crate) auth_module: &'a mut AuthRpcModule,
}
