//! Support for building payloads.
//!
//! The payload builder is responsible for building payloads.
//! Once a new payload is created, it is continuously updated.

use crate::{
    metrics::PayloadBuilderServiceMetrics, traits::PayloadJobGenerator, KeepPayloadJobAlive,
    PayloadJob,
};
use alloy_consensus::BlockHeader;
use alloy_primitives::{BlockTimestamp, B256};
use alloy_rpc_types::engine::PayloadId;
use futures_util::{future::FutureExt, Stream, StreamExt};
use reth_chain_state::CanonStateNotification;
use reth_execution_cache::SavedCache;
use reth_payload_builder_primitives::{Events, PayloadBuilderError, PayloadEvents};
use reth_payload_primitives::{BuiltPayload, PayloadAttributes, PayloadKind, PayloadTypes};
use reth_primitives_traits::{FastInstant as Instant, NodePrimitives};
use reth_trie_parallel::state_root_task::PayloadStateRootHandle;
use std::{
    future::Future,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};
use tokio::sync::{
    broadcast, mpsc,
    oneshot::{self, Receiver},
    watch,
};
use tokio_stream::wrappers::UnboundedReceiverStream;
use tracing::{debug, debug_span, info, trace, warn, Span};

type PayloadFuture<P> = Pin<Box<dyn Future<Output = Result<P, PayloadBuilderError>> + Send>>;
type PayloadResponse<P> = oneshot::Sender<Option<PayloadFuture<P>>>;

/// A communication channel to the [`PayloadBuilderService`] that can retrieve payloads.
///
/// This type is intended to be used to retrieve payloads from the service (e.g. from the engine
/// API).
#[derive(Debug)]
pub struct PayloadStore<T: PayloadTypes> {
    inner: Arc<PayloadBuilderHandle<T>>,
}

impl<T> PayloadStore<T>
where
    T: PayloadTypes,
{
    /// Resolves the payload job and returns the best payload that has been built so far.
    ///
    /// Note: depending on the installed [`PayloadJobGenerator`], this may or may not terminate the
    /// job, See [`PayloadJob::resolve`].
    pub fn resolve_kind(
        &self,
        id: PayloadId,
        kind: PayloadKind,
    ) -> impl Future<Output = Option<Result<T::BuiltPayload, PayloadBuilderError>>> {
        self.inner.resolve_kind(id, kind)
    }

    /// Resolves the payload job and returns the best payload that has been built so far.
    pub async fn resolve(
        &self,
        id: PayloadId,
    ) -> Option<Result<T::BuiltPayload, PayloadBuilderError>> {
        self.resolve_kind(id, PayloadKind::Earliest).await
    }

    /// Returns the best payload for the given identifier.
    ///
    /// Note: this merely returns the best payload so far and does not resolve the job.
    pub async fn best_payload(
        &self,
        id: PayloadId,
    ) -> Option<Result<T::BuiltPayload, PayloadBuilderError>> {
        self.inner.best_payload(id).await
    }

    /// Returns the payload timestamp associated with the given identifier.
    ///
    /// Note: this returns the timestamp of the payload and does not resolve the job.
    pub async fn payload_timestamp(
        &self,
        id: PayloadId,
    ) -> Option<Result<u64, PayloadBuilderError>> {
        self.inner.payload_timestamp(id).await
    }

    /// Create a new instance
    pub fn new(inner: PayloadBuilderHandle<T>) -> Self {
        Self { inner: Arc::new(inner) }
    }
}

impl<T> From<PayloadBuilderHandle<T>> for PayloadStore<T>
where
    T: PayloadTypes,
{
    fn from(inner: PayloadBuilderHandle<T>) -> Self {
        Self::new(inner)
    }
}

/// A communication channel to the [`PayloadBuilderService`].
///
/// This is the API used to create new payloads and to get the current state of existing ones.
#[derive(Debug)]
pub struct PayloadBuilderHandle<T: PayloadTypes> {
    /// Sender half of the message channel to the [`PayloadBuilderService`].
    to_service: mpsc::UnboundedSender<PayloadServiceCommand<T>>,
}

impl<T: PayloadTypes> PayloadBuilderHandle<T> {
    /// Creates a new payload builder handle for the given channel.
    ///
    /// Note: this is only used internally by the [`PayloadBuilderService`] to manage the payload
    /// building flow See [`PayloadBuilderService::poll`] for implementation details.
    pub const fn new(to_service: mpsc::UnboundedSender<PayloadServiceCommand<T>>) -> Self {
        Self { to_service }
    }

