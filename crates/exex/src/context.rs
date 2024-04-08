use reth_node_api::FullNodeTypes;
use reth_node_core::{
    dirs::{ChainPath, DataDirPath},
    node_config::NodeConfig,
};
use reth_primitives::Head;
use reth_provider::CanonStateNotification;
use reth_tasks::TaskExecutor;
use tokio::sync::mpsc::{Receiver, Sender};

use crate::ExExEvent;

/// Captures the context that an ExEx has access to.
#[derive(Debug)]
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
    /// Channel used to send [`ExExEvent`]s to the rest of the node.
    ///
    /// # Important
    ///
    /// The node expects at least one `FinishedHeight` event in order to prune blocks.
    pub events: Sender<ExExEvent>,
    /// Channel to receive [`CanonStateNotification`]s on state transitions.
    pub notifications: Receiver<CanonStateNotification>,
    // TODO(alexey): add pool, payload builder, anything else?
}
