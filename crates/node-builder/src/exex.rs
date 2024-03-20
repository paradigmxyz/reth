use crate::{BuilderContext, FullNodeTypes};
use futures::{future::BoxFuture, Future, FutureExt, Stream};
use reth_primitives::BlockNumber;
use std::pin::Pin;

/// The stream of events emitted by an ExEx (Execution Extension).
pub trait ExEx: Stream<Item = ExExEvent> + Send + 'static {}

/// Events emitted by an ExEx.
#[derive(Debug)]
pub enum ExExEvent {
    /// Highest block processed by the ExEx.
    ///
    /// ExEx should guarantee that it will not require all earlier blocks in the future, meaning
    /// that Reth is allowed to prune them.
    ///
    /// On reorgs, it's possible for the height to go down.
    FinishedHeight(BlockNumber),
}

/// Captures the context that an ExEx has access to.
pub struct ExExContext<Node: FullNodeTypes> {
    builder: BuilderContext<Node>,
    // TODO(alexey): add pool, payload builder, anything else?
}

impl<Node: FullNodeTypes> std::fmt::Debug for ExExContext<Node> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExExContext").field("builder", &self.builder).finish()
    }
}

/// A trait for launching an ExEx.
trait LaunchExEx<Node: FullNodeTypes>: Send {
    /// Launches the ExEx.
    ///
    /// The ExEx should be able to run independently and emit events on the stream.
    fn launch(self, ctx: ExExContext<Node>)
        -> impl Future<Output = eyre::Result<impl ExEx>> + Send;
}

type BoxExEx = Pin<Box<dyn ExEx<Item = ExExEvent> + Send + 'static>>;

/// A boxed version of [LaunchExEx] that returns a boxed future. Makes the trait object-safe.
pub(crate) trait BoxedLaunchExEx<Node: FullNodeTypes>: Send {
    fn launch(self: Box<Self>, ctx: ExExContext<Node>)
        -> BoxFuture<'static, eyre::Result<BoxExEx>>;
}

/// Implements [BoxedLaunchExEx] for any [LaunchExEx] that is [Send] and `'static`.
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
            let exex = self.launch(ctx).await?;
            Ok(exex)
        }
        .boxed()
    }
}

/// Implements `LaunchExEx` for any closure that takes an [ExExContext] and returns a future.
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