    /// Sends a message to the service to start building a new payload for the given payload.
    ///
    /// Returns a receiver that will receive the payload id.
    pub fn send_new_payload(
        &self,
        input: BuildNewPayload<T::PayloadAttributes>,
    ) -> Receiver<Result<PayloadId, PayloadBuilderError>> {
        let (tx, rx) = oneshot::channel();
        let span = debug_span!(parent: Span::current(), "payload_job");
        let _ =
            self.to_service.send(PayloadServiceCommand::BuildNewPayload(input.into(), span, tx));
        rx
    }

    /// Returns the best payload for the given identifier.
    /// Note: this does not resolve the job if it's still in progress.
    pub async fn best_payload(
        &self,
        id: PayloadId,
    ) -> Option<Result<T::BuiltPayload, PayloadBuilderError>> {
        let (tx, rx) = oneshot::channel();
        self.to_service.send(PayloadServiceCommand::BestPayload(id, tx)).ok()?;
        rx.await.ok()?
    }

    /// Resolves the payload job and returns the best payload that has been built so far.
    ///
    /// # Cancellation safety
    ///
    /// Dropping the returned future does not consume the payload id. If the service has not started
    /// resolving yet, the payload job remains available. If a terminating resolve has started, the
    /// service drives it to completion and caches the result for a retry.
    pub fn resolve_kind(
        &self,
        id: PayloadId,
        kind: PayloadKind,
    ) -> impl Future<Output = Option<Result<T::BuiltPayload, PayloadBuilderError>>> {
        let (tx, rx) = oneshot::channel();
        let sent = self.to_service.send(PayloadServiceCommand::Resolve(id, kind, tx)).is_ok();
        async move {
            if !sent {
                return None
            }

            match rx.await.transpose()? {
                Ok(fut) => Some(fut.await),
                Err(e) => Some(Err(e.into())),
            }
        }
    }

    /// Sends a message to the service to subscribe to payload events.
    /// Returns a receiver that will receive them.
    pub async fn subscribe(&self) -> Result<PayloadEvents<T>, PayloadBuilderError> {
        let (tx, rx) = oneshot::channel();
        let _ = self.to_service.send(PayloadServiceCommand::Subscribe(tx));
        Ok(PayloadEvents { receiver: rx.await? })
    }

    /// Returns the payload timestamp associated with the given identifier.
    ///
    /// Note: this returns the timestamp of the payload and does not resolve the job.
    pub async fn payload_timestamp(
        &self,
        id: PayloadId,
    ) -> Option<Result<u64, PayloadBuilderError>> {
        let (tx, rx) = oneshot::channel();
        self.to_service.send(PayloadServiceCommand::PayloadTimestamp(id, tx)).ok()?;
        rx.await.ok()?
    }
}

impl<T> Clone for PayloadBuilderHandle<T>
where
    T: PayloadTypes,
{
    fn clone(&self) -> Self {
        Self { to_service: self.to_service.clone() }
    }
}

/// A service that manages payload building tasks.
///
/// This type is an endless future that manages the building of payloads.
///
/// It tracks active payloads and their build jobs that run in a worker pool.
///
/// By design, this type relies entirely on the [`PayloadJobGenerator`] to create new payloads and
/// does know nothing about how to build them, it just drives their jobs to completion.
#[derive(Debug)]
#[must_use = "futures do nothing unless you `.await` or poll them"]
pub struct PayloadBuilderService<Gen, St, T>
where
    T: PayloadTypes,
    Gen: PayloadJobGenerator,
    Gen::Job: PayloadJob<PayloadAttributes = T::PayloadAttributes>,
{
    /// The type that knows how to create new payloads.
    generator: Gen,
    /// All active payload jobs, each accompanied by its id and the caller's tracing span
    /// propagated across the channel so that poll and resolve work appears as children of the
    /// original Engine API request.
    payload_jobs: Vec<(Gen::Job, PayloadId, Span)>,
    /// Terminating payload resolutions that the service drives to completion so request
    /// cancellation cannot consume an advertised payload id.
    pending_resolutions: Vec<PendingPayloadResolution<T::BuiltPayload>>,
    /// Copy of the sender half, so new [`PayloadBuilderHandle`] can be created on demand.
    service_tx: mpsc::UnboundedSender<PayloadServiceCommand<T>>,
    /// Receiver half of the command channel.
    command_rx: UnboundedReceiverStream<PayloadServiceCommand<T>>,
    /// Metrics for the payload builder service
    metrics: PayloadBuilderServiceMetrics,
    /// Chain events notification stream
    chain_events: St,
    /// Payload events handler, used to broadcast and subscribe to payload events.
    payload_events: broadcast::Sender<Events<T>>,
    /// We retain latest resolved payload just to make sure that we can handle repeating
    /// requests for it gracefully.
    cached_payload_rx: watch::Receiver<Option<(PayloadId, BlockTimestamp, T::BuiltPayload)>>,
    /// Sender half of the cached payload channel.
    cached_payload_tx: watch::Sender<Option<(PayloadId, BlockTimestamp, T::BuiltPayload)>>,
}

