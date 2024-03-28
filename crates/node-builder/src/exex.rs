#![allow(dead_code)]
// todo: expand this (examples, assumptions, invariants)
//! Execution extensions (ExEx).
//!
//! An execution extension is a task that derives its state from Reth's state.
//!
//! Some examples of state such state derives are rollups, bridges, and indexers.
//!
//! An ExEx is a [`Future`] resolving to a `Result<()>` that is run indefinitely alongside Reth.
//!
//! ExEx's are initialized using an async closure that resolves to the ExEx; this closure gets
//! passed an [`ExExContext`] where it is possible to spawn additional tasks and modify Reth.
//!
//! Most ExEx's will want to derive their state from the [`CanonStateNotification`] channel given in
//! [`ExExContext`]. A new notification is emitted whenever blocks are executed in live and
//! historical sync.
//!
//! # Pruning
//!
//! ExEx's **SHOULD** emit an `ExExEvent::FinishedHeight` event to signify what blocks have been
//! processed. This event is used by Reth to determine what state can be pruned.
//!
//! An ExEx will not receive notifications for blocks less than the block emitted in the event. To
//! clarify: if the ExEx emits `ExExEvent::FinishedHeight(0)` it will receive notifications for any
//! `block_number >= 0`.
//!
//! [`Future`]: std::future::Future
//! [`ExExContext`]: crate::exex::ExExContext
//! [`CanonStateNotification`]: reth_provider::CanonStateNotification

use crate::FullNodeTypes;
use futures::{future::BoxFuture, FutureExt};
use reth_node_core::{
    dirs::{ChainPath, DataDirPath},
    node_config::NodeConfig,
};
use reth_primitives::Head;
use reth_tasks::TaskExecutor;
use std::future::Future;

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

/// A trait for launching an ExEx.
trait LaunchExEx<Node: FullNodeTypes>: Send {
    /// Launches the ExEx.
    ///
    /// The ExEx should be able to run independently and emit events on the channels provided in
    /// the [`ExExContext`].
    fn launch(
        self,
        ctx: ExExContext<Node>,
    ) -> impl Future<Output = eyre::Result<impl Future<Output = eyre::Result<()>> + Send>> + Send;
}

type BoxExEx = BoxFuture<'static, eyre::Result<()>>;

/// A version of [LaunchExEx] that returns a boxed future. Makes the trait object-safe.
pub(crate) trait BoxedLaunchExEx<Node: FullNodeTypes>: Send {
    fn launch(self: Box<Self>, ctx: ExExContext<Node>)
        -> BoxFuture<'static, eyre::Result<BoxExEx>>;
}

/// Implements [BoxedLaunchExEx] for any [LaunchExEx] that is [Send] and `'static`.
///
/// Returns a [BoxFuture] that resolves to a [BoxExEx].
impl<E, Node> BoxedLaunchExEx<Node> for E
where
    E: LaunchExEx<Node> + Send + 'static,
    Node: FullNodeTypes,
{
    fn launch(
        self: Box<Self>,
        ctx: ExExContext<Node>,
    ) -> BoxFuture<'static, eyre::Result<BoxExEx>> {
        async move {
            let exex = LaunchExEx::launch(*self, ctx).await?;
            Ok(Box::pin(exex) as BoxExEx)
        }
        .boxed()
    }
}

/// Implements `LaunchExEx` for any closure that takes an [ExExContext] and returns a future
/// resolving to an ExEx.
impl<Node, F, Fut, E> LaunchExEx<Node> for F
where
    Node: FullNodeTypes,
    F: FnOnce(ExExContext<Node>) -> Fut + Send,
    Fut: Future<Output = eyre::Result<E>> + Send,
    E: Future<Output = eyre::Result<()>> + Send,
{
    fn launch(
        self,
        ctx: ExExContext<Node>,
    ) -> impl Future<Output = eyre::Result<impl Future<Output = eyre::Result<()>> + Send>> + Send
    {
        self(ctx)
    }
}
