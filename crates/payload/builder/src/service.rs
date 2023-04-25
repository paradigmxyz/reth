//! Support for building payloads.
//!
//! The payload builder is responsible for building payloads.
//! Once a new payload is created, it is continuously updated.

use crate::{
    error::PayloadBuilderError, metrics::PayloadBuilderServiceMetrics, traits::PayloadJobGenerator,
    BuiltPayload, PayloadBuilderAttributes, PayloadJob,
};
use futures_util::stream::{StreamExt, TryStreamExt};
use reth_rpc_types::engine::PayloadId;
use std::{
    future::Future,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};
use tokio::sync::{mpsc, oneshot};
use tokio_stream::wrappers::UnboundedReceiverStream;
use tracing::{trace, warn};

/// A communication channel to the [PayloadBuilderService] that can retrieve payloads.
#[derive(Debug, Clone)]
pub struct PayloadStore {
    inner: PayloadBuilderHandle,
}

// === impl PayloadStore ===

impl PayloadStore {
    /// Returns the best payload for the given identifier.
    pub async fn get_payload(
        &self,
        id: PayloadId,
    ) -> Option<Result<Arc<BuiltPayload>, PayloadBuilderError>> {
        self.inner.get_payload(id).await
    }
}

impl From<PayloadBuilderHandle> for PayloadStore {
    fn from(inner: PayloadBuilderHandle) -> Self {
        Self { inner }
    }
}

/// A communication channel to the [PayloadBuilderService].
///
/// This is the API used to create new payloads and to get the current state of existing ones.
#[derive(Debug, Clone)]
pub struct PayloadBuilderHandle {
    /// Sender half of the message channel to the [PayloadBuilderService].
    to_service: mpsc::UnboundedSender<PayloadServiceCommand>,
}

// === impl PayloadBuilderHandle ===

impl PayloadBuilderHandle {
    /// Returns the best payload for the given identifier.
    pub async fn get_payload(
        &self,
        id: PayloadId,
    ) -> Option<Result<Arc<BuiltPayload>, PayloadBuilderError>> {
        let (tx, rx) = oneshot::channel();
        self.to_service.send(PayloadServiceCommand::GetPayload(id, tx)).ok()?;
        rx.await.ok()?
    }

    /// Sends a message to the service to start building a new payload for the given payload.
    ///
    /// This is the same as [PayloadBuilderHandle::new_payload] but does not wait for the result and
    /// returns the receiver instead
    pub fn send_new_payload(
        &self,
        attr: PayloadBuilderAttributes,
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
        attr: PayloadBuilderAttributes,
    ) -> Result<PayloadId, PayloadBuilderError> {
        self.send_new_payload(attr).await?
    }
}

/// A service that manages payload building tasks.
///
/// This type is an endless future that manages the building of payloads.
///
/// It tracks active payloads and their build jobs that run in the worker pool.
///
/// By design, this type relies entirely on the [PayloadJobGenerator] to create new payloads and
/// does know nothing about how to build them, itt just drives the payload jobs.
#[must_use = "futures do nothing unless you `.await` or poll them"]
pub struct PayloadBuilderService<Gen>
where
    Gen: PayloadJobGenerator,
{
    /// The type that knows how to create new payloads.
    generator: Gen,
    /// All active payload jobs.
    payload_jobs: Vec<(Gen::Job, PayloadId)>,
    /// Copy of the sender half, so new [`PayloadBuilderHandle`] can be created on demand.
    _service_tx: mpsc::UnboundedSender<PayloadServiceCommand>,
    /// Receiver half of the command channel.
    command_rx: UnboundedReceiverStream<PayloadServiceCommand>,
    /// metrics for the payload builder service
    metrics: PayloadBuilderServiceMetrics,
}

// === impl PayloadBuilderService ===

