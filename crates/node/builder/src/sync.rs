//! Backfill the engine runs while the node is far behind the chain.

use crate::components::NodeComponents;
use reth_engine_tree::backfill::{BackfillSync, PipelineSync};
use reth_network_api::BlockDownloaderProvider;
use reth_provider::{providers::ProviderNodeTypes, ProviderFactory};
use reth_stages::Pipeline;
use reth_tasks::Runtime;

/// Builds the [`BackfillSync`] the engine hands long-range sync to.
///
/// Nodes pick one with [`EngineNodeLauncher::with_backfill`](crate::EngineNodeLauncher); the
/// default is [`PipelineBackfill`].
pub trait BackfillSyncBuilder<N: ProviderNodeTypes, C>: Send {
    /// The backfill this builder produces.
    type Backfill: BackfillSync + Unpin + 'static;

    /// Builds the backfill from the node's staged pipeline, block client and database.
    fn build(self, ctx: BackfillContext<N, C>) -> eyre::Result<Self::Backfill>;

    /// Recovers from an earlier run that stopped mid-write, before the node checks the database
    /// for consistency. A backfill that commits static files and the database separately uses it
    /// so a crash between the two isn't treated as corruption. Does nothing by default.
    fn recover(&mut self, _provider_factory: &ProviderFactory<N>) -> eyre::Result<()> {
        Ok(())
    }
}

/// What the launcher hands a [`BackfillSyncBuilder`].
#[derive(Debug)]
pub struct BackfillContext<N: ProviderNodeTypes, C> {
    // Staged pipeline the backfill runs or hands over to.
    pipeline: Pipeline<N>,
    // Client the pipeline downloads blocks with.
    client: C,
    provider_factory: ProviderFactory<N>,
    runtime: Runtime,
}

impl<N: ProviderNodeTypes, C> BackfillContext<N, C> {
    /// Creates the context for one backfill build.
    pub const fn new(
        pipeline: Pipeline<N>,
        client: C,
        provider_factory: ProviderFactory<N>,
        runtime: Runtime,
    ) -> Self {
        Self { pipeline, client, provider_factory, runtime }
    }

    /// Returns the client the pipeline downloads blocks with.
    pub const fn client(&self) -> &C {
        &self.client
    }

    /// Returns the node's database.
    pub const fn provider_factory(&self) -> &ProviderFactory<N> {
        &self.provider_factory
    }

    /// Returns the runtime backfill tasks are spawned on.
    pub const fn runtime(&self) -> &Runtime {
        &self.runtime
    }

    /// Consumes the context, returning the staged pipeline.
    pub fn into_pipeline(self) -> Pipeline<N> {
        self.pipeline
    }
}

/// Backfills with the staged pipeline, as every node does unless it picks another backfill.
#[derive(Debug, Clone, Copy, Default)]
pub struct PipelineBackfill;

impl<N: ProviderNodeTypes, C> BackfillSyncBuilder<N, C> for PipelineBackfill {
    type Backfill = PipelineSync<N>;

    fn build(self, ctx: BackfillContext<N, C>) -> eyre::Result<Self::Backfill> {
        let runtime = ctx.runtime().clone();
        Ok(PipelineSync::new(ctx.into_pipeline(), runtime))
    }
}

/// Client the network of `Components` hands the engine's backfill.
pub type BackfillClientFor<T, Components> =
    <<Components as NodeComponents<T>>::Network as BlockDownloaderProvider>::Client;

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::B256;
    use reth_engine_tree::backfill::{BackfillAction, BackfillEvent};
    use reth_provider::test_utils::{create_test_provider_factory, MockNodeTypesWithDB};
    use reth_prune::PruneModes;
    use reth_stages::PipelineTarget;
    use reth_static_file::StaticFileProducer;
    use std::task::{Context, Poll, Waker};

    #[tokio::test]
    async fn pipeline_backfill_starts_the_staged_pipeline() {
        let factory = create_test_provider_factory();
        let pipeline = Pipeline::<MockNodeTypesWithDB>::builder()
            .with_tip_sender(tokio::sync::watch::channel(B256::ZERO).0)
            .build(
                factory.clone(),
                StaticFileProducer::new(factory.clone(), PruneModes::default()),
            );
        let ctx = BackfillContext::new(pipeline, (), factory, Runtime::test());
        let mut backfill = PipelineBackfill.build(ctx).unwrap();

        let target = PipelineTarget::Sync(B256::repeat_byte(1));
        backfill.on_action(BackfillAction::Start(target));

        assert!(matches!(
            backfill.poll(&mut Context::from_waker(Waker::noop())),
            Poll::Ready(BackfillEvent::Started(started)) if started == target
        ));
    }
}
