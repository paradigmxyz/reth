//! Abstraction for launching a node.

use std::future::Future;

/// Launches a new node.
///
/// Acts as a node factory.
///
/// This is essentially the launch logic for a node.
pub trait LaunchNode<Target> {
    /// The node type that is created.
    type Node;

    /// Create and return a new node asynchronously.
    fn launch_node(self, target: Target) -> impl Future<Output = eyre::Result<Self::Node>> + Send;
}
