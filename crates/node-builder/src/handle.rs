use crate::{components::FullNodeComponents, rpc::RethRpcServerHandles};
use reth_node_core::{
    dirs::{ChainPath, DataDirPath},
    node_config::NodeConfig,
};

/// A Handle to the launched node.
#[derive(Debug)]
pub struct NodeHandle<Node: FullNodeComponents> {
    /// All node components.
    node: Node,
    /// The initial node config.
    config: NodeConfig,
    /// Handles to the RPC servers.
    rpc_handles: RethRpcServerHandles,
    // TODO add rpc registry
    /// The data dir of the node.
    data_dir: ChainPath<DataDirPath>,

    node_exit_future: (),
}
