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
use reth_storage_api::{errors::provider::ProviderResult, StateProviderBox};
use reth_trie_parallel::state_root_task::StateRootHandle;
use std::{
    fmt,
    future::Future,
    pin::Pin,
    sync::{
        atomic::{AtomicU8, Ordering},
        Arc,
    },
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
    pub async fn resolve_kind(
        &self,
        id: PayloadId,
        kind: PayloadKind,
    ) -> Option<Result<T::BuiltPayload, PayloadBuilderError>> {
        self.inner.resolve_kind(id, kind).await
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

    /// Sends a message to the service to start building a payload against an explicit state anchor.
    ///
    /// This is intended for opt-in downstream builders that need to build against a supplied
    /// speculative state provider while making the parent validity dependency explicit.
    pub fn send_new_payload_with_state_anchor(
        &self,
        input: BuildNewPayload<T::PayloadAttributes>,
        state_anchor: PayloadStateAnchor,
    ) -> Receiver<Result<PayloadId, PayloadBuilderError>> {
        let (tx, rx) = oneshot::channel();
        let span = debug_span!(parent: Span::current(), "payload_job");
        let input = BuildNewPayloadWithState::new(input, state_anchor);
        let _ = self.to_service.send(PayloadServiceCommand::BuildNewPayloadWithState(
            input.into(),
            span,
            tx,
        ));
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
    pub async fn resolve_kind(
        &self,
        id: PayloadId,
        kind: PayloadKind,
    ) -> Option<Result<T::BuiltPayload, PayloadBuilderError>> {
        let (tx, rx) = oneshot::channel();
        self.to_service.send(PayloadServiceCommand::Resolve(id, kind, tx)).ok()?;
        match rx.await.transpose()? {
            Ok(fut) => Some(fut.await),
            Err(e) => Some(Err(e.into())),
        }
    }

    /// Same as [`Self::resolve_kind`] but returns the underlying future.
    pub async fn resolve_kind_fut(
        &self,
        id: PayloadId,
        kind: PayloadKind,
    ) -> Result<Option<PayloadFuture<T::BuiltPayload>>, PayloadBuilderError> {
        let (tx, rx) = oneshot::channel();
        self.to_service.send(PayloadServiceCommand::Resolve(id, kind, tx))?;
        rx.await.map_err(Into::into)
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
        self.payload_jobs.iter().any(|(_, job_id, _)| *job_id == id)
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

    /// Returns the best payload for the given identifier that has been built so far and terminates
    /// the job if requested.
    fn resolve(
        &mut self,
        id: PayloadId,
        kind: PayloadKind,
    ) -> Option<PayloadFuture<T::BuiltPayload>> {
        let start = Instant::now();
        debug!(target: "payload_builder", %id, "resolving payload job");

        if let Some((cached, _, payload)) = &*self.cached_payload_rx.borrow() &&
            *cached == id
        {
            self.metrics.resolve_duration_seconds.record(start.elapsed());
            return Some(Box::pin(core::future::ready(Ok(payload.clone()))));
        }

        let job = self.payload_jobs.iter().position(|(_, job_id, _)| *job_id == id)?;
        let (fut, keep_alive) = self.payload_jobs[job].0.resolve_kind(kind);
        let payload_timestamp = self.payload_jobs[job].0.payload_timestamp();

        if keep_alive == KeepPayloadJobAlive::No {
            let (_, id, _) = self.payload_jobs.swap_remove(job);
            debug!(target: "payload_builder", %id, "terminated resolved job");
        }

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

        Some(Box::pin(fut))
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

            // marker for exit condition
            let mut new_job = false;

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
                                    new_job = true;
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
                    PayloadServiceCommand::BuildNewPayloadWithState(input, job_span, tx) => {
                        let id = input.payload_id();
                        let mut res = Ok(id);
                        let parent = input.parent_hash();

                        if this.contains_payload(id) {
                            debug!(target: "payload_builder", %id, %parent, "Payload job already in progress, ignoring.");
                        } else {
                            let start = Instant::now();
                            let attributes = input.payload.attributes.clone();
                            let job_result = {
                                let _entered = job_span.enter();
                                this.generator.new_payload_job_with_state(*input, id)
                            };

                            match job_result {
                                Ok(job) => {
                                    this.metrics.new_job_duration_seconds.record(start.elapsed());
                                    info!(target: "payload_builder", %id, %parent, "New payload job created");
                                    this.metrics.inc_initiated_jobs();
                                    new_job = true;
                                    this.payload_jobs.push((job, id, job_span));
                                    this.payload_events.send(Events::Attributes(attributes)).ok();

                                    // Clear stale cached payload for this id so resolve() never
                                    // returns an outdated result from a previous job with the same
                                    // id.
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
                        let _ = tx.send(this.resolve(id, strategy));
                    }
                    PayloadServiceCommand::Subscribe(tx) => {
                        let new_rx = this.payload_events.subscribe();
                        let _ = tx.send(new_rx);
                    }
                }
            }

            if !new_job {
                return Poll::Pending;
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
    /// Start building a new payload against an explicit state anchor.
    BuildNewPayloadWithState(
        Box<BuildNewPayloadWithState<T::PayloadAttributes>>,
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
    /// Optional handle to a background sparse trie task.
    pub trie_handle: Option<StateRootHandle>,
}

impl<T: PayloadAttributes> BuildNewPayload<T> {
    /// Returns the payload id for the new payload.
    pub fn payload_id(&self) -> PayloadId {
        self.attributes.payload_id(&self.parent_hash)
    }
}

/// A request to build a new payload against an explicit state anchor.
#[derive(Debug)]
pub struct BuildNewPayloadWithState<T> {
    /// The original payload build request.
    pub payload: BuildNewPayload<T>,
    /// State anchor to use for the payload build.
    pub state_anchor: PayloadStateAnchor,
}

impl<T> BuildNewPayloadWithState<T> {
    /// Creates a new anchored payload build request.
    pub const fn new(payload: BuildNewPayload<T>, state_anchor: PayloadStateAnchor) -> Self {
        Self { payload, state_anchor }
    }

    /// Returns the parent hash of the new payload.
    pub const fn parent_hash(&self) -> B256 {
        self.payload.parent_hash
    }
}

impl<T: PayloadAttributes> BuildNewPayloadWithState<T> {
    /// Returns the payload id for the new payload.
    pub fn payload_id(&self) -> PayloadId {
        self.payload.payload_id()
    }
}

impl<T> From<BuildNewPayload<T>> for BuildNewPayloadWithState<T> {
    fn from(payload: BuildNewPayload<T>) -> Self {
        Self::new(payload, PayloadStateAnchor::Canonical)
    }
}

/// State anchor used by an opt-in payload build.
#[derive(Clone, Debug, Default)]
pub enum PayloadStateAnchor {
    /// Build against the canonical provider selected by the parent hash.
    #[default]
    Canonical,
    /// Build against an explicitly supplied speculative state.
    Speculative(SpeculativePayloadState),
}

impl PayloadStateAnchor {
    /// Returns `true` if this anchor is speculative.
    pub const fn is_speculative(&self) -> bool {
        matches!(self, Self::Speculative(_))
    }

    /// Returns the speculative state provider factory, if one was supplied.
    pub fn state_provider(&self) -> Option<&SpeculativeStateProvider> {
        match self {
            Self::Canonical => None,
            Self::Speculative(state) => state.state_provider.as_ref(),
        }
    }

    /// Returns the validity dependency for this state anchor.
    pub fn validity_dependency(&self) -> Option<&PayloadValidityToken> {
        match self {
            Self::Canonical => None,
            Self::Speculative(state) => state.validity_dependency.as_ref(),
        }
    }

    /// Returns `true` if this anchor's validity dependency has been invalidated.
    pub fn should_discard(&self) -> bool {
        self.validity_dependency().is_some_and(PayloadValidityToken::should_discard)
    }
}

/// Speculative parent state for an opt-in payload build.
#[derive(Clone, Debug)]
pub struct SpeculativePayloadState {
    /// Validity dependency that must resolve before the result is treated as publishable.
    pub validity_dependency: Option<PayloadValidityToken>,
    /// Factory for constructing the state provider used by each build attempt.
    pub state_provider: Option<SpeculativeStateProvider>,
}

impl SpeculativePayloadState {
    /// Creates a speculative state anchor with a parent validity dependency.
    pub const fn new(validity_dependency: PayloadValidityToken) -> Self {
        Self { validity_dependency: Some(validity_dependency), state_provider: None }
    }

    /// Attaches a state provider factory.
    pub fn with_state_provider(mut self, state_provider: SpeculativeStateProvider) -> Self {
        self.state_provider = Some(state_provider);
        self
    }
}

/// Factory for opening a state provider for a speculative payload build.
#[derive(Clone)]
pub struct SpeculativeStateProvider {
    label: Arc<str>,
    factory: Arc<dyn Fn() -> ProviderResult<StateProviderBox> + Send + Sync>,
}

impl SpeculativeStateProvider {
    /// Creates a new speculative state provider factory.
    pub fn new(
        label: impl Into<Arc<str>>,
        factory: impl Fn() -> ProviderResult<StateProviderBox> + Send + Sync + 'static,
    ) -> Self {
        Self { label: label.into(), factory: Arc::new(factory) }
    }

    /// Returns a human-readable label for diagnostics.
    pub fn label(&self) -> &str {
        &self.label
    }

    /// Opens a new state provider.
    pub fn open(&self) -> ProviderResult<StateProviderBox> {
        (self.factory)()
    }
}

impl fmt::Debug for SpeculativeStateProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SpeculativeStateProvider")
            .field("label", &self.label)
            .finish_non_exhaustive()
    }
}

/// Shared validity token for a speculative payload parent.
#[derive(Clone, Debug)]
pub struct PayloadValidityToken {
    parent_hash: B256,
    status: Arc<AtomicU8>,
}

impl PayloadValidityToken {
    const PENDING: u8 = 0;
    const VALID: u8 = 1;
    const INVALID: u8 = 2;

    /// Creates a pending validity token for a speculative parent.
    pub fn pending(parent_hash: B256) -> Self {
        Self { parent_hash, status: Arc::new(AtomicU8::new(Self::PENDING)) }
    }

    /// Returns the parent hash guarded by this token.
    pub const fn parent_hash(&self) -> B256 {
        self.parent_hash
    }

    /// Marks the parent as valid.
    pub fn mark_valid(&self) {
        self.status.store(Self::VALID, Ordering::Release);
    }

    /// Marks the parent as invalid.
    pub fn mark_invalid(&self) {
        self.status.store(Self::INVALID, Ordering::Release);
    }

    /// Returns the current validity status.
    pub fn status(&self) -> PayloadValidityStatus {
        match self.status.load(Ordering::Acquire) {
            Self::VALID => PayloadValidityStatus::Valid,
            Self::INVALID => PayloadValidityStatus::Invalid,
            _ => PayloadValidityStatus::Pending,
        }
    }

    /// Returns `true` if speculative work guarded by this token should be discarded.
    pub fn should_discard(&self) -> bool {
        matches!(self.status(), PayloadValidityStatus::Invalid)
    }
}

/// Current status of a speculative parent validity dependency.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PayloadValidityStatus {
    /// Parent validation has not resolved yet.
    Pending,
    /// Parent validation succeeded.
    Valid,
    /// Parent validation failed.
    Invalid,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{KeepPayloadJobAlive, PayloadJob, PayloadJobGenerator};
    use alloy_primitives::{Address, B256};
    use reth_chain_state::CanonStateNotification;
    use reth_ethereum_engine_primitives::{EthBuiltPayload, EthEngineTypes, EthPayloadAttributes};
    use reth_payload_primitives::PayloadKind;
    use std::{
        sync::{Arc, Mutex},
        task::Poll,
    };

    #[derive(Debug)]
    struct CapturingGenerator {
        captured_anchor: Arc<Mutex<Option<PayloadStateAnchor>>>,
    }

    impl PayloadJobGenerator for CapturingGenerator {
        type Job = CapturingJob;

        fn new_payload_job(
            &self,
            input: BuildNewPayload<EthPayloadAttributes>,
            id: PayloadId,
        ) -> Result<Self::Job, PayloadBuilderError> {
            self.new_payload_job_with_state(input.into(), id)
        }

        fn new_payload_job_with_state(
            &self,
            input: BuildNewPayloadWithState<EthPayloadAttributes>,
            _id: PayloadId,
        ) -> Result<Self::Job, PayloadBuilderError> {
            *self.captured_anchor.lock().unwrap() = Some(input.state_anchor);
            Ok(CapturingJob { attributes: input.payload.attributes })
        }
    }

    #[derive(Debug)]
    struct CapturingJob {
        attributes: EthPayloadAttributes,
    }

    impl Future for CapturingJob {
        type Output = Result<(), PayloadBuilderError>;

        fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
            Poll::Pending
        }
    }

    impl PayloadJob for CapturingJob {
        type PayloadAttributes = EthPayloadAttributes;
        type ResolvePayloadFuture =
            futures_util::future::Ready<Result<EthBuiltPayload, PayloadBuilderError>>;
        type BuiltPayload = EthBuiltPayload;

        fn best_payload(&self) -> Result<Self::BuiltPayload, PayloadBuilderError> {
            Err(PayloadBuilderError::MissingPayload)
        }

        fn payload_attributes(&self) -> Result<Self::PayloadAttributes, PayloadBuilderError> {
            Ok(self.attributes.clone())
        }

        fn payload_timestamp(&self) -> Result<u64, PayloadBuilderError> {
            Ok(self.attributes.timestamp)
        }

        fn resolve_kind(
            &mut self,
            _kind: PayloadKind,
        ) -> (Self::ResolvePayloadFuture, KeepPayloadJobAlive) {
            (
                futures_util::future::ready(Err(PayloadBuilderError::MissingPayload)),
                KeepPayloadJobAlive::No,
            )
        }
    }

    fn test_attributes() -> EthPayloadAttributes {
        EthPayloadAttributes {
            timestamp: 1,
            prev_randao: B256::ZERO,
            suggested_fee_recipient: Address::ZERO,
            withdrawals: None,
            parent_beacon_block_root: None,
            slot_number: None,
        }
    }

    #[test]
    fn send_new_payload_with_state_anchor_reaches_generator() {
        let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
        rt.block_on(async {
            let captured_anchor = Arc::new(Mutex::new(None));
            let generator = CapturingGenerator { captured_anchor: captured_anchor.clone() };
            let (service, handle) = PayloadBuilderService::<_, _, EthEngineTypes>::new(
                generator,
                futures_util::stream::empty::<CanonStateNotification>(),
            );
            let service_task = tokio::spawn(service);

            let parent_hash = B256::from([0x42; 32]);
            let token = PayloadValidityToken::pending(parent_hash);
            let state_anchor = PayloadStateAnchor::Speculative(
                SpeculativePayloadState::new(token.clone()).with_state_provider(
                    SpeculativeStateProvider::new("test-overlay", || {
                        panic!("test should not open the provider")
                    }),
                ),
            );
            let input = BuildNewPayload {
                attributes: test_attributes(),
                parent_hash,
                cache: None,
                trie_handle: None,
            };
            let expected_payload_id = input.payload_id();

            let payload_id = handle
                .send_new_payload_with_state_anchor(input, state_anchor)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(payload_id, expected_payload_id);

            let captured = captured_anchor.lock().unwrap().take().unwrap();
            assert!(captured.is_speculative());
            let captured_token = captured.validity_dependency().unwrap();
            assert_eq!(captured_token.parent_hash(), parent_hash);
            assert_eq!(captured.state_provider().unwrap().label(), "test-overlay");

            token.mark_invalid();
            assert!(captured_token.should_discard());

            service_task.abort();
        });
    }
}