/// A terminating payload resolution owned by the service until its result is cached.
#[derive(derive_more::Debug)]
struct PendingPayloadResolution<P> {
    /// Payload id being resolved.
    id: PayloadId,
    /// Timestamp retained after the payload job is removed.
    timestamp: Option<BlockTimestamp>,
    /// Span inherited from the payload job.
    span: Span,
    /// Resolve future taken from the terminating payload job.
    #[debug(skip)]
    future: PayloadFuture<P>,
    /// Requests waiting for the service-owned resolution.
    #[debug(skip)]
    responses: Vec<PayloadResponse<P>>,
}

const PAYLOAD_EVENTS_BUFFER_SIZE: usize = 20;

// === impl PayloadBuilderService ===

impl<Gen, St, T> PayloadBuilderService<Gen, St, T>
where
    T: PayloadTypes,
    Gen: PayloadJobGenerator,
    Gen::Job: PayloadJob<PayloadAttributes = T::PayloadAttributes>,
    <Gen::Job as PayloadJob>::BuiltPayload: Into<T::BuiltPayload>,
{
    /// Creates a new payload builder service and returns the [`PayloadBuilderHandle`] to interact
    /// with it.
    ///
    /// This also takes a stream of chain events that will be forwarded to the generator to apply
    /// additional logic when new state is committed. See also
    /// [`PayloadJobGenerator::on_new_state`].
    pub fn new(generator: Gen, chain_events: St) -> (Self, PayloadBuilderHandle<T>) {
        let (service_tx, command_rx) = mpsc::unbounded_channel();
        let (payload_events, _) = broadcast::channel(PAYLOAD_EVENTS_BUFFER_SIZE);

        let (cached_payload_tx, cached_payload_rx) = watch::channel(None);

        let service = Self {
            generator,
            payload_jobs: Vec::new(),
            pending_resolutions: Vec::new(),
            service_tx,
            command_rx: UnboundedReceiverStream::new(command_rx),
            metrics: Default::default(),
            chain_events,
            payload_events,
            cached_payload_rx,
            cached_payload_tx,
        };

        let handle = service.handle();
        (service, handle)
    }

    /// Returns a handle to the service.
    pub fn handle(&self) -> PayloadBuilderHandle<T> {
        PayloadBuilderHandle::new(self.service_tx.clone())
    }

    /// Create clone on `payload_events` sending handle that could be used by builder to produce
    /// additional events during block building
    pub fn payload_events_handle(&self) -> broadcast::Sender<Events<T>> {
        self.payload_events.clone()
    }

    /// Returns true if the given payload is currently being built.
    fn contains_payload(&self, id: PayloadId) -> bool {
        self.payload_jobs.iter().any(|(_, job_id, _)| *job_id == id) ||
            self.pending_resolutions.iter().any(|resolution| resolution.id == id)
    }

    /// Returns the best payload for the given identifier that has been built so far.
    fn best_payload(&self, id: PayloadId) -> Option<Result<T::BuiltPayload, PayloadBuilderError>> {
        let res = self
            .payload_jobs
            .iter()
            .find(|(_, job_id, _)| *job_id == id)
            .map(|(j, _, _)| j.best_payload().map(|p| p.into()));
        if let Some(Ok(ref best)) = res {
            self.metrics.set_best_revenue(best.block().number(), f64::from(best.fees()));
        }

        res
    }

    /// Resolves the payload for the given identifier.
    ///
    /// Returns `true` when a new service-owned resolution was started and should be polled before
    /// the service yields.
    fn resolve(
        &mut self,
        id: PayloadId,
        kind: PayloadKind,
        response: PayloadResponse<T::BuiltPayload>,
    ) -> bool {
        if response.is_closed() {
            debug!(target: "payload_builder", %id, "resolve request already dropped, keeping payload job");
            return false
        }

        let start = Instant::now();
        debug!(target: "payload_builder", %id, "resolving payload job");

        let cached_payload = self
            .cached_payload_rx
            .borrow()
            .as_ref()
            .filter(|(cached_id, _, _)| *cached_id == id)
            .map(|(_, _, payload)| payload.clone());
        if let Some(payload) = cached_payload {
            self.metrics.resolve_duration_seconds.record(start.elapsed());
            let _ = response.send(Some(Box::pin(core::future::ready(Ok(payload)))));
            return false
        }

        if let Some(resolution) =
            self.pending_resolutions.iter_mut().find(|resolution| resolution.id == id)
        {
            resolution.responses.push(response);
            return false
        }

        let Some(job) = self.payload_jobs.iter().position(|(_, job_id, _)| *job_id == id) else {
            let _ = response.send(None);
            return false
        };
        let (fut, keep_alive) = self.payload_jobs[job].0.resolve_kind(kind);
        let payload_timestamp = self.payload_jobs[job].0.payload_timestamp();
        let pending_timestamp = payload_timestamp.as_ref().ok().copied();

        // Since the fees will not be known until the payload future is resolved / awaited, we wrap
        // the future in a new future that will update the metrics.
        let resolved_metrics = self.metrics.clone();
        let payload_events = self.payload_events.clone();
        let cached_payload_tx = self.cached_payload_tx.clone();

        let fut = async move {
            let res = fut.await;
            resolved_metrics.resolve_duration_seconds.record(start.elapsed());
            if let Ok(payload) = &res {
                if payload_events.receiver_count() > 0 {
                    payload_events.send(Events::BuiltPayload(payload.clone().into())).ok();
                }

                if let Ok(timestamp) = payload_timestamp {
                    let _ = cached_payload_tx.send(Some((id, timestamp, payload.clone().into())));
                }

                resolved_metrics
                    .set_resolved_revenue(payload.block().number(), f64::from(payload.fees()));
            }
            res.map(|p| p.into())
        };

        let fut: PayloadFuture<T::BuiltPayload> = Box::pin(fut);
        if keep_alive == KeepPayloadJobAlive::Yes {
            let _ = response.send(Some(fut));
            return false
        }

        let (_job, _, span) = self.payload_jobs.swap_remove(job);
        self.metrics.set_active_jobs(self.payload_jobs.len());
        self.pending_resolutions.push(PendingPayloadResolution {
            id,
            timestamp: pending_timestamp,
            span,
            future: fut,
            responses: vec![response],
        });
        debug!(target: "payload_builder", %id, "service is completing terminating payload resolution");
        true
    }

    /// Returns the payload timestamp for the given payload.
    fn payload_timestamp(&self, id: PayloadId) -> Option<Result<u64, PayloadBuilderError>> {
        if let Some((cached_id, timestamp, _)) = *self.cached_payload_rx.borrow() &&
            cached_id == id
        {
            return Some(Ok(timestamp));
        }

        if let Some(timestamp) = self
            .pending_resolutions
            .iter()
            .find(|resolution| resolution.id == id)
            .and_then(|resolution| resolution.timestamp)
        {
            return Some(Ok(timestamp))
        }

        let timestamp = self
            .payload_jobs
            .iter()
            .find(|(_, job_id, _)| *job_id == id)
            .map(|(j, _, _)| j.payload_timestamp());

        if timestamp.is_none() {
            trace!(target: "payload_builder", %id, "no matching payload job found to get timestamp for");
        }

        timestamp
    }
}

