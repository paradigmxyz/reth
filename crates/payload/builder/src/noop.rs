//! A payload builder service task that does nothing.
//!
//! This module provides `NoopPayloadBuilderService`, a payload builder implementation that
//! accepts payload building requests but does not actually build any payloads. All commands
//! return empty or default responses. Useful for testing and node implementations that do not
//! require payload building functionality (e.g., nodes that are not validators or sequencers).
//!
//! # Examples
//!
//! ```rust
//! use reth_ethereum_engine_primitives::EthEngineTypes;
//! use reth_payload_builder::noop::NoopPayloadBuilderService;
//!
//! let (service, handle) = NoopPayloadBuilderService::<EthEngineTypes>::new();
//! // The service can be spawned as a task, and the handle can be used to interact with it
//! ```
//!
//! # Use Cases
//!
//! - Testing scenarios where payload building is not needed
//! - Node implementations that do not implement validating or sequencing logic
//! - Wiring components together that require a payload builder handle but don't need actual payload
//!   building

use crate::{service::PayloadServiceCommand, PayloadBuilderHandle};
use futures_util::{ready, StreamExt};
use reth_payload_primitives::{PayloadBuilderAttributes, PayloadTypes};
use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};
use tokio::sync::mpsc;
use tokio_stream::wrappers::UnboundedReceiverStream;

/// A service task that does not build any payloads.
///
/// This service accepts all payload building commands but returns empty or default responses.
/// - `BuildNewPayload` returns the payload ID from attributes but does not build a payload
/// - `BestPayload`, `PayloadTimestamp`, and `Resolve` all return `None`
/// - `Subscribe` is ignored
///
/// Suitable for testing and scenarios where payload building functionality is not required.
#[derive(Debug)]
pub struct NoopPayloadBuilderService<T: PayloadTypes> {
    /// Receiver half of the command channel.
    command_rx: UnboundedReceiverStream<PayloadServiceCommand<T>>,
}

impl<T> NoopPayloadBuilderService<T>
where
    T: PayloadTypes,
{
    /// Creates a new [`NoopPayloadBuilderService`].
    pub fn new() -> (Self, PayloadBuilderHandle<T>) {
        let (service_tx, command_rx) = mpsc::unbounded_channel();
        (
            Self { command_rx: UnboundedReceiverStream::new(command_rx) },
            PayloadBuilderHandle::new(service_tx),
        )
    }
}

impl<T> Future for NoopPayloadBuilderService<T>
where
    T: PayloadTypes,
{
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        loop {
            let Some(cmd) = ready!(this.command_rx.poll_next_unpin(cx)) else {
                return Poll::Ready(())
            };
            match cmd {
                PayloadServiceCommand::BuildNewPayload(attr, tx) => {
                    let id = attr.payload_id();
                    tx.send(Ok(id)).ok()
                }
                PayloadServiceCommand::BestPayload(_, tx) => tx.send(None).ok(),
                PayloadServiceCommand::PayloadTimestamp(_, tx) => tx.send(None).ok(),
                PayloadServiceCommand::Resolve(_, _, tx) => tx.send(None).ok(),
                PayloadServiceCommand::Subscribe(_) => None,
            };
        }
    }
}

impl<T: PayloadTypes> Default for NoopPayloadBuilderService<T> {
    fn default() -> Self {
        let (service, _) = Self::new();
        service
    }
}

impl<T: PayloadTypes> PayloadBuilderHandle<T> {
    /// Returns a new noop instance.
    ///
    /// This creates a handle that is not connected to any active service. All requests
    /// sent through this handle will be silently ignored. Useful when a payload builder
    /// handle is required by the API but payload building is not needed.
    pub fn noop() -> Self {
        Self::new(mpsc::unbounded_channel().0)
    }
}
