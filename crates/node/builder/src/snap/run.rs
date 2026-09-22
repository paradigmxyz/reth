//! Alternates header catch-up and snap bootstrap until the state is downloaded.

use super::context::NodeSnapContext;
use alloy_primitives::B256;
use reth_errors::RethError;
use reth_network_p2p::snap::client::SnapClient;
use reth_provider::{providers::ProviderNodeTypes, ProviderFactory};
use reth_snap_sync::{SnapBootstrap, SnapBootstrapOutcome};
use reth_stages::{
    ControlFlow, Pipeline, PipelineError, PipelineTarget, PipelineWithResult, StageId,
};
use reth_tasks::Runtime;
use std::{pin::pin, time::Duration};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info};

/// Minimum time between header refreshes while forkchoice moves.
pub(super) const HEADER_REFRESH: Duration = Duration::from_secs(120);

// Everything one spawned run needs, moved into its task.
pub(super) struct SnapRun<N: ProviderNodeTypes, C> {
    pub(super) client: C,
    pub(super) factory: ProviderFactory<N>,
    pub(super) runtime: Runtime,
    pub(super) header_refresh: Duration,
    // Cancelled when the backfill is dropped.
    pub(super) stop: CancellationToken,
}

impl<N, C> SnapRun<N, C>
where
    N: ProviderNodeTypes,
    C: SnapClient + Clone + Unpin + 'static,
{
    // Hands the pipeline back with the result, so the backfill can run it again.
    pub(super) async fn run(
        self,
        mut pipeline: Pipeline<N>,
        targets: watch::Receiver<B256>,
    ) -> PipelineWithResult<N> {
        let result = self.bootstrap(&mut pipeline, targets).await;
        (pipeline, result)
    }

    // Alternates header catch-up and snap bootstrap until the state is downloaded or the run
    // stops. Every bootstrap step commits, so each loop resumes where the last one stopped.
    async fn bootstrap(
        &self,
        pipeline: &mut Pipeline<N>,
        mut targets: watch::Receiver<B256>,
    ) -> Result<ControlFlow, PipelineError> {
        // Returning without progress hands control back to the engine without a fatal error.
        let stopped = Ok(ControlFlow::NoProgress { block_number: None });
        loop {
            let target = *targets.borrow_and_update();
            // Snap needs canonical headers and their BAL commitments, but nothing below the pivot
            // may execute, so only the header stage runs.
            let headers = pipeline.run_until(StageId::Headers, Some(PipelineTarget::Sync(target)));
            let Some(headers) = self.stop.run_until_cancelled(headers).await else {
                return stopped
            };
            headers?;

            let run_stop = self.stop.child_token();
            let context =
                NodeSnapContext::new(self.factory.clone(), self.client.clone(), targets.clone());
            let mut session = SnapBootstrap::new(
                self.client.clone(),
                self.factory.clone(),
                self.runtime.clone(),
                context,
            )
            .with_cancellation(run_stop.clone());
            let outcome = {
                let mut run = pin!(session.run());
                tokio::select! {
                    biased;
                    outcome = &mut run => outcome,
                    () = refresh_due(&mut targets, self.header_refresh) => {
                        // Stops at the next step boundary, keeping committed progress.
                        run_stop.cancel();
                        run.await
                    }
                }
            };

            match outcome.map_err(|error| PipelineError::Internal(RethError::other(error)))? {
                SnapBootstrapOutcome::Stopped if self.stop.is_cancelled() => return stopped,
                SnapBootstrapOutcome::Stopped => {
                    debug!(target: "sync::snap", "Refreshing headers before resuming snap sync");
                }
                SnapBootstrapOutcome::TrieRebuild { pivot, .. } |
                SnapBootstrapOutcome::Verified { pivot } => {
                    info!(target: "sync::snap", ?pivot, "Snap state downloaded");
                    // Failing keeps the engine from executing on top of unactivated state.
                    return Err(PipelineError::Internal(RethError::msg(
                        "activating snap state is not supported yet",
                    )))
                }
            }
        }
    }
}

// Resolves once `interval` has passed and forkchoice has moved, or once the backfill is gone.
async fn refresh_due(targets: &mut watch::Receiver<B256>, interval: Duration) {
    tokio::time::sleep(interval).await;
    let _ = targets.changed().await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snap::tests::{headers_done, headers_reach, pipeline, NEXT_TARGET, TARGET};
    use reth_network_p2p::NoopFullBlockClient;
    use reth_stages_api::test_utils::TestStage;

    #[tokio::test]
    async fn a_new_target_refreshes_headers_then_resumes_until_stopped() {
        let (pipeline, factory) = pipeline(
            TestStage::new(StageId::Headers).add_exec(headers_done(0)).add_exec(headers_done(1)),
        );
        let (targets, receiver) = watch::channel(TARGET);
        let client: NoopFullBlockClient = NoopFullBlockClient::default();
        let stop = CancellationToken::new();
        let run = SnapRun {
            client,
            factory: factory.clone(),
            runtime: Runtime::test(),
            header_refresh: Duration::ZERO,
            stop: stop.clone(),
        };
        let run = tokio::spawn(run.run(pipeline, receiver));
        headers_reach(&factory, 0).await;

        targets.send(NEXT_TARGET).unwrap();

        // The bootstrap stopped, headers caught up to the new target and a new bootstrap resumed.
        headers_reach(&factory, 1).await;
        assert!(!run.is_finished());

        stop.cancel();
        let (_pipeline, result) = run.await.unwrap();
        assert!(matches!(result, Ok(ControlFlow::NoProgress { block_number: None })));
    }
}
