use crate::PayloadTypes;
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
pub enum Events<T: PayloadTypes> {
    /// The payload attributes as
    /// they are received from the CL through the engine api.
    Attributes(T::PayloadBuilderAttributes),
    /// The built payload that has been just built.
    /// Triggered by the CL whenever it asks for an execution payload.
    /// This event is only thrown if the CL is a validator.
    BuiltPayload(T::BuiltPayload),
}

/// Represents a receiver for various payload events.
#[derive(Debug)]
pub struct PayloadEvents<T: PayloadTypes> {
    /// The receiver for the payload events.
    pub receiver: broadcast::Receiver<Events<T>>,
}

impl<T: PayloadTypes> PayloadEvents<T> {
    /// Convert this receiver into a stream of `PayloadEvents`.
    pub fn into_stream(self) -> BroadcastStream<Events<T>> {
        BroadcastStream::new(self.receiver)
    }
    /// Asynchronously receives the next payload event.
    pub async fn recv(self) -> Option<Result<Events<T>, BroadcastStreamRecvError>> {
        let mut event_stream = self.into_stream();
        event_stream.next().await
    }

    /// Returns a new stream that yields all built payloads.
    pub fn into_built_payload_stream(self) -> BuiltPayloadStream<T> {
        BuiltPayloadStream { st: self.into_stream() }
    }

    /// Returns a new stream that yields received payload attributes
    pub fn into_attributes_stream(self) -> PayloadAttributeStream<T> {
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

impl<T: PayloadTypes> Stream for BuiltPayloadStream<T> {
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

impl<T: PayloadTypes> Stream for PayloadAttributeStream<T> {
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
