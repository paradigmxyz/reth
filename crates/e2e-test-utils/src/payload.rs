use futures_util::StreamExt;
use reth::api::{BuiltPayload, PayloadBuilderAttributes};
use reth_payload_builder::{PayloadBuilderHandle, PayloadId};
use reth_payload_primitives::{Events, PayloadBuilder, PayloadTypes};
use tokio_stream::wrappers::BroadcastStream;

/// Helper for payload operations
#[derive(Debug)]
pub struct PayloadTestContext<T: PayloadTypes> {
    pub payload_event_stream: BroadcastStream<Events<T>>,
    payload_builder: PayloadBuilderHandle<T>,
    pub timestamp: u64,
}

impl<T: PayloadTypes> PayloadTestContext<T> {
    /// Creates a new payload helper
    pub async fn new(payload_builder: PayloadBuilderHandle<T>) -> eyre::Result<Self> {
        let payload_events = payload_builder.subscribe().await?;
        let payload_event_stream = payload_events.into_stream();
        // Cancun timestamp
        Ok(Self { payload_event_stream, payload_builder, timestamp: 1710338135 })
    }

    /// Creates a new payload job from static attributes
    pub async fn new_payload(
        &mut self,
        attributes_generator: impl Fn(u64) -> T::PayloadBuilderAttributes,
    ) -> eyre::Result<T::PayloadBuilderAttributes> {
        self.timestamp += 1;
        let attributes = attributes_generator(self.timestamp);
        self.payload_builder.send_new_payload(attributes.clone()).await.unwrap()?;
        Ok(attributes)
    }

    /// Asserts that the next event is a payload attributes event
    pub async fn expect_attr_event(
        &mut self,
        attrs: T::PayloadBuilderAttributes,
    ) -> eyre::Result<()> {
        let first_event = self.payload_event_stream.next().await.unwrap()?;
        if let Events::Attributes(attr) = first_event {
            assert_eq!(attrs.timestamp(), attr.timestamp());
        } else {
            panic!("Expect first event as payload attributes.")
        }
        Ok(())
    }

    /// Wait until the best built payload is ready
    pub async fn wait_for_built_payload(&self, payload_id: PayloadId) {
        loop {
            let payload = self.payload_builder.best_payload(payload_id).await.unwrap().unwrap();
            if payload.block().body.transactions.is_empty() {
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                continue
            }
            break
        }
    }

    /// Expects the next event to be a built payload event or panics
    pub async fn expect_built_payload(&mut self) -> eyre::Result<T::BuiltPayload> {
        let second_event = self.payload_event_stream.next().await.unwrap()?;
        if let Events::BuiltPayload(payload) = second_event {
            Ok(payload)
        } else {
            panic!("Expect a built payload event.");
        }
    }
}
