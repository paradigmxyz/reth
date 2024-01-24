//! Builder support for rpc components.

use crate::components::FullNodeComponents;
use reth_network::NetworkHandle;
use reth_node_core::rpc::builder::{auth::AuthRpcModule, RethModuleRegistry, TransportRpcModules};
use reth_tasks::TaskExecutor;

/// Helper container to encapsulate [RethModuleRegistry],[TransportRpcModules] and [AuthRpcModule].
///
/// This can be used to access installed modules, or create commonly used handlers like
/// [reth_rpc::EthApi], and ultimately merge additional rpc handler into the configured transport
/// modules [TransportRpcModules] as well as configured authenticated methods [AuthRpcModule].
pub(crate) struct RethRpcComponents<'a, Node: FullNodeComponents> {
    /// The node components.
    pub(crate) node: Node,
    /// A Helper type the holds instances of the configured modules.
    ///
    /// This provides easy access to rpc handlers, such as [RethModuleRegistry::eth_api].
    // TODO simplify registry trait bounds
    pub(crate) registry:
        &'a mut RethModuleRegistry<Node::Provider, Node::Pool, NetworkHandle, TaskExecutor, ()>,
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
