use reth_node_api::FullNodeComponents;
use reth_node_core::{
    dirs::{ChainPath, DataDirPath},
    node_config::NodeConfig,
};
use reth_primitives::Head;
use reth_tasks::TaskExecutor;
use tokio::sync::mpsc::{Receiver, UnboundedSender};

use crate::{ExExEvent, ExExNotification};

/// Captures the context that an ExEx has access to.
#[derive(Debug)]
pub struct ExExContext<Node: FullNodeComponents> {
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
    /// The transaction pool of the node.
    pub pool: Node::Pool,
    /// Channel used to send [`ExExEvent`]s to the rest of the node.
    ///
    /// # Important
    ///
    /// The exex should emit a `FinishedHeight` whenever a processed block is safe to prune.
    /// Additionally, the exex can pre-emptively emit a `FinishedHeight` event to specify what
    /// blocks to receive notifications for.
    pub events: UnboundedSender<ExExEvent>,
    /// Channel to receive [`ExExNotification`]s.
    ///
    /// # Important
    ///
    /// Once a an [`ExExNotification`] is sent over the channel, it is considered delivered by the
    /// node.
    pub notifications: Receiver<ExExNotification>,
}
