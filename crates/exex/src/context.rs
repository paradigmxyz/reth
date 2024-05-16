use reth_node_api::{FullNodeComponents, FullNodeTypes, NodeTypes};
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

    /// node components
    pub components: Node,
}

impl<Node: FullNodeComponents> NodeTypes for ExExContext<Node> {
    type Primitives = Node::Primitives;
    type Engine = Node::Engine;
}

impl<Node: FullNodeComponents> FullNodeTypes for ExExContext<Node> {
    type DB = Node::DB;
    type Provider = Node::Provider;
}

impl<Node: FullNodeComponents> FullNodeComponents for ExExContext<Node> {
    type Pool = Node::Pool;
    type Evm = Node::Evm;
    type Executor = Node::Executor;

    fn pool(&self) -> &Self::Pool {
        self.components.pool()
    }

    fn evm_config(&self) -> &Self::Evm {
        self.components.evm_config()
    }

    fn block_executor(&self) -> &Self::Executor {
        self.components.block_executor()
    }

    fn provider(&self) -> &Self::Provider {
        self.components.provider()
    }

    fn network(&self) -> &reth_network::NetworkHandle {
        self.components.network()
    }

    fn payload_builder(&self) -> &reth_payload_builder::PayloadBuilderHandle<Self::Engine> {
        self.components.payload_builder()
    }

    fn task_executor(&self) -> &TaskExecutor {
        self.components.task_executor()
    }
}
