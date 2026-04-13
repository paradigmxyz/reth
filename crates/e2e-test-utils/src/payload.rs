use futures_util::StreamExt;
use reth_node_api::{BlockBody, PayloadAttributes, PayloadKind};
use reth_payload_builder::{PayloadBuilderHandle, PayloadId};
use reth_payload_builder_primitives::Events;
use reth_payload_primitives::{BuiltPayload, PayloadTypes};
use tokio_stream::wrappers::BroadcastStream;

/// Helper for payload operations
#[derive(derive_more::Debug)]
pub struct PayloadTestContext<T: PayloadTypes> {
    pub payload_event_stream: BroadcastStream<Events<T>>,
    payload_builder: PayloadBuilderHandle<T>,
    pub timestamp: u64,
    #[debug(skip)]
    attributes_generator: Box<dyn Fn(u64) -> T::PayloadAttributes + Send + Sync>,
}

impl<T: PayloadTypes> PayloadTestContext<T> {
    /// Creates a new payload helper
    pub async fn new(
        payload_builder: PayloadBuilderHandle<T>,
        attributes_generator: impl Fn(u64) -> T::PayloadAttributes + Send + Sync + 'static,
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

    /// Generates the next payload attributes
    pub fn next_attributes(&mut self) -> T::PayloadAttributes {
        self.timestamp += 1;
        (self.attributes_generator)(self.timestamp)
    }

    /// Asserts that the next event is a payload attributes event
    pub async fn expect_attr_event(&mut self, attrs: T::PayloadAttributes) -> eyre::Result<()> {
        let first_event = self.payload_event_stream.next().await.unwrap()?;
        if let Events::Attributes(attr) = first_event {
            assert_eq!(attrs.timestamp(), attr.timestamp());
        } else {
            panic!("Expect first event as payload attributes.")
        }
        Ok(())
    }

    /// Wait until the best built payload is ready.
    ///
    /// Panics if the payload builder does not produce a non-empty payload within 30 seconds.
    pub async fn wait_for_built_payload(&self, payload_id: PayloadId) {
        let start = std::time::Instant::now();
        loop {
            let payload =
                self.payload_builder.best_payload(payload_id).await.transpose().ok().flatten();
            if payload.is_none_or(|p| p.block().body().transactions().is_empty()) {
                assert!(
                    start.elapsed() < std::time::Duration::from_secs(30),
                    "timed out waiting for a non-empty payload for {payload_id} — \
                     check that the chain spec supports all generated tx types"
                );
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                continue
            }
            // Resolve payload once its built
            self.payload_builder
                .resolve_kind(payload_id, PayloadKind::Earliest)
                .await
                .unwrap()
                .unwrap();
            break;
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
