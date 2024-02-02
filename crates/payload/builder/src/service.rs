//! Support for building payloads.
//!
//! The payload builder is responsible for building payloads.
//! Once a new payload is created, it is continuously updated.

use crate::{
    error::PayloadBuilderError, metrics::PayloadBuilderServiceMetrics, traits::PayloadJobGenerator,
    KeepPayloadJobAlive, PayloadJob,
};
use futures_util::{future::FutureExt, Stream, StreamExt};
use reth_node_api::{BuiltPayload, EngineTypes, PayloadBuilderAttributes};
use reth_provider::CanonStateNotification;
use reth_rpc_types::engine::PayloadId;
use std::{
    fmt,
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};
use tokio::sync::{mpsc, oneshot};
use tokio_stream::wrappers::UnboundedReceiverStream;
use tracing::{debug, info, trace, warn};

/// A communication channel to the [PayloadBuilderService] that can retrieve payloads.
#[derive(Debug, Clone)]
pub struct PayloadStore<Engine: EngineTypes> {
    inner: PayloadBuilderHandle<Engine>,
}

// === impl PayloadStore ===

impl<Engine> PayloadStore<Engine>
where
    Engine: EngineTypes,
{
    /// Resolves the payload job and returns the best payload that has been built so far.
    ///
    /// Note: depending on the installed [PayloadJobGenerator], this may or may not terminate the
    /// job, See [PayloadJob::resolve].
    pub async fn resolve(
        &self,
        id: PayloadId,
    ) -> Option<Result<Engine::BuiltPayload, PayloadBuilderError>> {
        self.inner.resolve(id).await
    }

    /// Returns the best payload for the given identifier.
    ///
    /// Note: this merely returns the best payload so far and does not resolve the job.
    pub async fn best_payload(
        &self,
        id: PayloadId,
    ) -> Option<Result<Engine::BuiltPayload, PayloadBuilderError>> {
        self.inner.best_payload(id).await
    }

    /// Returns the payload attributes associated with the given identifier.
    ///
    /// Note: this returns the attributes of the payload and does not resolve the job.
    pub async fn payload_attributes(
        &self,
        id: PayloadId,
    ) -> Option<Result<Engine::PayloadBuilderAttributes, PayloadBuilderError>> {
        self.inner.payload_attributes(id).await
    }
}

impl<Engine> From<PayloadBuilderHandle<Engine>> for PayloadStore<Engine>
where
    Engine: EngineTypes,
{
    fn from(inner: PayloadBuilderHandle<Engine>) -> Self {
        Self { inner }
    }
}

/// A communication channel to the [PayloadBuilderService].
///
/// This is the API used to create new payloads and to get the current state of existing ones.
#[derive(Debug, Clone)]
pub struct PayloadBuilderHandle<Engine: EngineTypes> {
    /// Sender half of the message channel to the [PayloadBuilderService].
    to_service: mpsc::UnboundedSender<PayloadServiceCommand<Engine>>,
}

// === impl PayloadBuilderHandle ===

