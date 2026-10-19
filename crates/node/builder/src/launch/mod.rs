//! Abstraction for launching a node.

pub mod common;
mod exex;
pub mod invalid_block_hook;

pub(crate) mod debug;
pub(crate) mod engine;

pub use common::LaunchContext;
pub use exex::ExExLauncher;

use std::future::IntoFuture;

/// A general purpose trait that launches a new node of any kind.
///
/// Acts as a node factory that targets a certain node configuration and returns a handle to the
/// node.
///
/// This is essentially the launch logic for a node.
///
/// See also [`EngineNodeLauncher`](crate::EngineNodeLauncher) and
/// [`NodeBuilderWithComponents::launch_with`](crate::NodeBuilderWithComponents)
pub trait LaunchNode<Target>: Send {
    /// The node type that is created.
    type Node;

    /// The future type that is returned.
    type Future: IntoFuture<Output = eyre::Result<Self::Node>, IntoFuture: Send>;

    /// Create and return a new node asynchronously.
    fn launch_node(self, target: Target) -> Self::Future;
}

impl<F, Target, Fut, Node> LaunchNode<Target> for F
where
    F: FnOnce(Target) -> Fut + Send,
    Fut: IntoFuture<Output = eyre::Result<Node>, IntoFuture: Send> + Send,
{
    type Node = Node;
    type Future = Fut;

    fn launch_node(self, target: Target) -> Self::Future {
        self(target)
    }
}
