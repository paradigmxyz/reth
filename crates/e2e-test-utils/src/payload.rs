use futures_util::StreamExt;
use reth_payload_builder::{PayloadBuilderHandle, PayloadId};
use reth_payload_builder_primitives::{Events, PayloadBuilder};
use reth_payload_primitives::{BuiltPayload, PayloadBuilderAttributes, PayloadTypes};
use tokio_stream::wrappers::BroadcastStream;

/// Helper for payload operations
#[derive(derive_more::Debug)]
pub struct PayloadTestContext<T: PayloadTypes> {
    pub payload_event_stream: BroadcastStream<Events<T>>,
    payload_builder: PayloadBuilderHandle<T>,
    pub timestamp: u64,
    #[debug(skip)]
    attributes_generator: Box<dyn Fn(u64) -> T::PayloadBuilderAttributes>,
}

impl<T: PayloadTypes> PayloadTestContext<T> {
    /// Creates a new payload helper
    pub async fn new(
        payload_builder: PayloadBuilderHandle<T>,
        attributes_generator: impl Fn(u64) -> T::PayloadBuilderAttributes + 'static,
    ) -> eyre::Result<Self> {
        let payload_events = payload_builder.subscribe().await?;
        let payload_event_stream = payload_events.into_stream();
        // Cancun timestamp
        Ok(Self {
            payload_event_stream,
            payload_builder,
            timestamp: 1710338135,
            attributes_generator: Box::new(attributes_generator),
        })
    }

    /// Creates a new payload job from static attributes
    pub async fn new_payload(&mut self) -> eyre::Result<T::PayloadBuilderAttributes> {
        self.timestamp += 1;
        let attributes = (self.attributes_generator)(self.timestamp);
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
