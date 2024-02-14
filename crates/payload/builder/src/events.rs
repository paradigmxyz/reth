use futures_util::Stream;
use reth_node_api::EngineTypes;
use tokio::sync::broadcast;
use tokio_stream::{
    wrappers::{errors::BroadcastStreamRecvError, BroadcastStream},
    StreamExt,
};

/// Payload builder events.
#[derive(Clone, Debug)]
pub enum Events<Engine: EngineTypes> {
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
pub struct PayloadEvents<Engine: EngineTypes> {
    pub receiver: broadcast::Receiver<Events<Engine>>,
}

impl<Engine: EngineTypes + 'static> PayloadEvents<Engine> {
    // Convert this receiver into a stream of PayloadEvents.
    pub fn into_stream(
        self,
    ) -> impl Stream<Item = Result<Events<Engine>, BroadcastStreamRecvError>> {
        BroadcastStream::new(self.receiver)
    }
    /// Asynchronously receives the next payload event.
    pub async fn recv(self) -> Option<Result<Events<Engine>, BroadcastStreamRecvError>> {
        let mut event_stream = self.into_stream();
        event_stream.next().await
    }
}
