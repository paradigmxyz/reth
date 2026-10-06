//! Alternates header catch-up and snap bootstrap until the state is downloaded.

use super::{
    context::NodeSnapContext,
    handoff::{HandoffOutcome, RebuildOutcome, SnapHandoff},
};
use alloy_consensus::{BlockHeader, Sealable};
use alloy_eips::BlockNumHash;
use alloy_primitives::B256;
use reth_errors::RethError;
use reth_network_p2p::{headers::client::HeadersClient, snap::client::SnapClient};
use reth_provider::{
    providers::ProviderNodeTypes, BlockHashReader, BlockNumReader, ProviderError, ProviderFactory,
};
use reth_snap_sync::{SnapBootstrap, SnapBootstrapOutcome, SnapWrite};
use reth_stages::{
    ControlFlow, Pipeline, PipelineError, PipelineTarget, PipelineWithResult, StageId,
};
use reth_tasks::Runtime;
use reth_tracing::tracing::{debug, info, warn};
use std::{pin::pin, time::Duration};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

// Minimum time between header refreshes while forkchoice moves.
//
// Ten slots keeps the head well inside the ~128 recent blocks peers serve state for, without
// stopping the download every slot.
const HEADER_REFRESH: Duration = Duration::from_secs(120);

// Retry unresolved targets without flooding peers while the head remains unchanged.
const HEADER_LOOKUP_RETRY: Duration = Duration::from_secs(1);

// Returning without progress hands control back to the engine without a fatal error.
const STOPPED: ControlFlow = ControlFlow::NoProgress { block_number: None };

// Root mismatches one run tolerates before failing. A mismatch that repeats across fresh
// attempts points at local state rather than peers, so restarting again would loop forever.
const MAX_ROOT_MISMATCHES: u32 = 3;

// Everything one spawned run needs, moved into its task.
pub(super) struct SnapRun<N: ProviderNodeTypes, C> {
    client: C,
    factory: ProviderFactory<N>,
    runtime: Runtime,
    header_refresh: Duration,
    // Root mismatches the run tolerates before failing.
    max_root_mismatches: u32,
    // Cancelled when the backfill is dropped.
    stop: CancellationToken,
    // Latest finalized block the engine reported.
    finalized: watch::Receiver<B256>,
}

impl<N: ProviderNodeTypes, C> SnapRun<N, C> {
    pub(super) const fn new(
        client: C,
        factory: ProviderFactory<N>,
        runtime: Runtime,
        stop: CancellationToken,
        finalized: watch::Receiver<B256>,
    ) -> Self {
        Self {
            client,
            factory,
            runtime,
            header_refresh: HEADER_REFRESH,
            max_root_mismatches: MAX_ROOT_MISMATCHES,
            stop,
            finalized,
        }
    }

    #[cfg(test)]
    const fn with_header_refresh(mut self, header_refresh: Duration) -> Self {
        self.header_refresh = header_refresh;
        self
    }

    #[cfg(test)]
    const fn with_max_root_mismatches(mut self, max_root_mismatches: u32) -> Self {
        self.max_root_mismatches = max_root_mismatches;
        self
    }
}

