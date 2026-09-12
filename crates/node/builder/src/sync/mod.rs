//! Backfill implementations the launcher can hand to the engine orchestrator.

mod snap;

pub use snap::SnapBackfillSync;

use reth_engine_tree::backfill::{BackfillAction, BackfillEvent, BackfillSync, PipelineSync};
use reth_network_p2p::{headers::client::HeadersClient, snap::client::SnapClient};
use reth_provider::{providers::ProviderNodeTypes, ProviderFactory};
use reth_stages_api::Pipeline;
use reth_tasks::Runtime;
use std::task::{Context, Poll};

/// The backfill strategy selected at node startup.
#[derive(Debug)]
pub(crate) enum NodeBackfillSync<N: ProviderNodeTypes, C> {
    Pipeline(PipelineSync<N>),
    Snap(Box<SnapBackfillSync<N, C>>),
}

impl<N: ProviderNodeTypes, C> NodeBackfillSync<N, C> {
    pub(crate) fn new(
        snap_v2: bool,
        pipeline: Pipeline<N>,
        client: C,
        provider_factory: ProviderFactory<N>,
        runtime: Runtime,
    ) -> Self {
        if snap_v2 {
            Self::Snap(Box::new(SnapBackfillSync::new(pipeline, client, provider_factory, runtime)))
        } else {
            Self::Pipeline(PipelineSync::new(pipeline, runtime))
        }
    }
}

impl<N, C> BackfillSync for NodeBackfillSync<N, C>
where
    N: ProviderNodeTypes,
    C: SnapClient + HeadersClient + Clone + Unpin + 'static,
{
    fn on_action(&mut self, action: BackfillAction) {
        match self {
            Self::Pipeline(sync) => sync.on_action(action),
            Self::Snap(sync) => sync.on_action(action),
        }
    }

    fn poll(&mut self, cx: &mut Context<'_>) -> Poll<BackfillEvent> {
        match self {
            Self::Pipeline(sync) => sync.poll(cx),
            Self::Snap(sync) => sync.poll(cx),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::B256;
    use reth_network_p2p::NoopFullBlockClient;
    use reth_provider::{
        test_utils::{create_test_provider_factory, MockNodeTypesWithDB},
        DBProvider, DatabaseProviderFactory, StageCheckpointWriter,
    };
    use reth_prune::PruneModes;
    use reth_stages_api::{PipelineTarget, StageId};
    use reth_static_file::StaticFileProducer;
    use std::task::Waker;

    #[tokio::test]
    async fn only_snap_backfill_reads_the_generation_marker() {
        for enabled in [false, true] {
            let factory = create_test_provider_factory();
            let provider = factory.database_provider_rw().unwrap();
            provider
                .save_stage_checkpoint_progress(StageId::Other("SnapSync"), vec![0xff])
                .unwrap();
            provider.commit().unwrap();

            let pipeline = Pipeline::<MockNodeTypesWithDB>::builder()
                .with_tip_sender(tokio::sync::watch::channel(B256::ZERO).0)
                .build(
                    factory.clone(),
                    StaticFileProducer::new(factory.clone(), PruneModes::default()),
                );
            let client: NoopFullBlockClient = NoopFullBlockClient::default();
            let mut sync =
                NodeBackfillSync::new(enabled, pipeline, client, factory, Runtime::test());
            let target = PipelineTarget::Sync(B256::repeat_byte(1));
            sync.on_action(BackfillAction::Start(target));
            let event = sync.poll(&mut Context::from_waker(Waker::noop()));

            if enabled {
                assert!(matches!(event, Poll::Ready(BackfillEvent::Finished(Err(_)))));
            } else {
                assert!(
                    matches!(event, Poll::Ready(BackfillEvent::Started(started)) if started == target)
                );
            }
        }
    }
}
