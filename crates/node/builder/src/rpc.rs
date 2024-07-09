//! Builder support for rpc components.

use std::{fmt, sync::Arc};

use derive_more::{Deref, DerefMut};
use futures::{Future, TryFutureExt};
use reth_beacon_consensus::FullBlockchainTreeEngine;
use reth_consensus_debug_client::{DebugConsensusClient, EtherscanBlockProvider, RpcBlockProvider};
use reth_network::{FullClient, NetworkHandle};
use reth_node_api::{EngineComponent, FullNodeComponents, FullNodeComponentsExt, RpcComponent};
use reth_node_core::{
    node_config::NodeConfig,
    rpc::api::EngineApiServer,
    version::{CARGO_PKG_VERSION, CLIENT_CODE, NAME_CLIENT, VERGEN_GIT_SHA},
};
use reth_payload_builder::PayloadBuilderHandle;
use reth_rpc::eth::EthApi;
use reth_rpc_builder::{
    auth::{AuthRpcModule, AuthServerHandle},
    config::RethRpcServerConfig,
    EthApiBuild, RpcModuleBuilder, RpcRegistryInner, RpcServerHandle, TransportRpcModules,
};
use reth_rpc_engine_api::EngineApi;
use reth_rpc_layer::JwtSecret;
use reth_rpc_types::engine::ClientVersionV1;
use reth_tasks::TaskExecutor;
use reth_tracing::tracing::{debug, info};
use tokio::sync::oneshot;

use crate::{
    common::{ExtBuilderContext, InitializedComponents},
    hooks::OnComponentsInitializedHook,
    EngineAdapter, PipelineAdapter,
};

#[allow(missing_debug_implementations)]
#[derive(Clone)]
pub struct RpcAdapter<N: FullNodeComponents> {
    server_handles: RethRpcServerHandles,
    registry: RpcRegistry<N>,
}

impl<N: FullNodeComponents> RpcComponent<N> for RpcAdapter<N> {
    type ServerHandles = RethRpcServerHandles;
    type Registry = RpcRegistry<N>;

    fn handles(&self) -> &Self::ServerHandles {
        &self.server_handles
    }