impl<N, C> SnapRun<N, C>
where
    N: ProviderNodeTypes,
    C: SnapClient + HeadersClient + Clone + Unpin + 'static,
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
        let mut mismatches = 0;
        let verified = loop {
            match self.catch_up_headers(pipeline, &mut targets).await? {
                Pass::Stopped => return Ok(STOPPED),
                Pass::Again | Pass::RootMismatch => continue,
                Pass::Done => {}
            }
            match self.download(&mut targets).await? {
                SnapBootstrapOutcome::Stopped if self.stop.is_cancelled() => return Ok(STOPPED),
                SnapBootstrapOutcome::Stopped => {
                    debug!(target: "sync::snap", "Refreshing headers before resuming snap sync");
                }
                SnapBootstrapOutcome::TrieRebuild { write, pivot } => {
                    match self.rebuild_and_hand_off(pipeline, &mut targets, write, pivot).await? {
                        Pass::Stopped => return Ok(STOPPED),
                        Pass::Again => {}
                        Pass::RootMismatch => {
                            mismatches += 1;
                            if mismatches >= self.max_root_mismatches {
                                return Err(PipelineError::Internal(RethError::msg(format!(
                                    "snap state root mismatched in {mismatches} attempts"
                                ))))
                            }
                        }
                        Pass::Done => break Some(pivot),
                    }
                }
                SnapBootstrapOutcome::Verified { pivot } => break Some(pivot),
                SnapBootstrapOutcome::BeforeBlockAccessLists => break None,
            }
        };
        match verified {
            Some(pivot) => {
                info!(target: "sync::snap", ?pivot, "Snap state verified, resuming the pipeline");
            }
            None => {
                info!(target: "sync::snap", "Chain predates block access lists, syncing with the staged pipeline");
            }
        }
        self.finish(pipeline, &mut targets).await
    }

    // Runs one bootstrap until the state is downloaded, the backfill stops or headers are due.
    async fn download(
        &self,
        targets: &mut watch::Receiver<B256>,
    ) -> Result<SnapBootstrapOutcome, PipelineError> {
        let run_stop = self.stop.child_token();
        let context = NodeSnapContext::new(
            self.factory.clone(),
            self.client.clone(),
            targets.clone(),
            self.finalized.clone(),
        );
        let mut session = SnapBootstrap::new(
            self.client.clone(),
            self.factory.clone(),
            self.runtime.clone(),
            context,
        )
        .with_cancellation(run_stop.clone())
        .with_shutdown(self.stop.clone());
        let mut run = pin!(session.run());
        let outcome = tokio::select! {
            biased;
            outcome = &mut run => outcome,
            () = self.refresh_due(targets) => {
                // Stops at the next step boundary, keeping committed progress.
                run_stop.cancel();
                run.await
            }
        };
        match outcome {
            Ok(outcome) => Ok(outcome),
            Err(error) if error.is_fatal() => Err(PipelineError::Internal(RethError::other(error))),
            // A failed sync waits for the next head, and committed progress lets the next bootstrap
            // resume.
            Err(error) => {
                warn!(target: "sync::snap", %error, "Snap sync failed, retrying once forkchoice moves");
                let _ = self.stop.run_until_cancelled(targets.changed()).await;
                Ok(SnapBootstrapOutcome::Stopped)
            }
        }
    }

    // Rebuilds the trie at the pivot of `write`'s attempt, then hands the state off. The rebuild
    // can take hours, so it stops with the backfill.
    async fn rebuild_and_hand_off(
        &self,
        pipeline: &mut Pipeline<N>,
        targets: &mut watch::Receiver<B256>,
        write: SnapWrite,
        pivot: BlockNumHash,
    ) -> Result<Pass, PipelineError> {
        match self.with_handoff(move |handoff, stop| handoff.rebuild(write, stop)).await? {
            RebuildOutcome::Rebuilt => self.hand_off(pipeline, targets, write, pivot).await,
            RebuildOutcome::RootMismatch => Ok(Pass::RootMismatch),
            RebuildOutcome::Stopped => Ok(Pass::Stopped),
        }
    }

    // Catches headers up, since forkchoice may have moved during the rebuild, then publishes the
    // state if its pivot is still canonical.
    async fn hand_off(
        &self,
        pipeline: &mut Pipeline<N>,
        targets: &mut watch::Receiver<B256>,
        write: SnapWrite,
        pivot: BlockNumHash,
    ) -> Result<Pass, PipelineError> {
        let headers = self.catch_up_headers(pipeline, targets).await?;
        if headers != Pass::Done {
            return Ok(headers)
        }
        Ok(match self.with_handoff(move |handoff, stop| handoff.hand_off(write, stop)).await? {
            HandoffOutcome::Completed => Pass::Done,
            HandoffOutcome::PivotReorged => {
                info!(target: "sync::snap", ?pivot, "Snap pivot was reorged before the handoff, restarting");
                Pass::Again
            }
            HandoffOutcome::RootMismatch => Pass::RootMismatch,
            HandoffOutcome::Stopped => Pass::Stopped,
        })
    }

    // Runs `f` on the blocking pool, since rebuilding and publishing read every account.
    async fn with_handoff<T: Send + 'static>(
        &self,
        f: impl FnOnce(SnapHandoff<N>, &CancellationToken) -> Result<T, PipelineError> + Send + 'static,
    ) -> Result<T, PipelineError> {
        let handoff = SnapHandoff::new(self.factory.clone());
        let stop = self.stop.clone();
        self.runtime
            .spawn_blocking(move || f(handoff, &stop))
            .await
            .map_err(|error| PipelineError::Internal(RethError::other(error)))?
    }

    // Runs every stage to the latest target, above the pivot once snap state is published.
    async fn finish(
        &self,
        pipeline: &mut Pipeline<N>,
        targets: &mut watch::Receiver<B256>,
    ) -> Result<ControlFlow, PipelineError> {
        let Some((target, ahead)) = self.resolve_target(targets).await? else { return Ok(STOPPED) };
        // A stale target never completes the header stage, so the stages run to the local
        // head and the engine follows forkchoice from there.
        let target = if ahead { target } else { self.local_head()? };
        let stages = pipeline.run_until(StageId::Finish, Some(PipelineTarget::Sync(target)));
        self.stop.run_until_cancelled(stages).await.unwrap_or(Ok(STOPPED))
    }

    // Syncs headers to the latest target. A detached head unwinds headers short of the target,
    // and a target that moved during the pass is not synced yet, so both need another pass.
    async fn catch_up_headers(
        &self,
        pipeline: &mut Pipeline<N>,
        targets: &mut watch::Receiver<B256>,
    ) -> Result<Pass, PipelineError> {
        let Some((target, ahead)) = self.resolve_target(targets).await? else {
            return Ok(Pass::Stopped)
        };
        if !ahead {
            debug!(target: "sync::snap", %target, "Forkchoice target is not above the local headers, skipping the header pass");
            return Ok(Pass::Done)
        }
        // Snap needs canonical headers and their BAL commitments, but nothing below the pivot may
        // execute, so only the header stage runs. Finish the pass before retargeting: dropping it
        // retains partially downloaded headers in the stage's ETL collectors.
        let headers = pipeline.run_until(StageId::Headers, Some(PipelineTarget::Sync(target)));
        let Some(headers) = self.stop.run_until_cancelled(headers).await else {
            return Ok(Pass::Stopped)
        };
        if headers?.is_unwind() || targets.has_changed().unwrap_or(false) {
            return Ok(Pass::Again)
        }
        Ok(Pass::Done)
    }

    // Whether `target` is above the local headers, or `None` when no peer serves its header. The
    // header stage only finishes once it downloads the block after the local head, so a pass to a
    // sibling or an older block never completes.
    async fn is_ahead(&self, target: B256) -> Result<Option<bool>, PipelineError> {
        let head = {
            let provider = self.factory.provider()?;
            if provider.block_number(target)?.is_some() {
                return Ok(Some(false))
            }
            provider.last_block_number()?
        };
        let header = self.client.get_header(target.into()).await;
        // A peer may answer with another block's header.
        Ok(header
            .ok()
            .and_then(|header| header.into_data())
            .filter(|header| header.hash_slow() == target)
            .map(|header| header.number() > head))
    }

    // Returns the hash of the highest local header.
    fn local_head(&self) -> Result<B256, PipelineError> {
        let provider = self.factory.provider()?;
        let number = provider.last_block_number()?;
        Ok(provider.block_hash(number)?.ok_or(ProviderError::HeaderNotFound(number.into()))?)
    }

    // Resolves once the refresh interval has passed and forkchoice has moved, or once the
    // backfill is gone.
    async fn refresh_due(&self, targets: &mut watch::Receiver<B256>) {
        tokio::time::sleep(self.header_refresh).await;
        let _ = targets.changed().await;
    }

    // Resolve the target before starting a pipeline pass, which cannot safely be retargeted with
    // headers buffered. Only the peer lookup and retry delay are interrupted by forkchoice.
    async fn resolve_target(
        &self,
        targets: &mut watch::Receiver<B256>,
    ) -> Result<Option<(B256, bool)>, PipelineError> {
        loop {
            let target = *targets.borrow_and_update();
            let ahead = tokio::select! {
                biased;
                () = self.stop.cancelled() => return Ok(None),
                Ok(()) = targets.changed() => continue,
                ahead = self.is_ahead(target) => ahead?,
            };
            if let Some(ahead) = ahead {
                return Ok(Some((target, ahead)))
            }
            tokio::select! {
                biased;
                () = self.stop.cancelled() => return Ok(None),
                Ok(()) = targets.changed() => {},
                () = tokio::time::sleep(HEADER_LOOKUP_RETRY) => {},
            }
        }
    }
}

