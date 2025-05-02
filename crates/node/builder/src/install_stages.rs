//! A trait for installing custom stages into a stage set.

use std::future::Future;

use futures::{future::BoxFuture, FutureExt};
use reth_provider::providers::ProviderNodeTypes;
use reth_stages::StageSetBuilder;

/// A trait for installing custom stages into a stage set.
pub trait InstallStages<Node: ProviderNodeTypes>: Send {
    /// Installs custom stages into the stage set builder.
    ///
    /// This allows modifying the stage pipeline by adding, removing, or modifying stages.
    fn on_stages(
        self,
        stages: StageSetBuilder<Node>,
    ) -> impl Future<Output = eyre::Result<impl Future<Output = eyre::Result<()>> + Send>> + Send;
}

/// A boxed install stages.
pub type BoxInstallStages = BoxFuture<'static, eyre::Result<()>>;

/// A boxed version of [`InstallStages`] that returns a boxed future. Makes the trait object-safe.
pub trait BoxedInstallStages<Node: ProviderNodeTypes>: Send {
    /// Installs custom stages into the stage set builder with a boxed future.
    fn on_stages(
        self: Box<Self>,
        stages: StageSetBuilder<Node>,
    ) -> BoxFuture<'static, eyre::Result<BoxInstallStages>>;
}

/// Implements [`BoxedInstallStages`] for any [`InstallStages`] that is [Send] and `'static`.
///
/// Returns a [`BoxFuture`] that resolves to a [`BoxInstallStages`].
impl<E, Node> BoxedInstallStages<Node> for E
where
    E: InstallStages<Node> + Send + 'static,
    Node: ProviderNodeTypes + 'static,
{
    fn on_stages(
        self: Box<Self>,
        stages: StageSetBuilder<Node>,
    ) -> BoxFuture<'static, eyre::Result<BoxInstallStages>> {
        async move {
            let stage = InstallStages::on_stages(*self, stages).await?;
            Ok(Box::pin(stage) as BoxInstallStages)
        }
        .boxed()
    }
}

/// Implements `InstallStages` for closures that take a stage set builder
impl<Node, F, Fut, E> InstallStages<Node> for F
where
    Node: ProviderNodeTypes,
    F: FnOnce(StageSetBuilder<Node>) -> Fut + Send,
    Fut: Future<Output = eyre::Result<E>> + Send,
    E: Future<Output = eyre::Result<()>> + Send,
{
    fn on_stages(
        self,
        stages: StageSetBuilder<Node>,
    ) -> impl Future<Output = eyre::Result<impl Future<Output = eyre::Result<()>> + Send>> + Send
    {
        self(stages)
    }
}
