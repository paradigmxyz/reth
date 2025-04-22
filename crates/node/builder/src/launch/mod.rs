//! Abstraction for launching a node.

pub mod common;
mod exex;

pub(crate) mod debug;
pub(crate) mod engine;

pub use common::LaunchContext;
pub use exex::ExExLauncher;

use std::future::Future;

/// A general purpose trait that launches a new node of any kind.
///
/// Acts as a node factory that targets a certain node configuration and returns a handle to the
/// node.
///
/// This is essentially the launch logic for a node.
///
/// See also [`EngineNodeLauncher`](crate::EngineNodeLauncher) and
/// [`NodeBuilderWithComponents::launch_with`](crate::NodeBuilderWithComponents)
pub trait LaunchNode<Target> {
    /// The node type that is created.
    type Node;

    /// Create and return a new node asynchronously.
    fn launch_node(self, target: Target) -> impl Future<Output = eyre::Result<Self::Node>>;
}

impl<F, Target, Fut, Node> LaunchNode<Target> for F
where
    F: FnOnce(Target) -> Fut + Send,
    Fut: Future<Output = eyre::Result<Node>> + Send,
{
    type Node = Node;

    fn launch_node(self, target: Target) -> impl Future<Output = eyre::Result<Self::Node>> {
        self(target)
    }
}
