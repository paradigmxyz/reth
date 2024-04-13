//! Types for launching execution extensions (ExEx).
use futures::{future::BoxFuture, FutureExt};
use reth_exex::ExExContext;
use reth_node_api::FullNodeComponents;
use std::future::Future;

/// A trait for launching an ExEx.
trait LaunchExEx<Node: FullNodeComponents>: Send {
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
pub(crate) trait BoxedLaunchExEx<Node: FullNodeComponents>: Send {
    fn launch(self: Box<Self>, ctx: ExExContext<Node>)
        -> BoxFuture<'static, eyre::Result<BoxExEx>>;
}

/// Implements [BoxedLaunchExEx] for any [LaunchExEx] that is [Send] and `'static`.
///
/// Returns a [BoxFuture] that resolves to a [BoxExEx].
impl<E, Node> BoxedLaunchExEx<Node> for E
where
    E: LaunchExEx<Node> + Send + 'static,
    Node: FullNodeComponents,
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
    Node: FullNodeComponents,
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