impl<Gen, St, T, N> Future for PayloadBuilderService<Gen, St, T>
where
    T: PayloadTypes,
    N: NodePrimitives,
    Gen: PayloadJobGenerator + Unpin + 'static,
    <Gen as PayloadJobGenerator>::Job: Unpin + 'static,
    St: Stream<Item = CanonStateNotification<N>> + Send + Unpin + 'static,
    Gen::Job: PayloadJob<PayloadAttributes = T::PayloadAttributes>,
    <Gen::Job as PayloadJob>::BuiltPayload: Into<T::BuiltPayload>,
{
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        loop {
            // notify the generator of new chain events
            while let Poll::Ready(Some(new_head)) = this.chain_events.poll_next_unpin(cx) {
                this.generator.on_new_state(new_head);
            }

            // we poll all jobs first, so we always have the latest payload that we can report if
            // requests
            // we don't care about the order of the jobs, so we can just swap_remove them
            for idx in (0..this.payload_jobs.len()).rev() {
                let (mut job, id, job_span) = this.payload_jobs.swap_remove(idx);

                let poll_result = {
                    let _entered = job_span.enter();
                    job.poll_unpin(cx)
                };

                match poll_result {
                    Poll::Ready(Ok(_)) => {
                        this.metrics.set_active_jobs(this.payload_jobs.len());
                        trace!(target: "payload_builder", %id, "payload job finished");
                    }
                    Poll::Ready(Err(err)) => {
                        warn!(target: "payload_builder",%err, ?id, "Payload builder job failed; resolving payload");
                        this.metrics.inc_failed_jobs();
                        this.metrics.set_active_jobs(this.payload_jobs.len());
                    }
                    Poll::Pending => {
                        this.payload_jobs.push((job, id, job_span));
                    }
                }
            }

            // Poll terminating resolutions independently of their request futures. This ensures a
            // dropped Engine API request cannot cancel the only path to a retryable payload.
            for idx in (0..this.pending_resolutions.len()).rev() {
                let mut resolution = this.pending_resolutions.swap_remove(idx);
                let poll_result = {
                    let _entered = resolution.span.enter();
                    resolution.future.poll_unpin(cx)
                };

                match poll_result {
                    Poll::Ready(Ok(payload)) => {
                        debug!(target: "payload_builder", id=%resolution.id, "terminating payload resolution completed");
                        for response in resolution.responses {
                            if !response.is_closed() {
                                let _ = response
                                    .send(Some(Box::pin(core::future::ready(Ok(payload.clone())))));
                            }
                        }
                    }
                    Poll::Ready(Err(err)) => {
                        warn!(target: "payload_builder", %err, id=%resolution.id, "terminating payload resolution failed");
                        let mut err = Some(err);
                        for response in resolution.responses {
                            if response.is_closed() {
                                continue
                            }
                            let response_err =
                                err.take().unwrap_or(PayloadBuilderError::MissingPayload);
                            let _ = response
                                .send(Some(Box::pin(core::future::ready(Err(response_err)))));
                        }
                    }
                    Poll::Pending => this.pending_resolutions.push(resolution),
                }
            }

            // Marker for the exit condition. Newly created futures must be polled once before the
            // service yields so they can register their wakers.
            let mut poll_again = false;

            // drain all requests
            while let Poll::Ready(Some(cmd)) = this.command_rx.poll_next_unpin(cx) {
                match cmd {
                    PayloadServiceCommand::BuildNewPayload(input, job_span, tx) => {
                        let id = input.payload_id();
                        let mut res = Ok(id);
                        let parent = input.parent_hash;

                        if this.contains_payload(id) {
                            debug!(target: "payload_builder", %id, %parent, "Payload job already in progress, ignoring.");
                        } else {
                            let start = Instant::now();
                            let attributes = input.attributes.clone();
                            let job_result = {
                                let _entered = job_span.enter();
                                this.generator.new_payload_job(*input, id)
                            };

                            match job_result {
                                Ok(job) => {
                                    this.metrics.new_job_duration_seconds.record(start.elapsed());
                                    info!(target: "payload_builder", %id, %parent, "New payload job created");
                                    this.metrics.inc_initiated_jobs();
                                    poll_again = true;
                                    this.payload_jobs.push((job, id, job_span));
                                    this.payload_events.send(Events::Attributes(attributes)).ok();

                                    // Clear stale cached payload for this id so
                                    // resolve() never returns an outdated result
                                    // from a previous job with the same id.
                                    if this
                                        .cached_payload_rx
                                        .borrow()
                                        .as_ref()
                                        .is_some_and(|(cached_id, _, _)| *cached_id == id)
                                    {
                                        trace!(target: "payload_builder", %id, "clearing stale cached payload for reused payload id");
                                        let _ = this.cached_payload_tx.send(None);
                                    }
                                }
                                Err(err) => {
                                    this.metrics.new_job_duration_seconds.record(start.elapsed());
                                    this.metrics.inc_failed_jobs();
                                    warn!(target: "payload_builder", %err, %id, "Failed to create payload builder job");
                                    res = Err(err);
                                }
                            }
                        }

                        let _ = tx.send(res);
                    }
                    PayloadServiceCommand::BestPayload(id, tx) => {
                        let _ = tx.send(this.best_payload(id));
                    }
                    PayloadServiceCommand::PayloadTimestamp(id, tx) => {
                        let timestamp = this.payload_timestamp(id);
                        let _ = tx.send(timestamp);
                    }
                    PayloadServiceCommand::Resolve(id, strategy, tx) => {
                        poll_again |= this.resolve(id, strategy, tx);
                    }
                    PayloadServiceCommand::Subscribe(tx) => {
                        let new_rx = this.payload_events.subscribe();
                        let _ = tx.send(new_rx);
                    }
                }
            }

            if !poll_again {
                return Poll::Pending
            }
        }
    }
}

