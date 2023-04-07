//! Support for building payloads.
//!
//! The payload builder is responsible for building payloads.
//! Once a new payload is created, it is continuously updated.

use crate::{traits::PayloadGenerator, BuiltPayload, PayloadBuilderAttributes};
use reth_rpc_types::engine::PayloadId;
use std::{
    collections::HashMap,
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};
use tokio::sync::{mpsc, oneshot};
use tokio_stream::wrappers::UnboundedReceiverStream;

/// A communication channel to the [PayloadBuilderService].
///
/// This is the API used to create new payloads and to get the current state of existing ones.
// TODO this replaces the PayloadStore
#[derive(Debug, Clone)]
pub struct PayloadBuilderHandle {
    /// Sender half of the message channel to the [PayloadBuilderService].
    to_service: mpsc::UnboundedSender<PayloadServiceCommand>,
}

// === impl PayloadBuilderHandle ===

impl PayloadBuilderHandle {
    /// Returns the best payload for the given identifier.
    pub async fn get_payload(&self, id: PayloadId) -> Option<BuiltPayload> {
        let (tx, rx) = oneshot::channel();
        self.to_service.send(PayloadServiceCommand::GetPayload(id, tx)).ok()?;
        rx.await.ok()?
    }

    /// Starts building a new payload for the given payload attributes.
    ///
    /// Returns the identifier of the new payload.
    pub async fn new_payload(
        &self,
        attr: PayloadBuilderAttributes,
    ) -> Result<PayloadId, oneshot::error::RecvError> {
        let (tx, rx) = oneshot::channel();
        let _ = self.to_service.send(PayloadServiceCommand::BuildNewPayload(attr, tx));
        rx.await
    }
}

/// A service that manages payload building tasks.
///
/// This type is an endless future that manages the building of payloads.
///
/// It tracks active payloads and their build jobs that run in the worker pool.
///
/// By design, this type relies entirely on the [PayloadGenerator] to create new payloads and does
/// know nothing about how to build them, itt just drives the payload jobs.
#[must_use = "futures do nothing unless you `.await` or poll them"]
pub struct PayloadBuilderService<Gen>
where
    Gen: PayloadGenerator,
{
    /// All payloads we're currently tracking.
    ///
    /// These are payload jobs that were initiated by the engine API.
    local_payloads: HashMap<PayloadId, BuiltPayload>,
    /// The type that knows how to create new payloads.
    generator: Gen,
    /// All active payload jobs.
    payload_jobs: Vec<Gen::PayloadUpdates>,
    /// Copy of the sender half, so new [`PayloadBuilderHandle`] can be created on demand.
    _service_tx: mpsc::UnboundedSender<PayloadServiceCommand>,
    /// Receiver half of the command channel.
    command_rx: UnboundedReceiverStream<PayloadServiceCommand>,
}

impl<Gen> Future for PayloadBuilderService<Gen>
where
    Gen: PayloadGenerator + 'static,
{
    type Output = ();

    fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
        // payload lifecycle:
        // 1. create new payload (initially empty)
        // 2. start building payload via loop (default max 12s timeout == SECONDS_PER_SLOT), let's
        // call this the "payload generator loop" 3. calls the payload generator to create a
        // new payload until resolved or timeout reached; `Resolve` returns the latest built payload
        // and also terminates payload generation 4. if the generator yields a payload, the
        // payload is updated (note: there _could_ be multiple concurrent generators for the same
        // payload)

        Poll::Pending
    }
}

/// Message type for the [PayloadBuilderService].
#[derive(Debug)]
enum PayloadServiceCommand {
    /// Start building a new payload.
    BuildNewPayload(PayloadBuilderAttributes, oneshot::Sender<PayloadId>),
    /// Get the current payload.
    GetPayload(PayloadId, oneshot::Sender<Option<BuiltPayload>>),
}
