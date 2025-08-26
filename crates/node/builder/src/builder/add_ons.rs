//! Node add-ons. Depend on core [`NodeComponents`](crate::NodeComponents).

use crate::{exex::BoxedLaunchExEx, hooks::NodeHooks, rpc::RpcHooks};
use reth_node_api::{ConsensusEngineEvent, ConsensusEngineHandle, FullNodeComponents, NodeTypes};
use reth_node_core::node_config::NodeConfig;
use reth_rpc::eth::EthApiTypes;
use reth_rpc_layer::JwtSecret;
use reth_tokio_util::EventSender;
use std::future::Future;

/// Additional node extensions.
///
/// At this point we consider all necessary components defined.
pub struct AddOns<Node: FullNodeComponents, AddOns: NodeAddOns<Node>> {
    /// Additional `NodeHooks` that are called at specific points in the node's launch lifecycle.
    pub hooks: NodeHooks<Node, AddOns>,
    /// The `ExExs` (execution extensions) of the node.
    pub exexs: Vec<(String, Box<dyn BoxedLaunchExEx<Node>>)>,
    /// Additional captured addons.
    pub add_ons: AddOns,
}

/// Context passed to [`NodeAddOns::launch_add_ons`],
#[derive(Debug, Clone)]
pub struct AddOnsContext<'a, N: FullNodeComponents> {
    /// Node with all configured components.
    pub node: N,
    /// Node configuration.
    pub config: &'a NodeConfig<<N::Types as NodeTypes>::ChainSpec>,
    /// Handle to the beacon consensus engine.
    pub beacon_engine_handle: ConsensusEngineHandle<<N::Types as NodeTypes>::Payload>,
    /// Notification channel for engine API events
    pub engine_events: EventSender<ConsensusEngineEvent<<N::Types as NodeTypes>::Primitives>>,
    /// JWT secret for the node.
    pub jwt_secret: JwtSecret,
}

/// Customizable node add-on types.
///
/// This trait defines the interface for extending a node with additional functionality beyond
/// the core [`FullNodeComponents`]. It provides a way to launch supplementary services such as
/// RPC servers, monitoring, external integrations, or any custom functionality that builds on
/// top of the core node components.
///
/// ## Purpose
///
/// The `NodeAddOns` trait serves as an extension point in the node builder architecture,
/// allowing developers to:
/// - Define custom services that run alongside the main node
/// - Access all node components and configuration during initialization
/// - Return a handle for managing the launched services (e.g. handle to rpc server)
///
/// ## How it fits into `NodeBuilder`
///
/// In the node builder pattern, add-ons are the final layer that gets applied after all core
/// components are configured and started. The builder flow typically follows:
///
/// 1. Configure [`NodeTypes`] (chain spec, database types, etc.)
/// 2. Build [`FullNodeComponents`] (consensus, networking, transaction pool, etc.)
/// 3. Launch [`NodeAddOns`] with access to all components via [`AddOnsContext`]
///
/// ## Primary Use Case
///
/// The primary use of this trait is to launch RPC servers that provide external API access to
/// the node. For Ethereum nodes, this typically includes two main servers: the regular RPC
/// server (HTTP/WS/IPC) that handles user requests and the authenticated Engine API server
/// that communicates with the consensus layer. The returned handle contains the necessary
/// endpoints and control mechanisms for these servers, allowing the node to serve JSON-RPC
/// requests and participate in consensus. While RPC is the main use case, the trait is
/// intentionally flexible to support other kinds of add-ons such as monitoring, indexing, or
/// custom protocol extensions.
///
/// ## Context Access
///
/// The [`AddOnsContext`] provides access to:
/// - All node components via the `node` field
/// - Node configuration
/// - Engine API handles for consensus layer communication
/// - JWT secrets for authenticated endpoints
///
/// This ensures add-ons can integrate deeply with the node while maintaining clean separation
/// of concerns.
pub trait NodeAddOns<N: FullNodeComponents>: Send {
    /// Handle to add-ons.
    ///
    /// This type is returned by [`launch_add_ons`](Self::launch_add_ons) and represents a
    /// handle to the launched services. It must be `Clone` to allow multiple components to
    /// hold references and should provide methods to interact with the running services.
    ///
    /// For RPC add-ons, this typically includes:
    /// - Server handles to access local addresses and shutdown methods
    /// - RPC module registry for runtime inspection of available methods
    /// - Configured middleware and transport-specific settings
    /// - For Engine API implementations, this also includes handles for consensus layer
    ///   communication
    type Handle: Send + Sync + Clone;

    /// eth API implementation.
    type EthApi: EthApiTypes;

    /// Configures and launches the add-ons.
    ///
    /// This method is called once during node startup after all core components are initialized.
    /// It receives an [`AddOnsContext`] that provides access to:
    ///
    /// - The fully configured node with all its components
    /// - Node configuration for reading settings
    /// - Engine API handles for consensus layer communication
    /// - JWT secrets for setting up authenticated endpoints (if any).
    ///
    /// The implementation should:
    /// 1. Use the context to configure the add-on services
    /// 2. Launch any background tasks using the node's task executor
    /// 3. Return a handle that allows interaction with the launched services
    ///
    /// # Errors
    ///
    /// This method may fail if the add-ons cannot be properly configured or launched,
    /// for example due to port binding issues or invalid configuration.
    fn launch_add_ons(
        self,
        ctx: AddOnsContext<'_, N>,
        hooks: RpcHooks<N, Self::EthApi>,
    ) -> impl Future<Output = eyre::Result<Self::Handle>> + Send;
}
