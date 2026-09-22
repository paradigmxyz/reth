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
            let Some(headers) = self.sync_headers(pipeline, &mut targets).await else {
                return stopped
            };
            // A detached head unwinds headers short of the target, so sync them again.
            if headers?.is_unwind() {
                continue
            }

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

    // Runs the header stage to the latest target, or returns `None` once the run is stopped.
    // Snap needs canonical headers and their BAL commitments, but nothing below the pivot may
    // execute, so only the header stage runs.
    async fn sync_headers(
        &self,
        pipeline: &mut Pipeline<N>,
        targets: &mut watch::Receiver<B256>,
    ) -> Option<Result<ControlFlow, PipelineError>> {
        let tip = pipeline.tip_sender();
        let target = *targets.borrow_and_update();
        let mut headers =
            pin!(pipeline.run_until(StageId::Headers, Some(PipelineTarget::Sync(target))));
        let headers = async {
            loop {
                tokio::select! {
                    biased;
                    result = &mut headers => return result,
                    // The header stage re-reads the tip when polled, so a new target replaces one
                    // no peer serves anymore, such as a hash reorged out.
                    changed = targets.changed() => match changed {
                        Ok(()) => {
                            if let Some(tip) = &tip {
                                tip.send_replace(*targets.borrow_and_update());
                            }
                        }
                        // The backfill is gone, and its stop token ends this run.
                        Err(_) => return headers.await,
                    },
                }
            }
        };
        self.stop.run_until_cancelled(headers).await
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
    use crate::snap::tests::{
        headers_done, headers_reach, pipeline, pipeline_with, NEXT_TARGET, TARGET,
    };
    use alloy_eips::{eip1898::BlockWithParent, BlockNumHash};
    use reth_consensus::ConsensusError;
    use reth_network_p2p::NoopFullBlockClient;
    use reth_provider::test_utils::MockNodeTypesWithDB;
    use reth_stages::{
        ExecInput, ExecOutput, Stage, StageCheckpoint, StageError, UnwindInput, UnwindOutput,
    };
    use reth_stages_api::test_utils::TestStage;
    use std::task::{Context, Poll};

    #[tokio::test]
    async fn a_new_target_refreshes_headers_then_resumes_until_stopped() {
        let (pipeline, factory) = pipeline(
            TestStage::new(StageId::Headers).add_exec(headers_done(0)).add_exec(headers_done(1)),
        );
        let (targets, receiver) = watch::channel(TARGET);
        let (run, stop) = snap_run(&factory);
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

    #[tokio::test]
    async fn a_new_target_retargets_a_pending_header_download() {
        let (tip, tip_rx) = watch::channel(B256::ZERO);
        let mut pipeline_tip = tip.subscribe();
        let (pipeline, factory) = pipeline_with(PendingUntilRetargeted(tip_rx), tip);
        let (targets, receiver) = watch::channel(TARGET);
        let (run, stop) = snap_run(&factory);
        let run = tokio::spawn(run.run(pipeline, receiver));
        // Headers are downloading towards the first target before forkchoice moves.
        pipeline_tip.wait_for(|tip| *tip == TARGET).await.unwrap();

        targets.send(NEXT_TARGET).unwrap();

        tokio::time::timeout(Duration::from_secs(5), headers_reach(&factory, 1))
            .await
            .expect("the pending header download moves to the new target");
        stop.cancel();
        let (_pipeline, result) = run.await.unwrap();
        assert!(matches!(result, Ok(ControlFlow::NoProgress { block_number: None })));
    }

    #[tokio::test]
    async fn a_detached_head_syncs_headers_again_before_the_bootstrap() {
        let block = |number| BlockWithParent {
            parent: B256::ZERO,
            block: BlockNumHash::new(number, B256::repeat_byte(number as u8)),
        };
        let (pipeline, factory) = pipeline(
            TestStage::new(StageId::Headers)
                .add_exec(Err(StageError::DetachedHead {
                    local_head: Box::new(block(1)),
                    header: Box::new(block(2)),
                    error: Box::new(ConsensusError::BaseFeeMissing),
                }))
                .add_exec(headers_done(2)),
        );
        let (_targets, receiver) = watch::channel(TARGET);
        let (run, stop) = snap_run(&factory);
        let run = tokio::spawn(run.run(pipeline, receiver));

        // Forkchoice never moves, so only the unwind itself can trigger the second header run.
        tokio::time::timeout(Duration::from_secs(5), headers_reach(&factory, 2))
            .await
            .expect("headers are synced again after the unwind");
        stop.cancel();
        let (_pipeline, result) = run.await.unwrap();
        assert!(matches!(result, Ok(ControlFlow::NoProgress { block_number: None })));
    }

    // A run over `factory` that refreshes headers as soon as forkchoice moves.
    fn snap_run(
        factory: &ProviderFactory<MockNodeTypesWithDB>,
    ) -> (SnapRun<MockNodeTypesWithDB, NoopFullBlockClient>, CancellationToken) {
        let stop = CancellationToken::new();
        let run = SnapRun {
            client: NoopFullBlockClient::default(),
            factory: factory.clone(),
            runtime: Runtime::test(),
            header_refresh: Duration::ZERO,
            stop: stop.clone(),
        };
        (run, stop)
    }

    // A header stage stuck until the tip moves to `NEXT_TARGET`, like a download retrying a hash
    // no peer serves.
    #[derive(Debug)]
    struct PendingUntilRetargeted(watch::Receiver<B256>);

    impl<Provider> Stage<Provider> for PendingUntilRetargeted {
        fn id(&self) -> StageId {
            StageId::Headers
        }

        fn poll_execute_ready(
            &mut self,
            cx: &mut Context<'_>,
            _input: ExecInput,
        ) -> Poll<Result<(), StageError>> {
            if *self.0.borrow() == NEXT_TARGET {
                return Poll::Ready(Ok(()))
            }
            cx.waker().wake_by_ref();
            Poll::Pending
        }

        fn execute(
            &mut self,
            _provider: &Provider,
            _input: ExecInput,
        ) -> Result<ExecOutput, StageError> {
            Ok(ExecOutput { checkpoint: StageCheckpoint::new(1), done: true })
        }

        fn unwind(
            &mut self,
            _provider: &Provider,
            _input: UnwindInput,
        ) -> Result<UnwindOutput, StageError> {
            unreachable!("nothing unwinds in this test")
        }
    }
}