// What a header or handoff pass leaves the run to do.
#[derive(Debug, PartialEq, Eq)]
enum Pass {
    // The backfill stopped, so the run returns without progress.
    Stopped,
    // Headers moved or the pivot was reorged out, so the run starts over from headers.
    Again,
    // The rebuilt root differs from the pivot's header, so the attempt was abandoned and the run
    // starts over from headers.
    RootMismatch,
    // The pass finished, so the run moves on to its next step.
    Done,
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::snap::{
        handoff::tests::{downloaded_attempt, PIVOT},
        tests::{
            hashed_factory, headers_done, headers_reach, next_target, pipeline, pipeline_on,
            pipeline_with, target,
        },
    };
    use alloy_consensus::Header;
    use alloy_eips::eip1898::BlockWithParent;
    use futures::future::{pending, ready, Either, Pending, Ready};
    use reth_consensus::ConsensusError;
    use reth_db::{tables, transaction::DbTxMut};
    use reth_eth_wire_types::snap::{
        GetAccountRangeMessage, GetBlockAccessListsMessage, GetByteCodesMessage,
        GetStorageRangesMessage,
    };
    use reth_network_p2p::{
        download::DownloadClient, error::PeerRequestResult, headers::client::HeadersRequest,
        priority::Priority, NoopFullBlockClient,
    };
    use reth_network_peers::{PeerId, WithPeerId};
    use reth_primitives_traits::{Account, SealedHeader};
    use reth_provider::{
        test_utils::{insert_headers, MockNodeTypesWithDB},
        DBProvider, DatabaseProviderFactory, HeaderProvider, MetadataProvider,
    };
    use reth_snap_sync::{SnapAttemptStore, SnapStateVerifier, DEFAULT_SCAN_CHUNK};
    use reth_stages::{
        ExecInput, ExecOutput, Stage, StageError, StageSetBuilder, UnwindInput, UnwindOutput,
    };
    use reth_stages_api::test_utils::TestStage;
    use std::{
        sync::{
            atomic::{AtomicUsize, Ordering},
            Arc, Mutex,
        },
        task::{Context, Poll},
    };

