use crate::{components::FullNodeComponents, node::FullNode};

/// A Handle to the launched node.
#[derive(Debug)]
pub struct NodeHandle<Node: FullNodeComponents> {
    /// All node components.
    node: FullNode<Node>,
    node_exit_future: (),
}