impl<Engine> PayloadBuilderHandle<Engine>
where
    Engine: EngineTypes,
{
    /// Creates a new payload builder handle for the given channel.
    ///
    /// Note: this is only used internally by the [PayloadBuilderService] to manage the payload
    /// building flow See [PayloadBuilderService::poll] for implementation details.
    pub fn new(to_service: mpsc::UnboundedSender<PayloadServiceCommand<Engine>>) -> Self {
        Self { to_service }
    }

    /// Resolves the payload job and returns the best payload that has been built so far.
    ///
    /// Note: depending on the installed [PayloadJobGenerator], this may or may not terminate the
    /// job, See [PayloadJob::resolve].
    async fn resolve(
        &self,
        id: PayloadId,
    ) -> Option<Result<Engine::BuiltPayload, PayloadBuilderError>> {
        let (tx, rx) = oneshot::channel();
        self.to_service.send(PayloadServiceCommand::Resolve(id, tx)).ok()?;
        match rx.await.transpose()? {
            Ok(fut) => Some(fut.await),
            Err(e) => Some(Err(e.into())),
        }
    }

    /// Returns the best payload for the given identifier.
    async fn best_payload(
        &self,
        id: PayloadId,
    ) -> Option<Result<Engine::BuiltPayload, PayloadBuilderError>> {
        let (tx, rx) = oneshot::channel();
        self.to_service.send(PayloadServiceCommand::BestPayload(id, tx)).ok()?;
        rx.await.ok()?
    }

    /// Returns the payload attributes associated with the given identifier.
    ///
    /// Note: this returns the attributes of the payload and does not resolve the job.
    async fn payload_attributes(
        &self,
        id: PayloadId,
    ) -> Option<Result<Engine::PayloadBuilderAttributes, PayloadBuilderError>> {
        let (tx, rx) = oneshot::channel();
        self.to_service.send(PayloadServiceCommand::PayloadAttributes(id, tx)).ok()?;
        rx.await.ok()?
    }

    /// Sends a message to the service to start building a new payload for the given payload.
    ///
    /// This is the same as [PayloadBuilderHandle::new_payload] but does not wait for the result and
    /// returns the receiver instead
    pub fn send_new_payload(
        &self,
        attr: Engine::PayloadBuilderAttributes,
    ) -> oneshot::Receiver<Result<PayloadId, PayloadBuilderError>> {
        let (tx, rx) = oneshot::channel();
        let _ = self.to_service.send(PayloadServiceCommand::BuildNewPayload(attr, tx));
        rx
    }

    /// Starts building a new payload for the given payload attributes.
    ///
    /// Returns the identifier of the payload.
    ///
    /// Note: if there's already payload in progress with same identifier, it will be returned.
    pub async fn new_payload(
        &self,
        attr: Engine::PayloadBuilderAttributes,
    ) -> Result<PayloadId, PayloadBuilderError> {
        self.send_new_payload(attr).await?
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
pub struct PayloadBuilderService<Gen, St, Engine>
where
    Engine: EngineTypes,
    Gen: PayloadJobGenerator,
    Gen::Job: PayloadJob<PayloadAttributes = Engine::PayloadBuilderAttributes>,
{
    /// The type that knows how to create new payloads.
    generator: Gen,
    /// All active payload jobs.
    payload_jobs: Vec<(Gen::Job, PayloadId)>,
    /// Copy of the sender half, so new [`PayloadBuilderHandle`] can be created on demand.
    service_tx: mpsc::UnboundedSender<PayloadServiceCommand<Engine>>,
    /// Receiver half of the command channel.
    command_rx: UnboundedReceiverStream<PayloadServiceCommand<Engine>>,
    /// Metrics for the payload builder service
    metrics: PayloadBuilderServiceMetrics,
    /// Chain events notification stream
    chain_events: St,
}

// === impl PayloadBuilderService ===

impl<Gen, St, Engine> PayloadBuilderService<Gen, St, Engine>
where
    Engine: EngineTypes,
    Gen: PayloadJobGenerator,
    Gen::Job: PayloadJob<PayloadAttributes = Engine::PayloadBuilderAttributes>,
    <Gen::Job as PayloadJob>::BuiltPayload: Into<Engine::BuiltPayload>,
{
    /// Creates a new payload builder service and returns the [PayloadBuilderHandle] to interact
    /// with it.
    ///
    /// This also takes a stream of chain events that will be forwarded to the generator to apply
    /// additional logic when new state is committed. See also [PayloadJobGenerator::on_new_state].
    pub fn new(generator: Gen, chain_events: St) -> (Self, PayloadBuilderHandle<Engine>) {
        let (service_tx, command_rx) = mpsc::unbounded_channel();
        let service = Self {
            generator,
            payload_jobs: Vec::new(),
            service_tx,
            command_rx: UnboundedReceiverStream::new(command_rx),
            metrics: Default::default(),
            chain_events,
        };

        let handle = service.handle();
        (service, handle)
    }

    /// Returns a handle to the service.
    pub fn handle(&self) -> PayloadBuilderHandle<Engine> {
        PayloadBuilderHandle::new(self.service_tx.clone())
    }

    /// Returns true if the given payload is currently being built.
    fn contains_payload(&self, id: PayloadId) -> bool {
        self.payload_jobs.iter().any(|(_, job_id)| *job_id == id)
    }

    /// Returns the best payload for the given identifier that has been built so far.
    fn best_payload(
        &self,
        id: PayloadId,
    ) -> Option<Result<Engine::BuiltPayload, PayloadBuilderError>> {
        let res = self
            .payload_jobs
            .iter()
            .find(|(_, job_id)| *job_id == id)
            .map(|(j, _)| j.best_payload().map(|p| p.into()));
        if let Some(Ok(ref best)) = res {
            self.metrics.set_best_revenue(best.block().number, f64::from(best.fees()));
        }

        res
    }

    /// Returns the best payload for the given identifier that has been built so far and terminates
    /// the job if requested.
    fn resolve(&mut self, id: PayloadId) -> Option<PayloadFuture<Engine::BuiltPayload>> {
        trace!(%id, "resolving payload job");

        let job = self.payload_jobs.iter().position(|(_, job_id)| *job_id == id)?;
        let (fut, keep_alive) = self.payload_jobs[job].0.resolve();

        if keep_alive == KeepPayloadJobAlive::No {
            let (_, id) = self.payload_jobs.remove(job);
            trace!(%id, "terminated resolved job");
        }

        // Since the fees will not be known until the payload future is resolved / awaited, we wrap
        // the future in a new future that will update the metrics.
        let resolved_metrics = self.metrics.clone();
        let fut = async move {
            let res = fut.await;
            if let Ok(ref payload) = res {
                resolved_metrics
                    .set_resolved_revenue(payload.block().number, f64::from(payload.fees()));
            }
            res.map(|p| p.into())
        };

        Some(Box::pin(fut))
    }
}

impl<Gen, St, Engine> PayloadBuilderService<Gen, St, Engine>
where
    Engine: EngineTypes,
    Gen: PayloadJobGenerator,
    Gen::Job: PayloadJob<PayloadAttributes = Engine::PayloadBuilderAttributes>,
    <Gen::Job as PayloadJob>::BuiltPayload: Into<Engine::BuiltPayload>,
{
    /// Returns the payload attributes for the given payload.
    fn payload_attributes(
        &self,
        id: PayloadId,
    ) -> Option<Result<<Gen::Job as PayloadJob>::PayloadAttributes, PayloadBuilderError>> {
        let attributes = self
            .payload_jobs
            .iter()
            .find(|(_, job_id)| *job_id == id)
            .map(|(j, _)| j.payload_attributes());

        if attributes.is_none() {
            trace!(%id, "no matching payload job found to get attributes for");
        }

        attributes
    }
}

impl<Gen, St, Engine> Future for PayloadBuilderService<Gen, St, Engine>
where
    Engine: EngineTypes,
    Gen: PayloadJobGenerator + Unpin + 'static,
    <Gen as PayloadJobGenerator>::Job: Unpin + 'static,
    St: Stream<Item = CanonStateNotification> + Send + Unpin + 'static,
    Gen::Job: PayloadJob<PayloadAttributes = Engine::PayloadBuilderAttributes>,
    <Gen::Job as PayloadJob>::BuiltPayload: Into<Engine::BuiltPayload>,
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
                let (mut job, id) = this.payload_jobs.swap_remove(idx);

                // drain better payloads from the job
                match job.poll_unpin(cx) {
                    Poll::Ready(Ok(_)) => {
                        this.metrics.set_active_jobs(this.payload_jobs.len());
                        trace!(%id, "payload job finished");
                    }
                    Poll::Ready(Err(err)) => {
                        warn!(?err, ?id, "Payload builder job failed; resolving payload");
                        this.metrics.inc_failed_jobs();
                        this.metrics.set_active_jobs(this.payload_jobs.len());
                    }
                    Poll::Pending => {
                        // still pending, put it back
                        this.payload_jobs.push((job, id));
                    }
                }
            }

            // marker for exit condition
            let mut new_job = false;

            // drain all requests
            while let Poll::Ready(Some(cmd)) = this.command_rx.poll_next_unpin(cx) {
                match cmd {
                    PayloadServiceCommand::BuildNewPayload(attr, tx) => {
                        let id = attr.payload_id();
                        let mut res = Ok(id);

                        if this.contains_payload(id) {
                            debug!(%id, parent = %attr.parent(), "Payload job already in progress, ignoring.");
                        } else {
                            // no job for this payload yet, create one
                            let parent = attr.parent();
                            match this.generator.new_payload_job(attr) {
                                Ok(job) => {
                                    info!(%id, %parent, "New payload job created");
                                    this.metrics.inc_initiated_jobs();
                                    new_job = true;
                                    this.payload_jobs.push((job, id));
                                }
                                Err(err) => {
                                    this.metrics.inc_failed_jobs();
                                    warn!(?err, %id, "Failed to create payload builder job");
                                    res = Err(err);
                                }
                            }
                        }

                        // return the id of the payload
                        let _ = tx.send(res);
                    }
                    PayloadServiceCommand::BestPayload(id, tx) => {
                        let _ = tx.send(this.best_payload(id));
                    }
                    PayloadServiceCommand::PayloadAttributes(id, tx) => {
                        let _ = tx.send(this.payload_attributes(id));
                    }
                    PayloadServiceCommand::Resolve(id, tx) => {
                        let _ = tx.send(this.resolve(id));
                    }
                }
            }

            if !new_job {
                return Poll::Pending
            }
        }
    }
}