    // Answers a header request with the requested one of `headers`, or with all of them when none
    // matches, as a faulty peer would. Snap requests fail like the noop client's.
    #[derive(Clone, Debug)]
    pub(crate) struct ServesHeaders {
        headers: Vec<Header>,
        noop: NoopFullBlockClient,
        empty_responses: Arc<AtomicUsize>,
        pending_target: Option<B256>,
    }

    impl Default for ServesHeaders {
        fn default() -> Self {
            Self {
                headers: (100..=101)
                    .map(|number| Header { number, ..Default::default() })
                    .collect(),
                noop: NoopFullBlockClient::default(),
                empty_responses: Arc::default(),
                pending_target: None,
            }
        }
    }

    impl DownloadClient for ServesHeaders {
        fn report_bad_message(&self, _peer_id: PeerId) {}

        fn num_connected_peers(&self) -> usize {
            1
        }
    }

    impl HeadersClient for ServesHeaders {
        type Header = Header;
        type Output =
            Either<Ready<PeerRequestResult<Vec<Header>>>, Pending<PeerRequestResult<Vec<Header>>>>;

        fn get_headers_with_priority(
            &self,
            request: HeadersRequest,
            _priority: Priority,
        ) -> Self::Output {
            if self.pending_target.is_some_and(|hash| request.start == hash.into()) {
                return Either::Right(pending())
            }
            if self
                .empty_responses
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |left| left.checked_sub(1))
                .is_ok()
            {
                return Either::Left(ready(Ok(WithPeerId::new(PeerId::random(), vec![]))))
            }
            let requested = self
                .headers
                .iter()
                .find(|header| request.start == header.hash_slow().into())
                .map_or_else(|| self.headers.clone(), |header| vec![header.clone()]);
            Either::Left(ready(Ok(WithPeerId::new(PeerId::random(), requested))))
        }
    }

    impl SnapClient for ServesHeaders {
        type Output = <NoopFullBlockClient as SnapClient>::Output;

        fn get_account_range_with_priority(
            &self,
            request: GetAccountRangeMessage,
            priority: Priority,
        ) -> Self::Output {
            self.noop.get_account_range_with_priority(request, priority)
        }

        fn get_storage_ranges(&self, request: GetStorageRangesMessage) -> Self::Output {
            self.noop.get_storage_ranges(request)
        }

        fn get_storage_ranges_with_priority(
            &self,
            request: GetStorageRangesMessage,
            priority: Priority,
        ) -> Self::Output {
            self.noop.get_storage_ranges_with_priority(request, priority)
        }

        fn get_byte_codes(&self, request: GetByteCodesMessage) -> Self::Output {
            self.noop.get_byte_codes(request)
        }

        fn get_byte_codes_with_priority(
            &self,
            request: GetByteCodesMessage,
            priority: Priority,
        ) -> Self::Output {
            self.noop.get_byte_codes_with_priority(request, priority)
        }

        fn get_block_access_lists_with_priority(
            &self,
            request: GetBlockAccessListsMessage,
            priority: Priority,
        ) -> Self::Output {
            self.noop.get_block_access_lists_with_priority(request, priority)
        }
    }

    // A header stage that waits for `gate`, then records whether a snap attempt exists at each
    // pass.
    #[derive(Debug)]
    struct GatedHeaders {
        gate: watch::Receiver<bool>,
        attempts: Arc<Mutex<Vec<bool>>>,
        stage: TestStage,
    }

    impl<Provider: MetadataProvider> Stage<Provider> for GatedHeaders {
        fn id(&self) -> StageId {
            StageId::Headers
        }

        fn poll_execute_ready(
            &mut self,
            cx: &mut Context<'_>,
            _input: ExecInput,
        ) -> Poll<Result<(), StageError>> {
            if *self.gate.borrow() {
                return Poll::Ready(Ok(()))
            }
            cx.waker().wake_by_ref();
            Poll::Pending
        }

        fn execute(
            &mut self,
            provider: &Provider,
            input: ExecInput,
        ) -> Result<ExecOutput, StageError> {
            self.attempts.lock().unwrap().push(provider.snap_attempt()?.is_some());
            self.stage.execute(provider, input)
        }

        fn unwind(
            &mut self,
            _provider: &Provider,
            _input: UnwindInput,
        ) -> Result<UnwindOutput, StageError> {
            unreachable!("nothing unwinds in this test")
        }
    }

    // A run over `factory` that refreshes headers as soon as forkchoice moves.
    fn snap_run(
        factory: &ProviderFactory<MockNodeTypesWithDB>,
    ) -> (SnapRun<MockNodeTypesWithDB, ServesHeaders>, CancellationToken) {
        snap_run_with(factory, ServesHeaders::default())
    }

    // A run over `factory` requesting from `client` that refreshes headers as soon as forkchoice
    // moves.
    fn snap_run_with<C>(
        factory: &ProviderFactory<MockNodeTypesWithDB>,
        client: C,
    ) -> (SnapRun<MockNodeTypesWithDB, C>, CancellationToken) {
        let stop = CancellationToken::new();
        let run = SnapRun::new(
            client,
            factory.clone(),
            Runtime::test(),
            stop.clone(),
            watch::channel(B256::ZERO).1,
        )
        .with_header_refresh(Duration::ZERO);
        (run, stop)
    }

    // A downloaded attempt with its trie rebuild started, and a pipeline over the same database
    // running `headers`.
    fn handoff_ready(
        headers: TestStage,
    ) -> (
        Pipeline<MockNodeTypesWithDB>,
        ProviderFactory<MockNodeTypesWithDB>,
        SnapWrite,
        BlockNumHash,
    ) {
        let (factory, write) = downloaded_attempt();
        let pivot = factory.sealed_header(PIVOT).unwrap().unwrap().num_hash();
        let provider = factory.database_provider_rw().unwrap();
        provider.start_trie_rebuild(write, DEFAULT_SCAN_CHUNK, &CancellationToken::new()).unwrap();
        provider.commit().unwrap();
        let pipeline = pipeline_on(
            &factory,
            StageSetBuilder::default().add_stage(headers),
            watch::channel(B256::ZERO).0,
        );
        (pipeline, factory, write, pivot)
    }

    // A header stage error that unwinds the pipeline; only the unwind matters here.
    fn detached_head() -> StageError {
        StageError::DetachedHead {
            local_head: Box::new(BlockWithParent {
                parent: B256::ZERO,
                block: BlockNumHash::default(),
            }),
            header: Box::new(BlockWithParent {
                parent: B256::ZERO,
                block: BlockNumHash::default(),
            }),
            error: Box::new(ConsensusError::BaseFeeMissing),
        }
    }

    #[tokio::test]
    async fn a_new_target_refreshes_headers_then_resumes_until_stopped() {
        let (pipeline, factory) = pipeline(
            TestStage::new(StageId::Headers).add_exec(headers_done(0)).add_exec(headers_done(1)),
        );
        let (targets, receiver) = watch::channel(target());
        let (run, stop) = snap_run(&factory);
        let mut events = pipeline.events();
        let run = tokio::spawn(run.run(pipeline, receiver));
        headers_reach(&mut events, 0).await;

        targets.send(next_target()).unwrap();

        // The bootstrap stopped, headers caught up to the new target and a new bootstrap resumed.
        headers_reach(&mut events, 1).await;
        assert!(!run.is_finished());

        stop.cancel();
        let (_pipeline, result) = run.await.unwrap();
        assert!(matches!(result, Ok(ControlFlow::NoProgress { block_number: None })));
    }

    #[tokio::test]
    async fn a_target_moved_during_headers_is_synced_before_the_bootstrap() {
        let (open, gate) = watch::channel(false);
        let attempts = Arc::new(Mutex::new(Vec::new()));
        let headers = GatedHeaders {
            gate,
            attempts: attempts.clone(),
            stage: TestStage::new(StageId::Headers)
                .add_exec(headers_done(1))
                .add_exec(headers_done(2)),
        };
        let (tip, _) = watch::channel(B256::ZERO);
        let mut pipeline_tip = tip.subscribe();
        let (pipeline, factory) = pipeline_with(headers, tip);
        // Headers with block access list commitments, so a bootstrap can anchor a pivot.
        let mut parent = B256::ZERO;
        let stored: Vec<_> = (0..=64)
            .map(|number| {
                let header = SealedHeader::seal_slow(Header {
                    number,
                    parent_hash: parent,
                    block_access_list_hash: Some(B256::ZERO),
                    ..Default::default()
                });
                parent = header.hash();
                header
            })
            .collect();
        insert_headers(&factory, &stored);
        // Targets above the stored headers that peers serve, so each pass runs to completion.
        let target = Header { number: 65, parent_hash: parent, ..Default::default() };
        let next = Header { number: 66, parent_hash: target.hash_slow(), ..Default::default() };
        let (target_hash, next_hash) = (target.hash_slow(), next.hash_slow());
        let client = ServesHeaders { headers: vec![target, next], ..Default::default() };
        let (targets, receiver) = watch::channel(target_hash);
        let (run, stop) = snap_run_with(&factory, client);
        let mut events = pipeline.events();
        let run = tokio::spawn(run.run(pipeline, receiver));

        // Forkchoice moves while the first header pass is still downloading.
        pipeline_tip.wait_for(|tip| *tip == target_hash).await.unwrap();
        targets.send(next_hash).unwrap();
        open.send(true).unwrap();

        tokio::time::timeout(Duration::from_secs(5), headers_reach(&mut events, 2))
            .await
            .expect("headers are synced to the moved target");
        stop.cancel();
        let (_pipeline, result) = run.await.unwrap();
        assert!(matches!(result, Ok(ControlFlow::NoProgress { block_number: None })));
        // No bootstrap recorded an attempt on the outdated headers between the two passes.
        assert_eq!(*attempts.lock().unwrap(), [false, false]);
    }

    #[tokio::test]
    async fn a_detached_head_syncs_headers_again_before_the_bootstrap() {
        let (pipeline, factory) = pipeline(
            TestStage::new(StageId::Headers)
                .add_exec(Err(detached_head()))
                .add_exec(headers_done(2)),
        );
        let (_targets, receiver) = watch::channel(target());
        let (run, stop) = snap_run(&factory);
        let mut events = pipeline.events();
        let run = tokio::spawn(run.run(pipeline, receiver));

        // Forkchoice never moves, so only the unwind itself can trigger the second header run.
        tokio::time::timeout(Duration::from_secs(5), headers_reach(&mut events, 2))
            .await
            .expect("headers are synced again after the unwind");
        stop.cancel();
        let (_pipeline, result) = run.await.unwrap();
        assert!(matches!(result, Ok(ControlFlow::NoProgress { block_number: None })));
    }

    #[tokio::test]
    async fn the_pipeline_finishes_to_the_latest_target() {
        let (tip, _) = watch::channel(B256::ZERO);
        let pipeline_tip = tip.subscribe();
        let (mut pipeline, factory) =
            pipeline_with(TestStage::new(StageId::Finish).add_exec(headers_done(1)), tip);
        let (targets, mut receiver) = watch::channel(target());
        let (run, _stop) = snap_run(&factory);

        // Forkchoice moved while the state was downloading.
        targets.send(next_target()).unwrap();
        run.finish(&mut pipeline, &mut receiver).await.unwrap();

        assert_eq!(*pipeline_tip.borrow(), next_target());
    }

    #[tokio::test]
    async fn a_moved_target_syncs_headers_before_the_handoff() {
        let (mut pipeline, factory, write, pivot) =
            handoff_ready(TestStage::new(StageId::Headers).add_exec(headers_done(PIVOT)));
        let (targets, mut receiver) = watch::channel(target());
        let (run, _stop) = snap_run(&factory);

        // Forkchoice moved while the trie was rebuilding.
        targets.send(next_target()).unwrap();
        let pass =
            run.rebuild_and_hand_off(&mut pipeline, &mut receiver, write, pivot).await.unwrap();

        assert_eq!(pass, Pass::Done);
        assert!(factory.provider().unwrap().snap_attempt().unwrap().unwrap().is_verified());
    }

    #[tokio::test]
    async fn a_reorg_during_the_rebuild_syncs_headers_again_without_a_handoff() {
        let (mut pipeline, factory, write, pivot) =
            handoff_ready(TestStage::new(StageId::Headers).add_exec(Err(detached_head())));
        let (_targets, mut receiver) = watch::channel(target());
        let (run, _stop) = snap_run(&factory);

        let pass =
            run.rebuild_and_hand_off(&mut pipeline, &mut receiver, write, pivot).await.unwrap();

        assert_eq!(pass, Pass::Again);
        assert!(!factory.provider().unwrap().snap_attempt().unwrap().unwrap().is_verified());
    }

    #[tokio::test]
    async fn a_root_mismatch_starts_a_new_attempt_instead_of_failing() {
        let (mut pipeline, factory, write, pivot) = handoff_ready(TestStage::new(StageId::Headers));
        // State whose root differs from the one the pivot header commits to.
        let provider = factory.database_provider_rw().unwrap();
        provider
            .tx_ref()
            .put::<tables::HashedAccounts>(
                B256::repeat_byte(3),
                Account { nonce: 1, ..Default::default() },
            )
            .unwrap();
        provider.commit().unwrap();
        let (_targets, mut receiver) = watch::channel(target());
        let (run, _stop) = snap_run(&factory);

        let pass =
            run.rebuild_and_hand_off(&mut pipeline, &mut receiver, write, pivot).await.unwrap();

        assert_eq!(pass, Pass::RootMismatch);
        assert!(factory.provider().unwrap().active_snap_write().unwrap().is_none());
    }

    #[tokio::test]
    async fn repeated_root_mismatches_fail_the_run() {
        let (mut pipeline, factory, _, _) =
            handoff_ready(TestStage::new(StageId::Headers).add_exec(headers_done(PIVOT)));
        // State whose root differs from the one the pivot header commits to.
        let provider = factory.database_provider_rw().unwrap();
        provider
            .tx_ref()
            .put::<tables::HashedAccounts>(
                B256::repeat_byte(3),
                Account { nonce: 1, ..Default::default() },
            )
            .unwrap();
        provider.commit().unwrap();
        let (_targets, receiver) = watch::channel(target());
        let (run, _stop) = snap_run(&factory);

        let result = run.with_max_root_mismatches(1).bootstrap(&mut pipeline, receiver).await;

        assert!(matches!(result, Err(PipelineError::Internal(_))), "{result:?}");
        assert!(factory.provider().unwrap().active_snap_write().unwrap().is_none());
    }

    #[tokio::test]
    async fn a_chain_before_block_access_lists_syncs_with_the_staged_pipeline() {
        let factory = hashed_factory();
        let stages = StageSetBuilder::default()
            .add_stage(
                TestStage::new(StageId::Headers)
                    .add_exec(headers_done(0))
                    .add_exec(headers_done(0)),
            )
            .add_stage(TestStage::new(StageId::Finish).add_exec(headers_done(0)));
        let mut pipeline = pipeline_on(&factory, stages, watch::channel(B256::ZERO).0);
        insert_headers(&factory, &[SealedHeader::seal_slow(Header::default())]);
        let (_targets, receiver) = watch::channel(target());
        let (run, _stop) = snap_run(&factory);

        // The second header run is the staged pipeline finishing to the target.
        let result = run.bootstrap(&mut pipeline, receiver).await.unwrap();

        assert_ne!(result, STOPPED);
        assert!(factory.provider().unwrap().snap_attempt().unwrap().is_none());
    }

    #[tokio::test]
    async fn a_sibling_of_the_local_head_skips_the_header_pass() {
        // A header stage with no outputs, so running it fails the test.
        let (mut pipeline, factory) =
            pipeline_with(TestStage::new(StageId::Headers), watch::channel(B256::ZERO).0);
        let (run, _, mut receiver) = sibling_of_head(&factory);

        let pass = run.catch_up_headers(&mut pipeline, &mut receiver).await.unwrap();

        assert_eq!(pass, Pass::Done);
    }

    #[tokio::test]
    async fn a_sibling_of_the_local_head_finishes_to_the_local_head() {
        let factory = hashed_factory();
        let stages = StageSetBuilder::default()
            .add_stage(TestStage::new(StageId::Headers).add_exec(headers_done(1)))
            .add_stage(TestStage::new(StageId::Finish).add_exec(headers_done(1)));
        let (tip, pipeline_tip) = watch::channel(B256::ZERO);
        let mut pipeline = pipeline_on(&factory, stages, tip);
        let (run, head, mut receiver) = sibling_of_head(&factory);

        let result = run.finish(&mut pipeline, &mut receiver).await.unwrap();

        assert_ne!(result, STOPPED);
        assert_eq!(*pipeline_tip.borrow(), head);
    }

    #[tokio::test]
    async fn a_header_other_than_the_target_leaves_it_unresolved() {
        let factory = hashed_factory();
        let genesis = Header::default();
        insert_headers(&factory, &[SealedHeader::seal_slow(genesis.clone())]);
        // The peer answers with the genesis header, which isn't the requested target.
        let client = ServesHeaders { headers: vec![genesis], ..Default::default() };
        let (run, _stop) = snap_run_with(&factory, client);

        assert_eq!(run.is_ahead(target()).await.unwrap(), None);
    }

    #[tokio::test]
    async fn a_new_target_replaces_an_unresolved_one_before_headers_start() {
        let (tip, pipeline_tip) = watch::channel(B256::ZERO);
        let (mut pipeline, factory) =
            pipeline_with(TestStage::new(StageId::Headers).add_exec(headers_done(1)), tip);
        let (targets, mut receiver) = watch::channel(B256::repeat_byte(0xff));
        let (run, _stop) = snap_run(&factory);
        let mut pass = pin!(run.catch_up_headers(&mut pipeline, &mut receiver));

        assert!(futures::poll!(&mut pass).is_pending());
        assert_eq!(*pipeline_tip.borrow(), B256::ZERO);
        targets.send(next_target()).unwrap();

        let pass = tokio::time::timeout(Duration::from_secs(5), pass)
            .await
            .expect("the new target resolves without starting the old header pass");
        assert_eq!(pass.unwrap(), Pass::Done);
        assert_eq!(*pipeline_tip.borrow(), next_target());
    }

    #[tokio::test]
    async fn a_new_target_replaces_an_unresolved_one_before_finishing() {
        let factory = hashed_factory();
        let stages = StageSetBuilder::default()
            .add_stage(TestStage::new(StageId::Headers).add_exec(headers_done(1)))
            .add_stage(TestStage::new(StageId::Finish).add_exec(headers_done(1)));
        let (tip, pipeline_tip) = watch::channel(B256::ZERO);
        let mut pipeline = pipeline_on(&factory, stages, tip);
        let (targets, mut receiver) = watch::channel(B256::repeat_byte(0xff));
        let (run, _stop) = snap_run(&factory);
        let mut finished = pin!(run.finish(&mut pipeline, &mut receiver));

        assert!(futures::poll!(&mut finished).is_pending());
        assert_eq!(*pipeline_tip.borrow(), B256::ZERO);
        targets.send(next_target()).unwrap();

        let result = tokio::time::timeout(Duration::from_secs(5), finished)
            .await
            .expect("the new target resolves before the pipeline starts");
        assert_ne!(result.unwrap(), STOPPED);
        assert_eq!(*pipeline_tip.borrow(), next_target());
    }

    #[tokio::test]
    async fn a_new_target_interrupts_a_pending_header_lookup() {
        let (tip, pipeline_tip) = watch::channel(B256::ZERO);
        let (mut pipeline, factory) =
            pipeline_with(TestStage::new(StageId::Headers).add_exec(headers_done(1)), tip);
        let (targets, mut receiver) = watch::channel(target());
        let client = ServesHeaders { pending_target: Some(target()), ..Default::default() };
        let (run, _stop) = snap_run_with(&factory, client);
        let mut pass = pin!(run.catch_up_headers(&mut pipeline, &mut receiver));

        assert!(futures::poll!(&mut pass).is_pending());
        assert_eq!(*pipeline_tip.borrow(), B256::ZERO);
        targets.send(next_target()).unwrap();

        let pass = tokio::time::timeout(Duration::from_secs(5), pass)
            .await
            .expect("forkchoice interrupts the pending peer request");
        assert_eq!(pass.unwrap(), Pass::Done);
        assert_eq!(*pipeline_tip.borrow(), next_target());
    }

    #[tokio::test]
    async fn an_empty_response_retries_a_stale_target_without_starting_headers() {
        let (tip, pipeline_tip) = watch::channel(B256::ZERO);
        let (mut pipeline, factory) = pipeline_with(TestStage::new(StageId::Headers), tip);
        let (run, _, mut receiver) = sibling_of_head(&factory);
        run.client.empty_responses.store(1, Ordering::Relaxed);
        let mut pass = pin!(run.catch_up_headers(&mut pipeline, &mut receiver));

        assert!(futures::poll!(&mut pass).is_pending());
        assert_eq!(*pipeline_tip.borrow(), B256::ZERO);
        let pass = tokio::time::timeout(Duration::from_secs(5), pass)
            .await
            .expect("the same target is retried without another forkchoice update");

        assert_eq!(pass.unwrap(), Pass::Done);
        assert_eq!(*pipeline_tip.borrow(), B256::ZERO);
    }

    #[tokio::test]
    async fn stopping_an_unresolved_target_leaves_headers_untouched() {
        let (tip, pipeline_tip) = watch::channel(B256::ZERO);
        let (mut pipeline, factory) = pipeline_with(TestStage::new(StageId::Headers), tip);
        let (_targets, mut receiver) = watch::channel(target());
        let client = ServesHeaders { pending_target: Some(target()), ..Default::default() };
        let (run, stop) = snap_run_with(&factory, client);
        let mut pass = pin!(run.catch_up_headers(&mut pipeline, &mut receiver));

        assert!(futures::poll!(&mut pass).is_pending());
        stop.cancel();

        assert_eq!(pass.await.unwrap(), Pass::Stopped);
        assert_eq!(*pipeline_tip.borrow(), B256::ZERO);
    }

    // Headers through block 1 in `factory`, and a run whose forkchoice target is a sibling of
    // block 1 that peers serve. Returns the run, the local head's hash and the target receiver.
    fn sibling_of_head(
        factory: &ProviderFactory<MockNodeTypesWithDB>,
    ) -> (SnapRun<MockNodeTypesWithDB, ServesHeaders>, B256, watch::Receiver<B256>) {
        let genesis = SealedHeader::seal_slow(Header::default());
        let head = SealedHeader::seal_slow(Header {
            number: 1,
            parent_hash: genesis.hash(),
            ..Default::default()
        });
        insert_headers(factory, &[genesis.clone(), head.clone()]);
        let sibling =
            Header { number: 1, parent_hash: genesis.hash(), gas_limit: 1, ..Default::default() };
        let receiver = watch::channel(sibling.hash_slow()).1;
        let client = ServesHeaders { headers: vec![sibling], ..Default::default() };
        (snap_run_with(factory, client).0, head.hash(), receiver)
    }
}
