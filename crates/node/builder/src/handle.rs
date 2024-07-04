use std::fmt;

use derive_more::Deref;
use reth_node_api::FullNodeComponentsExt;
use reth_node_core::exit::NodeExitFuture;

use crate::node::FullNode;

/// A Handle to the launched node.
#[must_use = "Needs to await the node exit future"]
#[derive(Deref)]
pub struct NodeHandle<Node: FullNodeComponentsExt> {
    /// All node components.
    #[deref]
    pub node: FullNode<Node>,
    /// The exit future of the node.
    pub node_exit_future: NodeExitFuture,
}

impl<Node: FullNodeComponentsExt> NodeHandle<Node> {
    /// Waits for the node to exit, if it was configured to exit.
    pub async fn wait_for_node_exit(self) -> eyre::Result<()> {
        self.node_exit_future.await
    }
}

impl<Node: FullNodeComponentsExt> fmt::Debug for NodeHandle<Node> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NodeHandle")
            .field("node", &"...")
            .field("node_exit_future", &self.node_exit_future)
            .finish()
    }
}