    fn registry(&self) -> &Self::Registry {
        &self.registry
    }
}

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
    /// Creates a new, empty [`RpcHooks`] instance for the given node type.
    pub(crate) fn new() -> Self {
        Self { on_rpc_started: Box::<()>::default(), extend_rpc_modules: Box::<()>::default() }
    }

    /// Sets the hook that is run once the rpc server is started.
    pub(crate) fn set_on_rpc_started(&mut self, hook: Box<dyn OnRpcStarted<Node>>) -> &mut Self {
        self.on_rpc_started = hook;
        self
    }

    /// Sets the hook that is run once the rpc server is started.
    #[allow(unused)]
    pub(crate) fn on_rpc_started(mut self, hook: Box<dyn OnRpcStarted<Node>>) -> Self {
        self.set_on_rpc_started(hook);
        self
    }

    /// Sets the hook that is run to configure the rpc modules.
    pub(crate) fn set_extend_rpc_modules(
        &mut self,
        hook: Box<dyn ExtendRpcModules<Node>>,
    ) -> &mut Self {
        self.extend_rpc_modules = hook;
        self
    }

    /// Sets the hook that is run to configure the rpc modules.
    #[allow(unused)]
    pub(crate) fn extend_rpc_modules(mut self, hook: Box<dyn ExtendRpcModules<Node>>) -> Self {
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
pub trait OnRpcStarted<Node: FullNodeComponents>: Send {
    /// The hook that is called once the rpc server is started.
    fn on_rpc_started(
        self: Box<Self>,
        ctx: RpcContext<'_, Node>,
        handles: RethRpcServerHandles,
    ) -> eyre::Result<()>;
}

impl<Node, F> OnRpcStarted<Node> for F
where
    F: FnOnce(RpcContext<'_, Node>, RethRpcServerHandles) -> eyre::Result<()> + Send,
    Node: FullNodeComponents,
{
    fn on_rpc_started(
        self: Box<Self>,
        ctx: RpcContext<'_, Node>,
        handles: RethRpcServerHandles,
    ) -> eyre::Result<()> {
        (*self)(ctx, handles)
    }
}

impl<Node: FullNodeComponents> OnRpcStarted<Node> for () {
    fn on_rpc_started(
        self: Box<Self>,
        _: RpcContext<'_, Node>,
        _: RethRpcServerHandles,
    ) -> eyre::Result<()> {
        Ok(())
    }
}

/// Event hook that is called when the rpc server is started.
pub trait ExtendRpcModules<Node: FullNodeComponents>: Send {
    /// The hook that is called once the rpc server is started.
    fn extend_rpc_modules(self: Box<Self>, ctx: RpcContext<'_, Node>) -> eyre::Result<()>;
}

impl<Node, F> ExtendRpcModules<Node> for F
where
    F: FnOnce(RpcContext<'_, Node>) -> eyre::Result<()> + Send,
    Node: FullNodeComponents,
{
    fn extend_rpc_modules(self: Box<Self>, ctx: RpcContext<'_, Node>) -> eyre::Result<()> {
        (*self)(ctx)
    }
}

impl<Node: FullNodeComponents> ExtendRpcModules<Node> for () {
    fn extend_rpc_modules(self: Box<Self>, _: RpcContext<'_, Node>) -> eyre::Result<()> {
        Ok(())
    }
}

/// Helper wrapper type to encapsulate the [`RpcRegistryInner`] over components trait.
#[derive(Deref, DerefMut, Clone)]
#[allow(clippy::type_complexity)]
#[allow(missing_debug_implementations)]
pub struct RpcRegistry<Node: FullNodeComponents> {
    pub(crate) registry: RpcRegistryInner<
        Node::Provider,
        Node::Pool,
        NetworkHandle,
        TaskExecutor,
        Node::Provider,
        EthApi<Node::Provider, Node::Pool, NetworkHandle, Node::Evm>,
    >,
}

/// Helper container to encapsulate [`RpcRegistryInner`], [`TransportRpcModules`] and
/// [`AuthRpcModule`].
///
/// This can be used to access installed modules, or create commonly used handlers like
/// [`reth_rpc::eth::EthApi`], and ultimately merge additional rpc handler into the configured
/// transport modules [`TransportRpcModules`] as well as configured authenticated methods
/// [`AuthRpcModule`].
#[allow(missing_debug_implementations)]
pub struct RpcContext<'a, Node: FullNodeComponents> {
    /// The node components.
    pub(crate) node: Node,

    /// Gives access to the node configuration.
    pub(crate) config: &'a NodeConfig,

    /// A Helper type the holds instances of the configured modules.
    ///
    /// This provides easy access to rpc handlers, such as [`RpcRegistryInner::eth_api`].
    pub registry: &'a mut RpcRegistry<Node>,
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

impl<'a, Node: FullNodeComponents> RpcContext<'a, Node> {
    /// Returns the config of the node.
    pub const fn config(&self) -> &NodeConfig {
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
    pub fn network(&self) -> &NetworkHandle {
        self.node.network()
    }

    /// Returns the handle to the payload builder service
    pub fn payload_builder(&self) -> &PayloadBuilderHandle<Node::EngineTypes> {
        self.node.payload_builder()
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
    Node: FullNodeComponents,
    Engine: EngineApiServer<Node::EngineTypes>,
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
        .with_evm_config(node.evm_config().clone())
        .build_with_auth_server(module_config, engine_api, EthApiBuild::build);

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
        node,
        config,
        registry: &mut registry,
        modules: &mut modules,
        auth_module: &mut auth_module,
    };

    on_rpc_started.on_rpc_started(ctx, handles.clone())?;

    Ok((handles, registry))
}

/// Builds optional RPC component.
pub trait RpcBuilder<Core, BT, C, Node>: Send
where
    Core: FullNodeComponents,
    BT: FullBlockchainTreeEngine + Clone,
    C: FullClient + Clone,
    Node: FullNodeComponentsExt<
        EngineTypes = Core::EngineTypes,
        Core = Core,
        Pipeline = PipelineAdapter<C>,
        Tree = BT,
        Engine = EngineAdapter<Core, BT, C>,
        Rpc = RpcAdapter<Core>,
    >,
{
    /// Installs RPC on the node.
    fn build_rpc(
        self: Box<Self>,
        ctx: ExtBuilderContext<'_, Node>,
    ) -> impl Future<Output = eyre::Result<()>> {
        async move {
            let client = ClientVersionV1 {
                code: CLIENT_CODE,
                name: NAME_CLIENT.to_string(),
                version: CARGO_PKG_VERSION.to_string(),
                commit: VERGEN_GIT_SHA.to_string(),
            };
            let engine_api = EngineApi::new(
                ctx.blockchain_db().clone(),
                ctx.chain_spec(),
                ctx.right()
                    .engine()
                    .expect("engine should be built before rpc") // todo: fix engine deps with impl of engine builder
                    .handle()
                    .clone(),
                ctx.node().payload_builder().clone().into(),
                Box::new(ctx.node().task_executor().clone()),
                client,
            );
            info!(target: "reth::cli", "Engine API handler initialized");

            // extract the jwt secret from the args if possible
            let jwt_secret = ctx.auth_jwt_secret()?;

            // Start RPC servers
            let (server_handles, registry) = crate::rpc::launch_rpc_servers(
                ctx.node().clone().into_core(),
                engine_api,
                ctx.node_config(),
                jwt_secret,
                ctx.right().rpc_add_ons_mut().take().unwrap_or(RpcHooks::new()),
            )
            .await?;

            // in dev mode we generate 20 random dev-signer accounts
            if ctx.is_dev() {
                registry.eth_api().with_dev_accounts();
            }

            // Run consensus engine to completion
            // TODO: can safely be spawned in NodeComponentsBuilderExt::build, i.e. after executing
            // code below?
            let (tx, rx) = oneshot::channel();
            ctx.right()
                .engine_mut()
                .map(|engine| engine.shutdown_rx_mut().set(rx).expect("should not be set yet"));
            info!(target: "reth::cli", "Starting consensus engine");
            ctx.task_executor().spawn_critical_blocking("consensus engine", async move {
                let res = ctx
                    .right()
                    .engine()
                    .expect("engine should be built before rpc") // todo: fix engine deps with impl of engine builder
                    .engine()
                    .take()
                    .expect("engine should not be spawned yet")
                    .into_inner()
                    .await;
                let _ = tx.send(res);
            });

            if let Some(maybe_custom_etherscan_url) = ctx.node_config().debug.etherscan.clone() {
                info!(target: "reth::cli", "Using etherscan as consensus client");

                let chain = ctx.node_config().chain.chain;
                let etherscan_url = maybe_custom_etherscan_url.map(Ok).unwrap_or_else(|| {
                    // If URL isn't provided, use default Etherscan URL for the chain if it is known
                    chain.etherscan_urls().map(|urls| urls.0.to_string()).ok_or_else(|| {
                        eyre::eyre!("failed to get etherscan url for chain: {chain}")
                    })
                })?;

                let block_provider = EtherscanBlockProvider::new(
                    etherscan_url,
                    chain.etherscan_api_key().ok_or_else(|| {
                        eyre::eyre!(
                        "etherscan api key not found for rpc consensus client for chain: {chain}"
                    )
                    })?,
                );
                let rpc_consensus_client = DebugConsensusClient::new(
                    server_handles.auth.clone(),
                    Arc::new(block_provider),
                );
                ctx.task_executor().spawn_critical("etherscan consensus client", async move {
                    rpc_consensus_client.run::<Node::EngineTypes>().await
                });
            }

            if let Some(rpc_ws_url) = ctx.node_config().debug.rpc_consensus_ws.clone() {
                info!(target: "reth::cli", "Using rpc provider as consensus client");

                let block_provider = RpcBlockProvider::new(rpc_ws_url);
                let rpc_consensus_client = DebugConsensusClient::new(
                    server_handles.auth.clone(),
                    Arc::new(block_provider),
                );
                ctx.node().task_executor().spawn_critical("rpc consensus client", async move {
                    rpc_consensus_client.run::<Node::EngineTypes>().await
                });
            }

            ctx.right().rpc_mut().map(|rpc| *rpc = RpcAdapter { server_handles, registry });

            Ok(())
        }
    }
}

impl<Core, BT, C, Node, F> RpcBuilder<Core, BT, C, Node> for F
where
    Core: FullNodeComponents,
    BT: FullBlockchainTreeEngine + Clone,
    C: FullClient + Clone,
    Node: FullNodeComponentsExt<
        EngineTypes = Core::EngineTypes,
        Core = Core,
        Pipeline = PipelineAdapter<C>,
        Tree = BT,
        Engine = EngineAdapter<Core, BT, C>,
        Rpc = RpcAdapter<Core>,
    >,
    F: OnComponentsInitializedHook<Node> + Send,
    Node::Rpc: RpcComponent<Node>,
{
    fn build_rpc(
        self: Box<Self>,
        ctx: ExtBuilderContext<'_, Node>,
    ) -> impl Future<Output = eyre::Result<()>> {
        self.on_event(ctx)
    }
}

pub trait RpcServerHandles {
    fn rpc(&self) -> &RpcServerHandle;
    fn auth(&self) -> &AuthServerHandle;
}
