//! A payload builder service task that does nothing.

use crate::{
    service::PayloadServiceCommand, traits::EthEngineTypes, PayloadBuilderAttributes,
    PayloadBuilderHandle,
};
use futures_util::{ready, StreamExt};
use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};
use tokio::sync::mpsc;
use tokio_stream::wrappers::UnboundedReceiverStream;

/// A service task that does not build any payloads.
#[derive(Debug)]
pub struct NoopPayloadBuilderService {
    /// Receiver half of the command channel.
    command_rx: UnboundedReceiverStream<PayloadServiceCommand<PayloadBuilderAttributes>>,
}

impl NoopPayloadBuilderService {
    /// Creates a new [NoopPayloadBuilderService].
    pub fn new() -> (Self, PayloadBuilderHandle<EthEngineTypes>) {
        let (service_tx, command_rx) = mpsc::unbounded_channel();
        let handle = PayloadBuilderHandle::new(service_tx);
        (Self { command_rx: UnboundedReceiverStream::new(command_rx) }, handle)
    }
}

impl Future for NoopPayloadBuilderService {
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
                PayloadServiceCommand::PayloadAttributes(_, tx) => tx.send(None).ok(),
                PayloadServiceCommand::Resolve(_, tx) => tx.send(None).ok(),
            };
        }
    }
}