/// Message type for the [`PayloadBuilderService`].
#[derive(derive_more::Debug)]
pub enum PayloadServiceCommand<T: PayloadTypes> {
    /// Start building a new payload.
    ///
    /// Carries the caller's [`Span`] so the service can parent payload-building work under the
    /// originating Engine API trace.
    BuildNewPayload(
        Box<BuildNewPayload<T::PayloadAttributes>>,
        Span,
        oneshot::Sender<Result<PayloadId, PayloadBuilderError>>,
    ),
    /// Get the best payload so far
    BestPayload(PayloadId, oneshot::Sender<Option<Result<T::BuiltPayload, PayloadBuilderError>>>),
    /// Get the payload timestamp for the given payload
    PayloadTimestamp(PayloadId, oneshot::Sender<Option<Result<u64, PayloadBuilderError>>>),
    /// Resolve the payload and return the payload
    Resolve(
        PayloadId,
        /* kind: */ PayloadKind,
        #[debug(skip)] oneshot::Sender<Option<PayloadFuture<T::BuiltPayload>>>,
    ),
    /// Payload service events
    Subscribe(oneshot::Sender<broadcast::Receiver<Events<T>>>),
}

/// A request to build a new payload.
#[derive(Debug)]
pub struct BuildNewPayload<T> {
    /// The attributes for the new payload
    pub attributes: T,
    /// The parent hash of the new payload
    pub parent_hash: B256,
    /// Optional execution cache to use for the payload.
    ///
    /// Only provided if `--engine.share-execution-cache-with-payload-builder` is enabled.
    pub cache: Option<SavedCache>,
    /// Optional handle to a background state-root task.
    pub state_root_handle: Option<PayloadStateRootHandle>,
}