impl<Gen> PayloadBuilderService<Gen>
where
    Gen: PayloadJobGenerator,
{
    /// Creates a new payload builder service.
    pub fn new(generator: Gen) -> (Self, PayloadBuilderHandle) {
        let (service_tx, command_rx) = mpsc::unbounded_channel();
        let service = Self {
            generator,
            payload_jobs: Vec::new(),
            _service_tx: service_tx.clone(),
            command_rx: UnboundedReceiverStream::new(command_rx),
            metrics: Default::default(),
        };
        let handle = PayloadBuilderHandle { to_service: service_tx };
        (service, handle)
    }

    /// Returns true if the given payload is currently being built.
    fn contains_payload(&self, id: PayloadId) -> bool {
        self.payload_jobs.iter().any(|(_, job_id)| *job_id == id)
    }

    /// Returns the best payload for the given identifier.
    fn get_payload(&self, id: PayloadId) -> Option<Result<Arc<BuiltPayload>, PayloadBuilderError>> {
        self.payload_jobs.iter().find(|(_, job_id)| *job_id == id).map(|(j, _)| j.best_payload())
    }
}

impl<Gen> Future for PayloadBuilderService<Gen>
where
    Gen: PayloadJobGenerator + Unpin + 'static,
    <Gen as PayloadJobGenerator>::Job: Unpin + 'static,
{
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();

        loop {
            // we poll all jobs first, so we always have the latest payload that we can report if
            // requests
            // we don't care about the order of the jobs, so we can just swap_remove them
            'jobs: for idx in (0..this.payload_jobs.len()).rev() {
                let (mut job, id) = this.payload_jobs.swap_remove(idx);

                // drain better payloads from the job
                loop {
                    match job.try_poll_next_unpin(cx) {
                        Poll::Ready(Some(Ok(payload))) => {
                            this.metrics.set_active_jobs(this.payload_jobs.len());
                            trace!(?payload, %id, "new payload");
                        }
                        Poll::Ready(Some(Err(err))) => {
                            warn!(?err, %id, "payload job failed; resolving payload");
                            this.metrics.set_active_jobs(this.payload_jobs.len());
                            this.metrics.inc_failed_jobs();
                            continue 'jobs
                        }
                        Poll::Ready(None) => {
                            // job is done
                            trace!(?id, "payload job finished");
                            this.metrics.set_active_jobs(this.payload_jobs.len());
                            continue 'jobs
                        }
                        Poll::Pending => {
                            // still pending, put it back
                            this.payload_jobs.push((job, id));
                            continue 'jobs
                        }
                    }
                }
            }

            // marker for exit condition
            // TODO(mattsse): this could be optmized so we only poll new jobs
            let mut new_job = false;

            // drain all requests
            while let Poll::Ready(Some(cmd)) = this.command_rx.poll_next_unpin(cx) {
                match cmd {
                    PayloadServiceCommand::BuildNewPayload(attr, tx) => {
                        let id = attr.payload_id();
                        let mut res = Ok(id);

                        if !this.contains_payload(id) {
                            // no job for this payload yet, create one
                            match this.generator.new_payload_job(attr) {
                                Ok(job) => {
                                    this.metrics.inc_initiated_jobs();
                                    new_job = true;
                                    this.payload_jobs.push((job, id));
                                }
                                Err(err) => {
                                    this.metrics.inc_failed_jobs();
                                    warn!(?err, %id, "failed to create payload job");
                                    res = Err(err);
                                }
                            }
                        }

                        // return the id of the payload
                        let _ = tx.send(res);
                    }
                    PayloadServiceCommand::GetPayload(id, tx) => {
                        let _ = tx.send(this.get_payload(id));
                    }
                }
            }

            if !new_job {
                return Poll::Pending
            }
        }
    }
}

/// Message type for the [PayloadBuilderService].
#[derive(Debug)]
enum PayloadServiceCommand {
    /// Start building a new payload.
    BuildNewPayload(
        PayloadBuilderAttributes,
        oneshot::Sender<Result<PayloadId, PayloadBuilderError>>,
    ),
    /// Get the current payload.
    GetPayload(PayloadId, oneshot::Sender<Option<Result<Arc<BuiltPayload>, PayloadBuilderError>>>),
}
