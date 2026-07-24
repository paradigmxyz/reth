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
type ResolvePayloadResult<P, Job> = (Option<PayloadFuture<P>>, Option<PayloadJobEntry<Job>>);

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
    /// The future returned by this method is not cancellation-safe. This method sends the resolve
    /// command before returning the future. If the returned future is dropped before the service
    /// has processed the command, the command is ignored and the payload job is kept alive, so
    /// the same payload id can be resolved again. If it is dropped after the service has started
    /// resolving, the response receiver is dropped and the job identified by `id` is cancelled.
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
    payload_jobs: Vec<PayloadJobEntry<Gen::Job>>,
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
        self.payload_jobs.iter().any(|entry| entry.id == id)
    }

    /// Returns the best payload for the given identifier that has been built so far.
    fn best_payload(&self, id: PayloadId) -> Option<Result<T::BuiltPayload, PayloadBuilderError>> {
        let res = self
            .payload_jobs
            .iter()
            .find(|entry| entry.id == id)
            .map(|entry| entry.job.best_payload().map(|payload| payload.into()));
        if let Some(Ok(ref best)) = res {
            self.metrics.set_best_revenue(best.block().number(), f64::from(best.fees()));
        }

        res
    }

    /// Returns the best payload for the given identifier that has been built so far.
    ///
    /// If the job should be terminated, this removes it from active polling and returns it so the
    /// caller can drop it after the response is sent.
    fn resolve(
        &mut self,
        id: PayloadId,
        kind: PayloadKind,
    ) -> ResolvePayloadResult<T::BuiltPayload, Gen::Job> {
        let start = Instant::now();
        debug!(target: "payload_builder", %id, "resolving payload job");

        if let Some((cached, _, payload)) = &*self.cached_payload_rx.borrow() &&
            *cached == id
        {
            self.metrics.resolve_duration_seconds.record(start.elapsed());
            return (Some(Box::pin(core::future::ready(Ok(payload.clone())))), None);
        }

        let Some(job) = self.payload_jobs.iter().position(|entry| entry.id == id) else {
            return (None, None)
        };
        let (fut, keep_alive) = self.payload_jobs[job].job.resolve_kind(kind);
        let payload_timestamp = self.payload_jobs[job].job.payload_timestamp();

        let mut resolved_job =
            (keep_alive == KeepPayloadJobAlive::No).then(|| self.payload_jobs.swap_remove(job));
        let leases = resolved_job
            .as_mut()
            .map(|entry| std::mem::take(&mut entry.leases))
            .unwrap_or_default();

        // Since the fees will not be known until the payload future is resolved / awaited, we wrap
        // the future in a new future that will update the metrics.
        let resolved_metrics = self.metrics.clone();
        let payload_events = self.payload_events.clone();
        let cached_payload_tx = self.cached_payload_tx.clone();

        let fut = async move {
            let _leases = leases;
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

        (Some(Box::pin(fut)), resolved_job)
    }

    /// Returns the payload timestamp for the given payload.
    fn payload_timestamp(&self, id: PayloadId) -> Option<Result<u64, PayloadBuilderError>> {
        if let Some((cached_id, timestamp, _)) = *self.cached_payload_rx.borrow() &&
            cached_id == id
        {
            return Some(Ok(timestamp));
        }

        let timestamp = self
            .payload_jobs
            .iter()
            .find(|entry| entry.id == id)
            .map(|entry| entry.job.payload_timestamp());

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
                let PayloadJobEntry { mut job, id, span, leases } =
                    this.payload_jobs.swap_remove(idx);

                let poll_result = {
                    let _entered = span.enter();
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
                        this.payload_jobs.push(PayloadJobEntry { job, id, span, leases });
                    }
                }
            }

            // marker for exit condition
            let mut new_job = false;

            // drain all requests
            while let Poll::Ready(Some(cmd)) = this.command_rx.poll_next_unpin(cx) {
                match cmd {
                    PayloadServiceCommand::BuildNewPayload(mut input, job_span, tx) => {
                        let id = input.payload_id();
                        let mut res = Ok(id);
                        let parent = input.parent_hash;

                        if this.contains_payload(id) {
                            debug!(target: "payload_builder", %id, %parent, "Payload job already in progress, ignoring.");
                        } else {
                            let start = Instant::now();
                            let attributes = input.attributes.clone();
                            let leases = input.resources.take_leases();
                            let job_result = {
                                let _entered = job_span.enter();
                                this.generator.new_payload_job(*input, id)
                            };

                            match job_result {
                                Ok(job) => {
                                    this.metrics.new_job_duration_seconds.record(start.elapsed());
                                    info!(target: "payload_builder", %id, %parent, "New payload job created");
                                    this.metrics.inc_initiated_jobs();
                                    new_job = true;
                                    this.payload_jobs.push(PayloadJobEntry {
                                        job,
                                        id,
                                        span: job_span,
                                        leases,
                                    });
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
                        // If the caller dropped the request before the command was processed,
                        // resolving would consume the job without any way to deliver or cache
                        // the payload, and a retry of the advertised payload id would fail.
                        // Skip resolution so the job and any in-progress build are kept for a
                        // retry, see <https://github.com/paradigmxyz/reth/issues/26302>
                        if tx.is_closed() {
                            debug!(
                                target: "payload_builder", %id,
                                "resolve request already dropped, keeping payload job"
                            );
                        } else {
                            let (payload_fut, resolved_job) = this.resolve(id, strategy);
                            let _ = tx.send(payload_fut);

                            if let Some(entry) = resolved_job {
                                debug!(target: "payload_builder", id = %entry.id, "terminated resolved job");
                            }
                        }
                    }
                    PayloadServiceCommand::Subscribe(tx) => {
                        let new_rx = this.payload_events.subscribe();
                        let _ = tx.send(new_rx);
                    }
                }
            }

            if !new_job {
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
    /// Resources loaned to the payload builder for this job.
    pub resources: PayloadBuilderResources,
}

impl<T: PayloadAttributes> BuildNewPayload<T> {
    /// Returns the payload id for the new payload.
    pub fn payload_id(&self) -> PayloadId {
        self.attributes.payload_id(&self.parent_hash)
    }
}

/// Resources loaned to a payload builder job by the engine.
#[derive(Debug, Default)]
pub struct PayloadBuilderResources {
    /// Optional execution cache to use for the payload.
    ///
    /// Only provided if `--engine.share-execution-cache-with-payload-builder` is enabled.
    execution_cache: Option<SavedCache>,
    /// Optional handle to a background state-root task.
    state_root_handle: Option<PayloadStateRootHandle>,
    /// Lifecycle leases retained by the service while the payload job is active.
    leases: Vec<PayloadBuilderLease>,
}

impl PayloadBuilderResources {
    /// Creates a new payload builder resource bundle.
    pub const fn new(
        execution_cache: Option<SavedCache>,
        state_root_handle: Option<PayloadStateRootHandle>,
    ) -> Self {
        Self { execution_cache, state_root_handle, leases: Vec::new() }
    }

    /// Adds a lease that remains active for the lifetime of the payload job.
    pub fn with_lease(mut self, lease: PayloadBuilderLease) -> Self {
        self.leases.push(lease);
        self
    }

    /// Returns the loaned execution cache, if any.
    pub const fn execution_cache(&self) -> Option<&SavedCache> {
        self.execution_cache.as_ref()
    }

    /// Takes the loaned execution cache, if any.
    pub const fn take_execution_cache(&mut self) -> Option<SavedCache> {
        self.execution_cache.take()
    }

    /// Returns the loaned state-root task handle, if any.
    pub const fn state_root_handle(&self) -> Option<&PayloadStateRootHandle> {
        self.state_root_handle.as_ref()
    }

    /// Takes the loaned state-root task handle, if any.
    pub const fn take_state_root_handle(&mut self) -> Option<PayloadStateRootHandle> {
        self.state_root_handle.take()
    }

    /// Takes the lifecycle leases that the service must retain for this job.
    fn take_leases(&mut self) -> Vec<PayloadBuilderLease> {
        std::mem::take(&mut self.leases)
    }
}

/// Keeps a loaned resource active for the lifetime of a payload job.
pub struct PayloadBuilderLease {
    _lease: Box<dyn Send>,
}

impl PayloadBuilderLease {
    /// Wraps a lease that releases its resource when dropped.
    pub fn new(lease: impl Send + 'static) -> Self {
        Self { _lease: Box::new(lease) }
    }
}

impl std::fmt::Debug for PayloadBuilderLease {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PayloadBuilderLease").finish_non_exhaustive()
    }
}

/// An active payload job and its service metadata.
#[derive(Debug)]
struct PayloadJobEntry<Job> {
    job: Job,
    id: PayloadId,
    span: Span,
    leases: Vec<PayloadBuilderLease>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::test_payload_service;
    use alloy_consensus::Block;
    use alloy_primitives::{Address, U256};
    use futures_util::future::BoxFuture;
    use reth_ethereum_engine_primitives::{EthBuiltPayload, EthEngineTypes, EthPayloadAttributes};
    use reth_primitives_traits::{Block as _, RecoveredBlock};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    struct DropProbe(Arc<AtomicBool>);

    impl Drop for DropProbe {
        fn drop(&mut self) {
            self.0.store(true, Ordering::Release);
        }
    }

    #[test]
    fn payload_builder_lease_is_held_until_resolve_finishes() {
        tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(async {
            let (service, handle) = test_payload_service::<EthEngineTypes>();
            let service = tokio::spawn(service);
            let dropped = Arc::new(AtomicBool::new(false));
            let lease = PayloadBuilderLease::new(DropProbe(Arc::clone(&dropped)));
            let input = BuildNewPayload {
                attributes: EthPayloadAttributes {
                    timestamp: 1,
                    prev_randao: B256::ZERO,
                    suggested_fee_recipient: Address::ZERO,
                    withdrawals: None,
                    parent_beacon_block_root: None,
                    slot_number: None,
                    target_gas_limit: None,
                },
                parent_hash: B256::ZERO,
                resources: PayloadBuilderResources::default().with_lease(lease),
            };

            let id = handle.send_new_payload(input).await.unwrap().unwrap();
            assert!(!dropped.load(Ordering::Acquire));

            handle.resolve_kind(id, PayloadKind::Earliest).await.unwrap().unwrap();
            assert!(dropped.load(Ordering::Acquire));
            service.abort();
        });
    }

    #[derive(Clone, Debug)]
    struct MockPayloadJobGenerator {
        resolve_calls: Arc<AtomicUsize>,
    }

    impl PayloadJobGenerator for MockPayloadJobGenerator {
        type Job = MockPayloadJob;

        fn new_payload_job(
            &self,
            input: BuildNewPayload<EthPayloadAttributes>,
            _id: PayloadId,
        ) -> Result<Self::Job, PayloadBuilderError> {
            Ok(MockPayloadJob { attr: input.attributes, resolve_calls: self.resolve_calls.clone() })
        }
    }

    #[derive(Debug)]
    struct MockPayloadJob {
        attr: EthPayloadAttributes,
        resolve_calls: Arc<AtomicUsize>,
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
            (Box::pin(futures_util::future::ready(Ok(test_payload()))), KeepPayloadJobAlive::No)
        }
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

    /// A resolve request whose caller went away before the service processed the command must
    /// not consume the payload job: a retry for the same advertised payload id resolves
    /// normally.
    ///
    /// Regression test for the unsent-receiver window of
    /// <https://github.com/paradigmxyz/reth/issues/26302>.
    #[test]
    fn dropped_resolve_request_keeps_payload_job_for_retry() {
        let resolve_calls = Arc::new(AtomicUsize::new(0));
        let generator = MockPayloadJobGenerator { resolve_calls: resolve_calls.clone() };
        let chain_events = futures_util::stream::empty::<CanonStateNotification>();
        let (mut service, handle) =
            PayloadBuilderService::<_, _, EthEngineTypes>::new(generator, chain_events);

        let mut cx = Context::from_waker(futures_util::task::noop_waker_ref());
        let parent_hash = B256::ZERO;
        let attributes = test_payload_attributes();
        let payload_id = attributes.payload_id(&parent_hash);

        let mut new_payload = Box::pin(handle.send_new_payload(BuildNewPayload {
            attributes,
            parent_hash,
            resources: PayloadBuilderResources::default(),
        }));

        assert!(Pin::new(&mut service).poll(&mut cx).is_pending());
        match new_payload.as_mut().poll(&mut cx) {
            Poll::Ready(Ok(Ok(returned_id))) => assert_eq!(returned_id, payload_id),
            other => panic!("payload id should be returned for the build command: {other:?}"),
        }

        assert!(Pin::new(&mut service).poll(&mut cx).is_pending());
        assert_eq!(service.payload_jobs.len(), 1);

        // the caller drops the resolve request before the service processes the command
        drop(handle.resolve_kind(payload_id, PayloadKind::Earliest));
        assert!(Pin::new(&mut service).poll(&mut cx).is_pending());

        // the command is skipped: the job was neither resolved nor consumed
        assert_eq!(resolve_calls.load(Ordering::SeqCst), 0);
        assert_eq!(service.payload_jobs.len(), 1);

        // a retry for the same payload id resolves normally
        let mut retry = Box::pin(handle.resolve_kind(payload_id, PayloadKind::Earliest));
        assert!(Pin::new(&mut service).poll(&mut cx).is_pending());
        match retry.as_mut().poll(&mut cx) {
            Poll::Ready(Some(Ok(_payload))) => {}
            other => panic!("retry should resolve the kept payload job: {other:?}"),
        }

        assert_eq!(resolve_calls.load(Ordering::SeqCst), 1);
        assert!(service.payload_jobs.is_empty());
    }
}