impl<T: PayloadAttributes> BuildNewPayload<T> {
    /// Returns the payload id for the new payload.
    pub fn payload_id(&self) -> PayloadId {
        self.attributes.payload_id(&self.parent_hash)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::Block;
    use alloy_primitives::{Address, U256};
    use futures_util::future::{poll_fn, BoxFuture};
    use reth_ethereum_engine_primitives::{EthBuiltPayload, EthEngineTypes, EthPayloadAttributes};
    use reth_primitives_traits::{Block as _, RecoveredBlock};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    #[derive(Clone, Debug)]
    struct MockPayloadJobGenerator {
        resolve_calls: Arc<AtomicUsize>,
        resolve_ready: Arc<AtomicBool>,
        resolve_fails: bool,
    }

    impl PayloadJobGenerator for MockPayloadJobGenerator {
        type Job = MockPayloadJob;

        fn new_payload_job(
            &self,
            input: BuildNewPayload<EthPayloadAttributes>,
            _id: PayloadId,
        ) -> Result<Self::Job, PayloadBuilderError> {
            Ok(MockPayloadJob {
                attr: input.attributes,
                resolve_calls: self.resolve_calls.clone(),
                resolve_ready: self.resolve_ready.clone(),
                resolve_fails: self.resolve_fails,
            })
        }
    }

    #[derive(Debug)]
    struct MockPayloadJob {
        attr: EthPayloadAttributes,
        resolve_calls: Arc<AtomicUsize>,
        resolve_ready: Arc<AtomicBool>,
        resolve_fails: bool,
    }

    impl Future for MockPayloadJob {
        type Output = Result<(), PayloadBuilderError>;

        fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
            Poll::Pending
        }
    }

