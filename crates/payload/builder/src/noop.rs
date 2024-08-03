//! A payload builder service task that does nothing.

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
#[derive(Debug)]
pub struct NoopPayloadBuilderService<Engine: PayloadTypes> {
    /// Receiver half of the command channel.
    command_rx: UnboundedReceiverStream<PayloadServiceCommand<Engine>>,
}

impl<Engine> NoopPayloadBuilderService<Engine>
where
    Engine: PayloadTypes + 'static,
{
    /// Creates a new [`NoopPayloadBuilderService`].
    pub fn new() -> (Self, PayloadBuilderHandle<Engine>) {
        let (service_tx, command_rx) = mpsc::unbounded_channel();
        (
            Self { command_rx: UnboundedReceiverStream::new(command_rx) },
            PayloadBuilderHandle::new(service_tx),
        )
    }
}

impl<Engine> Future for NoopPayloadBuilderService<Engine>
where
    Engine: PayloadTypes,
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
                PayloadServiceCommand::PayloadAttributes(_, tx) => tx.send(None).ok(),
                PayloadServiceCommand::Resolve(_, tx) => tx.send(None).ok(),
                PayloadServiceCommand::Subscribe(_) => None,
            };
        }
    }
}
