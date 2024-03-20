use crate::{BuilderContext, FullNodeTypes};
use futures::{future::BoxFuture, Future, FutureExt, Stream};
use reth_primitives::BlockNumber;
use std::pin::Pin;

/// The ExEx (Execution Extension) component.
pub trait ExEx: Stream<Item = ExExEvent> + Send + 'static {}

/// Events emitted by the ExEx.
pub enum ExExEvent {
    /// Highest block processed by the ExEx.
    ///
    /// ExEx should guarantee that it will not require all earlier blocks in the future, meaning
    /// that Reth is allowed to prune them.
    ///
    /// On reorgs, it's possible for the height to go down.
    FinishedHeight(BlockNumber),
}

/// A trait for launching an ExEx.
pub trait LaunchExEx<Node: FullNodeTypes>: Send {
    fn launch(
        self,
        ctx: &BuilderContext<Node>,
    ) -> impl Future<Output = eyre::Result<impl ExEx>> + Send;
}

pub(crate) type BoxExEx = Pin<Box<dyn ExEx<Item = ExExEvent> + Send + 'static>>;

/// A version of `LaunchExEx` that returns a boxed future. Makes the trait object-safe.
pub(crate) trait BoxedLaunchExEx<Node: FullNodeTypes>: Send {
    fn launch<'a>(
        self: Box<Self>,
        ctx: &'a BuilderContext<Node>,
    ) -> BoxFuture<'a, eyre::Result<BoxExEx>>;
}

/// Implements `BoxedLaunchExEx` for any `LaunchExEx` that is `Send` and `'static`.
impl<E, Node> BoxedLaunchExEx<Node> for E
where
    E: LaunchExEx<Node> + Send + 'static,
    Node: FullNodeTypes,
{
    fn launch<'a>(
        self: Box<Self>,
        ctx: &'a BuilderContext<Node>,
    ) -> BoxFuture<'a, eyre::Result<BoxExEx>> {
        async move {
            let exex = self.launch(ctx).await?;
            Ok(exex)
        }
        .boxed()
    }
}

/// Implements `LaunchExEx` for any closure that takes a `BuilderContext` and returns a future.
impl<Node, F, Fut, E> LaunchExEx<Node> for F
where
    Node: FullNodeTypes,
    F: FnOnce(&BuilderContext<Node>) -> Fut + Send,
    Fut: Future<Output = eyre::Result<E>> + Send,
    E: ExEx,
{
    fn launch(
        self,
        ctx: &BuilderContext<Node>,
    ) -> impl Future<Output = eyre::Result<impl ExEx>> + Send {
        self(ctx)
    }
}