    impl PayloadJob for MockPayloadJob {
        type PayloadAttributes = EthPayloadAttributes;
        type ResolvePayloadFuture =
            BoxFuture<'static, Result<EthBuiltPayload, PayloadBuilderError>>;
        type BuiltPayload = EthBuiltPayload;

        fn best_payload(&self) -> Result<EthBuiltPayload, PayloadBuilderError> {
            Ok(test_payload())
        }

        fn payload_attributes(&self) -> Result<EthPayloadAttributes, PayloadBuilderError> {
            Ok(self.attr.clone())
        }

        fn payload_timestamp(&self) -> Result<u64, PayloadBuilderError> {
            Ok(self.attr.timestamp)
        }

        fn resolve_kind(
            &mut self,
            _kind: PayloadKind,
        ) -> (Self::ResolvePayloadFuture, KeepPayloadJobAlive) {
            self.resolve_calls.fetch_add(1, Ordering::SeqCst);
            let resolve_ready = self.resolve_ready.clone();
            let resolve_fails = self.resolve_fails;
            let fut = poll_fn(move |_cx| {
                if resolve_ready.load(Ordering::SeqCst) {
                    if resolve_fails {
                        Poll::Ready(Err(PayloadBuilderError::MissingPayload))
                    } else {
                        Poll::Ready(Ok(test_payload()))
                    }
                } else {
                    Poll::Pending
                }
            });
            (Box::pin(fut), KeepPayloadJobAlive::No)
        }
    }

