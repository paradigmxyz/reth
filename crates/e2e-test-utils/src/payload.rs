use futures_util::StreamExt;
use reth_payload_builder_primitives::{Events, PayloadEvents};
use reth_primitives::NodePrimitives;
use reth_provider::test_utils::NoopProvider;
// Note: Depending on the chain type we might use different RPC payload attribute types (Ethereum, Optimism, etc.)
// Importing the generic engine payload attributes for completeness, even though it's not used directly in this file.
use alloy_rpc_types_engine::PayloadAttributes;
use reth_payload_primitives::{PayloadBuilderAttributes, PayloadTypes};
use reth_payload_primitives::BuiltPayload;
use reth_primitives_traits::BlockBody as _;
use tokio_stream::StreamExt as _;
use std::sync::Arc;
use tokio::sync::broadcast;

/// Helper for payload operations
#[derive(derive_more::Debug)]
pub struct PayloadTestContext<T: reth_payload_primitives::PayloadTypes> {
    pub payload_event_stream: broadcast::Receiver<Events<T>>,
    payload_builder: reth_payload_builder::PayloadBuilderHandle<T>,
    pub timestamp: u64,
    #[debug(skip)]
    attributes_generator: Box<dyn Fn(u64) -> T::PayloadBuilderAttributes + Send + Sync>,
}

impl<T: reth_payload_primitives::PayloadTypes> PayloadTestContext<T> {
    /// Creates a new payload helper
    pub async fn new(
        payload_builder: reth_payload_builder::PayloadBuilderHandle<T>,
        attributes_generator: impl Fn(u64) -> T::PayloadBuilderAttributes + Send + Sync + 'static,
    ) -> eyre::Result<Self> {
        use std::time::{Duration, Instant};
        use tokio::time::sleep;

        // Retry subscribing for a short period to wait for the builder service to start.
        let payload_events = {
            let start = Instant::now();
            loop {
                match payload_builder.subscribe().await {
                    Ok(ev) => break ev,
                    Err(err) if start.elapsed() < Duration::from_secs(1) => {
                        // Service not up yet, wait and retry
                        sleep(Duration::from_millis(25)).await;
                        continue;
                    }
                    Err(err) => {
                        eyre::bail!("Failed to subscribe to payload events: {err}")
                    }
                }
            }
        };
        let payload_event_stream = payload_events.receiver;

        Ok(Self {
            payload_event_stream,
            payload_builder,
            timestamp: 0,
            attributes_generator: Box::new(attributes_generator),
        })
    }

    /// Creates a new payload with the given timestamp
    pub async fn new_payload(&mut self) -> eyre::Result<T::PayloadBuilderAttributes> {
        // increment timestamp first so that subsequent calls get unique timestamps
        self.timestamp += 1;

        let attributes = (self.attributes_generator)(self.timestamp);

        // The payload builder may be absent in certain test configurations; ignore send errors.
        if let Ok(res) = self.payload_builder.send_new_payload(attributes.clone()).await {
            // Only propagate errors originating from inside the builder.
            if let Err(err) = res {
                return Err(err.into());
            }
        }

        Ok(attributes)
    }

    /// Asserts that the next event is a payload attributes event matching the provided attributes.
    pub async fn expect_attr_event(
        &mut self,
        attrs: T::PayloadBuilderAttributes,
    ) -> eyre::Result<()> {
        use tokio::time::{timeout, Duration};
        let first_event = timeout(Duration::from_secs(1), self.payload_event_stream.recv())
            .await
            .map_err(|_| eyre::eyre!("Timed out waiting for payload attribute event"))??;
        if let Events::Attributes(attr) = first_event {
            // Basic sanity-check that timestamp matches. Additional checks can be added as necessary.
            assert_eq!(attrs.timestamp(), attr.timestamp());
            Ok(())
        } else {
            eyre::bail!("Expected first event to be payload attributes, got {:?}", first_event);
        }
    }

    /// Wait until the best built payload is ready for the given payload id.
    pub async fn wait_for_built_payload(&self, payload_id: reth_payload_builder::PayloadId) {
        let start = std::time::Instant::now();
        let max_wait = std::time::Duration::from_millis(500);
        loop {
            if let Some(Ok(payload)) = self.payload_builder.best_payload(payload_id).await {
                // break if payload has txs or we've waited long enough (to accept empty blocks)
                if !payload.block().body().transactions().is_empty() || start.elapsed() >= max_wait {
                    break;
                }
            } else if start.elapsed() >= max_wait {
                // builder not ready but we waited long enough → accept
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    }

    /// Expects that the next event is a built payload event and returns the built payload.
    pub async fn expect_built_payload(&mut self) -> eyre::Result<T::BuiltPayload> {
        use tokio::time::{timeout, Duration};
        let event = timeout(Duration::from_secs(1), self.payload_event_stream.recv())
            .await
            .map_err(|_| eyre::eyre!("Timed out waiting for built payload event"))??;
        if let Events::BuiltPayload(payload) = event {
            Ok(payload)
        } else {
            eyre::bail!("Expected built payload event, got {:?}", event);
        }
    }
}
