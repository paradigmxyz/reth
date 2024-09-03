use reth_payload_primitives::PayloadTypes;
use std::{
    pin::Pin,
    task::{ready, Context, Poll},
};
use tokio::sync::broadcast;
use tokio_stream::{
    wrappers::{errors::BroadcastStreamRecvError, BroadcastStream},
    Stream, StreamExt,
};
use tracing::debug;

/// Payload builder events.
#[derive(Clone, Debug)]
pub enum Events<Engine: PayloadTypes> {
    /// The payload attributes as
    /// they are received from the CL through the engine api.
    Attributes(Engine::PayloadBuilderAttributes),
    /// The built payload that has been just built.
    /// Triggered by the CL whenever it asks for an execution payload.
    /// This event is only thrown if the CL is a validator.
    BuiltPayload(Engine::BuiltPayload),
}

/// Represents a receiver for various payload events.
#[derive(Debug)]
pub struct PayloadEvents<Engine: PayloadTypes> {
    /// The receiver for the payload events.
    pub receiver: broadcast::Receiver<Events<Engine>>,
}

impl<Engine: PayloadTypes + 'static> PayloadEvents<Engine> {
    /// Convert this receiver into a stream of `PayloadEvents`.
    pub fn into_stream(self) -> BroadcastStream<Events<Engine>> {
        BroadcastStream::new(self.receiver)
    }
    /// Asynchronously receives the next payload event.
    pub async fn recv(self) -> Option<Result<Events<Engine>, BroadcastStreamRecvError>> {
        let mut event_stream = self.into_stream();
        event_stream.next().await
    }

    /// Returns a new stream that yields all built payloads.
    pub fn into_built_payload_stream(self) -> BuiltPayloadStream<Engine> {
        BuiltPayloadStream { st: self.into_stream() }
    }

    /// Returns a new stream that yields received payload attributes
    pub fn into_attributes_stream(self) -> PayloadAttributeStream<Engine> {
        PayloadAttributeStream { st: self.into_stream() }
    }
}

/// A stream that yields built payloads.
#[derive(Debug)]
#[pin_project::pin_project]
pub struct BuiltPayloadStream<T: PayloadTypes> {
    /// The stream of events.
    #[pin]
    st: BroadcastStream<Events<T>>,
}

impl<T: PayloadTypes + 'static> Stream for BuiltPayloadStream<T> {
    type Item = T::BuiltPayload;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        loop {
            return match ready!(self.as_mut().project().st.poll_next(cx)) {
                Some(Ok(Events::BuiltPayload(payload))) => Poll::Ready(Some(payload)),
                Some(Ok(Events::Attributes(_))) => {
                    // ignoring attributes
                    continue
                }
                Some(Err(err)) => {
                    debug!(%err, "payload event stream stream lagging behind");
                    continue
                }
                None => Poll::Ready(None),
            }
        }
    }
}

/// A stream that yields received payload attributes
#[derive(Debug)]
#[pin_project::pin_project]
pub struct PayloadAttributeStream<T: PayloadTypes> {
    /// The stream of events.
    #[pin]
    st: BroadcastStream<Events<T>>,
}

impl<T: PayloadTypes + 'static> Stream for PayloadAttributeStream<T> {
    type Item = T::PayloadBuilderAttributes;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        loop {
            return match ready!(self.as_mut().project().st.poll_next(cx)) {
                Some(Ok(Events::Attributes(attr))) => Poll::Ready(Some(attr)),
                Some(Ok(Events::BuiltPayload(_))) => {
                    // ignoring payloads
                    continue
                }
                Some(Err(err)) => {
                    debug!(%err, "payload event stream stream lagging behind");
                    continue
                }
                None => Poll::Ready(None),
            }
        }
    }
}
