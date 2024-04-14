use std::time::{SystemTime, UNIX_EPOCH};

use futures_util::StreamExt;
use reth::{
    api::{BuiltPayload, EngineTypes, PayloadBuilderAttributes},
    rpc::types::engine::PayloadAttributes,
};
use reth_node_ethereum::EthEngineTypes;
use reth_payload_builder::{EthPayloadBuilderAttributes, Events, PayloadBuilderHandle, PayloadId};
use reth_primitives::{Address, B256};
use tokio_stream::wrappers::BroadcastStream;

impl PayloadHelper<EthEngineTypes> {}

/// Helper for payload operations
pub struct PayloadHelper<E: EngineTypes + 'static> {
    pub payload_event_stream: BroadcastStream<Events<E>>,
    payload_builder: PayloadBuilderHandle<E>,
}

impl<E: EngineTypes> PayloadHelper<E> {
    /// Creates a new payload helper
    pub async fn new(payload_builder: PayloadBuilderHandle<E>) -> eyre::Result<Self> {
        let payload_events = payload_builder.subscribe().await?;
        let payload_event_stream = payload_events.into_stream();
        Ok(Self { payload_event_stream, payload_builder })
    }

    /// Creates a new payload job from static attributes
    pub async fn new_payload(&self) -> eyre::Result<E::PayloadBuilderAttributes> {
        let attributes = eth_payload_attributes::<E::PayloadAttributes>();
        self.payload_builder.new_payload(attributes.clone()).await.unwrap();
        Ok(attributes)
    }

    /// Asserts that the next event is a payload attributes event
    pub async fn expect_attr_event(
        &mut self,
        attrs: EthPayloadBuilderAttributes,
    ) -> eyre::Result<()> {
        let first_event = self.payload_event_stream.next().await.unwrap()?;
        if let reth::payload::Events::Attributes(attr) = first_event {
            assert_eq!(attrs.timestamp, attr.timestamp());
        } else {
            panic!("Expect first event as payload attributes.")
        }
        Ok(())
    }

    /// Wait until the best built payload is ready
    pub async fn wait_for_built_payload(&self, payload_id: PayloadId) {
        loop {
            let payload = self.payload_builder.best_payload(payload_id).await.unwrap().unwrap();
            if payload.block().body.is_empty() {
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                continue;
            }
            break;
        }
    }

    /// Expects the next event to be a built payload event or panics
    pub async fn expect_built_payload(&mut self) -> eyre::Result<<E as EngineTypes>::BuiltPayload> {
        let second_event = self.payload_event_stream.next().await.unwrap()?;
        if let reth::payload::Events::BuiltPayload(payload) = second_event {
            Ok(payload)
        } else {
            panic!("Expect a built payload event.");
        }
    }
}

/// Helper function to create a new eth payload attributes
fn eth_payload_attributes<
    Attrs: PayloadBuilderAttributes<RpcPayloadAttributes = PayloadAttributes>,
>() -> Attrs {
    let timestamp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();

    let attributes = PayloadAttributes {
        timestamp,
        prev_randao: B256::ZERO,
        suggested_fee_recipient: Address::ZERO,
        withdrawals: Some(vec![]),
        parent_beacon_block_root: Some(B256::ZERO),
    };
    Attrs::try_new(B256::ZERO, attributes).unwrap()
}
