#![allow(dead_code)]

use crate::FullNodeTypes;
use futures::{future::BoxFuture, Future, FutureExt, Stream};
use reth_node_core::{
    dirs::{ChainPath, DataDirPath},
    node_config::NodeConfig,
};
use reth_primitives::{BlockNumber, Head};
use reth_tasks::TaskExecutor;
use std::{borrow::Cow, pin::Pin};

/// An ExEx (Execution Extension) that processes new blocks and emits events on a stream.
pub trait ExEx: Stream<Item = ExExEvent> + Send + 'static {
    /// Returns the name of the ExEx. It will appear in logs and metrics.
    fn name(&self) -> Cow<'static, str>;
}

/// Events emitted by an ExEx.
#[derive(Debug)]
pub enum ExExEvent {
    /// Highest block processed by the ExEx.
    ///
    /// ExEx must guarantee that it will not require all earlier blocks in the future, meaning
    /// that Reth is allowed to prune them.
    ///
    /// On reorgs, it's possible for the height to go down.
    FinishedHeight(BlockNumber),
}

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
    /// The ExEx should be able to run independently and emit events on the stream.
    fn launch(self, ctx: ExExContext<Node>)
        -> impl Future<Output = eyre::Result<impl ExEx>> + Send;
}

type BoxExEx = Pin<Box<dyn ExEx + Send + 'static>>;

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
/// resolving to an [ExEx]
impl<Node, F, Fut, E> LaunchExEx<Node> for F
where
    Node: FullNodeTypes,
    F: FnOnce(ExExContext<Node>) -> Fut + Send,
    Fut: Future<Output = eyre::Result<E>> + Send,
    E: ExEx,
{
    fn launch(
        self,
        ctx: ExExContext<Node>,
    ) -> impl Future<Output = eyre::Result<impl ExEx>> + Send {
        self(ctx)
    }
}
