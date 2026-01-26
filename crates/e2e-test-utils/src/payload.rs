use futures_util::StreamExt;
use reth_node_api::{BlockBody, PayloadKind};
use reth_payload_builder::{PayloadBuilderHandle, PayloadId};
use reth_payload_builder_primitives::Events;
use reth_payload_primitives::{BuiltPayload, PayloadBuilderAttributes, PayloadTypes};
use tokio_stream::wrappers::BroadcastStream;

/// Helper for payload operations
#[derive(derive_more::Debug)]
pub struct PayloadTestContext<T: PayloadTypes> {
    pub payload_event_stream: BroadcastStream<Events<T>>,
    payload_builder: PayloadBuilderHandle<T>,
    pub timestamp: u64,
    #[debug(skip)]
    attributes_generator: Box<dyn Fn(u64) -> T::PayloadBuilderAttributes + Send + Sync>,
}

impl<T: PayloadTypes> PayloadTestContext<T> {
    /// Creates a new payload helper
    pub async fn new(
        payload_builder: PayloadBuilderHandle<T>,
        attributes_generator: impl Fn(u64) -> T::PayloadBuilderAttributes + Send + Sync + 'static,
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

    /// Wait until the best built payload is ready (including empty blocks).
    ///
    /// Use [`Self::wait_for_non_empty_payload`] if the payload must contain transactions.
    pub async fn wait_for_built_payload(&self, payload_id: PayloadId) {
        let start = std::time::Instant::now();
        loop {
            let payload =
                self.payload_builder.best_payload(payload_id).await.transpose().ok().flatten();
            if payload.is_some() {
                self.payload_builder
                    .resolve_kind(payload_id, PayloadKind::Earliest)
                    .await
                    .unwrap()
                    .unwrap();
                break;
            }
            assert!(
                start.elapsed() < std::time::Duration::from_secs(30),
                "wait_for_built_payload timed out after 30s"
            );
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    }

    /// Wait until the best built payload contains at least one transaction.
    ///
    /// Use this when transactions have been injected and must be included in the block.
    pub async fn wait_for_non_empty_payload(&self, payload_id: PayloadId) {
        let start = std::time::Instant::now();
        loop {
            let payload =
                self.payload_builder.best_payload(payload_id).await.transpose().ok().flatten();
            if payload.is_some_and(|p| !p.block().body().transactions().is_empty()) {
                self.payload_builder
                    .resolve_kind(payload_id, PayloadKind::Earliest)
                    .await
                    .unwrap()
                    .unwrap();
                break;
            }
            assert!(
                start.elapsed() < std::time::Duration::from_secs(30),
                "wait_for_non_empty_payload timed out after 30s"
            );
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
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
