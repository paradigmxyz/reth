//! Support for building payloads.
//!
//! The payload builder is responsible for building payloads.
//! Once a new payload is created, it is continuously updated.


use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::sync::mpsc;
use tokio_stream::wrappers::UnboundedReceiverStream;
use reth_rpc_types::engine::PayloadId;
use crate::BuiltPayload;

/// A communication channel to the [PayloadBuilderService].
///
/// This is the API used to create new payloads and to get the current state of existing ones.
#[derive(Debug, Clone)]
pub struct PayloadBuilderHandle {
    /// Sender half of the message channel to the [PayloadBuilderService].
    to_service: mpsc::UnboundedSender<PayloadBuilderCommand>,
}

// === impl PayloadBuilderHandle ===

impl PayloadBuilderHandle {

    // TODO add async functions for creating new payloads and getting the current state of existing ones
}

/// A service that manages building payloads.
///
/// This type is an endless future that manages the building of payloads.
///
/// It tracks active payloads and their build jobs that run in the worker pool.
#[must_use = "futures do nothing unless you `.await` or poll them"]
pub struct PayloadBuilderService {
    /// All active payloads.
    payloads: HashMap<PayloadId, BuiltPayload>,
    /// All active build jobs.
    /// TODO(mattsse): I think this can be a futures unordered
    build_jobs: (),
    /// Copy of the sender half, so new [`PayloadBuilderHandle`] can be created on demand.
    _service_tx: mpsc::UnboundedSender<PayloadBuilderCommand>,
    /// Receiver half of the command channel.
    command_rx: UnboundedReceiverStream<PayloadBuilderCommand>,
}

impl Future for PayloadBuilderService {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context) -> Poll<Self::Output> {
        // TODO(mattsse): implement
        Poll::Pending
    }
}

/// Message type for the [PayloadBuilderService].
#[derive(Debug)]
enum PayloadBuilderCommand {

}