// TODO: make generic over built payload type
type PayloadFuture<P> = Pin<Box<dyn Future<Output = Result<P, PayloadBuilderError>> + Send + Sync>>;

/// Message type for the [PayloadBuilderService].
pub enum PayloadServiceCommand<Engine: EngineTypes> {
    /// Start building a new payload.
    BuildNewPayload(
        Engine::PayloadBuilderAttributes,
        oneshot::Sender<Result<PayloadId, PayloadBuilderError>>,
    ),
    /// Get the best payload so far
    BestPayload(
        PayloadId,
        oneshot::Sender<Option<Result<Engine::BuiltPayload, PayloadBuilderError>>>,
    ),
    /// Get the payload attributes for the given payload
    PayloadAttributes(
        PayloadId,
        oneshot::Sender<Option<Result<Engine::PayloadBuilderAttributes, PayloadBuilderError>>>,
    ),
    /// Resolve the payload and return the payload
    Resolve(PayloadId, oneshot::Sender<Option<PayloadFuture<Engine::BuiltPayload>>>),
}

impl<Engine> fmt::Debug for PayloadServiceCommand<Engine>
where
    Engine: EngineTypes,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PayloadServiceCommand::BuildNewPayload(f0, f1) => {
                f.debug_tuple("BuildNewPayload").field(&f0).field(&f1).finish()
            }
            PayloadServiceCommand::BestPayload(f0, f1) => {
                f.debug_tuple("BestPayload").field(&f0).field(&f1).finish()
            }
            PayloadServiceCommand::PayloadAttributes(f0, f1) => {
                f.debug_tuple("PayloadAttributes").field(&f0).field(&f1).finish()
            }
            PayloadServiceCommand::Resolve(f0, _f1) => f.debug_tuple("Resolve").field(&f0).finish(),
        }
    }
}
