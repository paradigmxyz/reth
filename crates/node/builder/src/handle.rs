use std::fmt;

use reth_node_api::{FullNodeComponents, NodeAddOns};
use reth_node_core::exit::NodeExitFuture;

use crate::node::FullNode;

/// A Handle to the launched node.
#[must_use = "Needs to await the node exit future"]
pub struct NodeHandle<Node: FullNodeComponents, AddOns: NodeAddOns<Node>> {
    /// All node components.
    pub node: FullNode<Node, AddOns>,
    /// The exit future of the node.
    pub node_exit_future: NodeExitFuture,
}

impl<Node, AddOns> NodeHandle<Node, AddOns>
where
    Node: FullNodeComponents,
    AddOns: NodeAddOns<Node>,
{
    /// Waits for the node to exit, if it was configured to exit.
    pub async fn wait_for_node_exit(self) -> eyre::Result<()> {
        self.node_exit_future.await
    }
}

impl<Node, AddOns> fmt::Debug for NodeHandle<Node, AddOns>
where
    Node: FullNodeComponents,
    AddOns: NodeAddOns<Node>,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NodeHandle")
            .field("node", &"...")
            .field("node_exit_future", &self.node_exit_future)
            .finish()
    }
}
