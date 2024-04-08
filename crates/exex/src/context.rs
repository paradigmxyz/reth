use reth_node_api::FullNodeTypes;
use reth_node_core::{
    dirs::{ChainPath, DataDirPath},
    node_config::NodeConfig,
};
use reth_primitives::Head;
use reth_tasks::TaskExecutor;

/// Captures the context that an ExEx has access to.
#[derive(Clone, Debug)]
pub struct ExExContext<Node: FullNodeTypes> {
    /// The current head of the blockchain at launch.
    pub head: Head,
    /// The configured provider to interact with the blockchain.
    pub provider: Node::Provider,
    /// The task executor of the node.
    pub task_executor: TaskExecutor,
    /// The data dir of the node.
    pub data_dir: ChainPath<DataDirPath>,
    /// The config of the node
    pub config: NodeConfig,
    /// The loaded node config
    pub reth_config: reth_config::Config,
    // TODO(alexey): add pool, payload builder, anything else?
}
