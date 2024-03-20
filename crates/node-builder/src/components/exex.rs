use crate::{BuilderContext, FullNodeTypes};
use futures::{future::BoxFuture, Future, FutureExt, Stream};
use std::pin::Pin;

pub trait ExEx: Stream<Item = ExExEvent> + Send + 'static {}

pub enum ExExEvent {}

pub trait LaunchExEx<Node: FullNodeTypes>: Send {
    fn launch(
        self,
        ctx: &BuilderContext<Node>,
    ) -> impl Future<Output = eyre::Result<impl ExEx>> + Send;
}

pub(crate) type BoxExEx = Pin<Box<dyn ExEx<Item = ExExEvent> + Send + 'static>>;

pub(crate) trait BoxedLaunchExEx<Node: FullNodeTypes>: Send {
    fn launch<'a>(
        self: Box<Self>,
        ctx: &'a BuilderContext<Node>,
    ) -> BoxFuture<'a, eyre::Result<BoxExEx>>;
}

impl<E, Node: FullNodeTypes> BoxedLaunchExEx<Node> for E
where
    E: LaunchExEx<Node> + Send + 'static,
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
    ) -> impl Future<Output = Result<impl ExEx, eyre::Report>> + Send {
        self(ctx)
    }
}