    #[test]
    fn dropped_resolve_before_service_processing_keeps_job_for_retry() {
        let resolve_calls = Arc::new(AtomicUsize::new(0));
        let resolve_ready = Arc::new(AtomicBool::new(true));
        let generator = MockPayloadJobGenerator {
            resolve_calls: resolve_calls.clone(),
            resolve_ready,
            resolve_fails: false,
        };
        let chain_events = futures_util::stream::empty::<CanonStateNotification>();
        let (mut service, handle) =
            PayloadBuilderService::<_, _, EthEngineTypes>::new(generator, chain_events);
        let mut cx = Context::from_waker(futures_util::task::noop_waker_ref());

        let payload_id = start_payload_job(&mut service, &handle, &mut cx);

        // The command is queued eagerly, then its caller disappears before the service sees it.
        drop(handle.resolve_kind(payload_id, PayloadKind::Earliest));
        assert!(Pin::new(&mut service).poll(&mut cx).is_pending());

        assert_eq!(resolve_calls.load(Ordering::SeqCst), 0);
        assert_eq!(service.payload_jobs.len(), 1);

        let mut retry = Box::pin(handle.resolve_kind(payload_id, PayloadKind::Earliest));
        assert!(Pin::new(&mut service).poll(&mut cx).is_pending());
        assert!(matches!(retry.as_mut().poll(&mut cx), Poll::Ready(Some(Ok(_)))));
        assert_eq!(resolve_calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn cancelled_in_progress_resolve_keeps_payload_id_resolvable() {
        let resolve_calls = Arc::new(AtomicUsize::new(0));
        let resolve_ready = Arc::new(AtomicBool::new(false));
        let generator = MockPayloadJobGenerator {
            resolve_calls: resolve_calls.clone(),
            resolve_ready: resolve_ready.clone(),
            resolve_fails: false,
        };
        let chain_events = futures_util::stream::empty::<CanonStateNotification>();
        let (mut service, handle) =
            PayloadBuilderService::<_, _, EthEngineTypes>::new(generator, chain_events);
        let mut cx = Context::from_waker(futures_util::task::noop_waker_ref());

        let payload_id = start_payload_job(&mut service, &handle, &mut cx);

        let mut first_resolve = Box::pin(handle.resolve_kind(payload_id, PayloadKind::Earliest));
        assert!(Pin::new(&mut service).poll(&mut cx).is_pending());
        assert!(first_resolve.as_mut().poll(&mut cx).is_pending());
        assert_eq!(resolve_calls.load(Ordering::SeqCst), 1);

        // Resolution has consumed the terminating job, but request cancellation must not consume
        // the advertised payload id or cancel the service-owned resolution.
        drop(first_resolve);

        let mut timestamp = Box::pin(handle.payload_timestamp(payload_id));
        assert!(timestamp.as_mut().poll(&mut cx).is_pending());
        assert!(Pin::new(&mut service).poll(&mut cx).is_pending());
        assert!(matches!(timestamp.as_mut().poll(&mut cx), Poll::Ready(Some(Ok(1)))));

        let mut retry = Box::pin(handle.resolve_kind(payload_id, PayloadKind::Earliest));
        assert!(Pin::new(&mut service).poll(&mut cx).is_pending());
        assert!(retry.as_mut().poll(&mut cx).is_pending());
        assert_eq!(resolve_calls.load(Ordering::SeqCst), 1);

        resolve_ready.store(true, Ordering::SeqCst);
        assert!(Pin::new(&mut service).poll(&mut cx).is_pending());
        assert!(matches!(retry.as_mut().poll(&mut cx), Poll::Ready(Some(Ok(_)))));

        let mut cached_retry = Box::pin(handle.resolve_kind(payload_id, PayloadKind::Earliest));
        assert!(Pin::new(&mut service).poll(&mut cx).is_pending());
        assert!(matches!(cached_retry.as_mut().poll(&mut cx), Poll::Ready(Some(Ok(_)))));
        assert_eq!(resolve_calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn failed_service_owned_resolve_clears_pending_state() {
        let resolve_calls = Arc::new(AtomicUsize::new(0));
        let resolve_ready = Arc::new(AtomicBool::new(true));
        let generator =
            MockPayloadJobGenerator { resolve_calls, resolve_ready, resolve_fails: true };
        let chain_events = futures_util::stream::empty::<CanonStateNotification>();
        let (mut service, handle) =
            PayloadBuilderService::<_, _, EthEngineTypes>::new(generator, chain_events);
        let mut cx = Context::from_waker(futures_util::task::noop_waker_ref());

        let payload_id = start_payload_job(&mut service, &handle, &mut cx);
        let mut resolve = Box::pin(handle.resolve_kind(payload_id, PayloadKind::Earliest));

        assert!(Pin::new(&mut service).poll(&mut cx).is_pending());
        assert!(matches!(resolve.as_mut().poll(&mut cx), Poll::Ready(Some(Err(_)))));
        assert!(service.pending_resolutions.is_empty());
        assert!(service.cached_payload_rx.borrow().is_none());
    }

    fn start_payload_job(
        service: &mut PayloadBuilderService<
            MockPayloadJobGenerator,
            futures_util::stream::Empty<CanonStateNotification>,
            EthEngineTypes,
        >,
        handle: &PayloadBuilderHandle<EthEngineTypes>,
        cx: &mut Context<'_>,
    ) -> PayloadId {
        let parent_hash = B256::ZERO;
        let attributes = test_payload_attributes();
        let payload_id = attributes.payload_id(&parent_hash);
        let mut new_payload = Box::pin(handle.send_new_payload(BuildNewPayload {
            attributes,
            parent_hash,
            cache: None,
            state_root_handle: None,
        }));

        assert!(Pin::new(&mut *service).poll(cx).is_pending());
        match new_payload.as_mut().poll(cx) {
            Poll::Ready(Ok(Ok(returned_id))) => assert_eq!(returned_id, payload_id),
            other => panic!("payload id should be returned for the build command: {other:?}"),
        }

        assert!(Pin::new(&mut *service).poll(cx).is_pending());
        assert_eq!(service.payload_jobs.len(), 1);
        payload_id
    }

    fn test_payload_attributes() -> EthPayloadAttributes {
        EthPayloadAttributes {
            timestamp: 1,
            prev_randao: B256::ZERO,
            suggested_fee_recipient: Address::ZERO,
            withdrawals: Some(vec![]),
            parent_beacon_block_root: Some(B256::ZERO),
            slot_number: None,
            target_gas_limit: None,
        }
    }

    fn test_payload() -> EthBuiltPayload {
        EthBuiltPayload::new(
            Arc::new(RecoveredBlock::new_sealed(Block::<_>::default().seal_slow(), vec![])),
            U256::ZERO,
            None,
            None,
        )
    }
}
