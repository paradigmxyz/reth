//! The implementation of the [`PayloadAttributesBuilder`] for the
//! [`LocalEngineService`](super::service::LocalEngineService).

use alloy_primitives::{Address, B256};
use reth_ethereum_engine_primitives::EthPayloadAttributes;
use reth_payload_primitives::PayloadAttributesBuilder;
use std::{convert::Infallible, time::UNIX_EPOCH};

/// The attributes builder for local Ethereum payload.
#[derive(Debug)]
pub struct EthLocalPayloadAttributesBuilder;

impl PayloadAttributesBuilder for EthLocalPayloadAttributesBuilder {
    type PayloadAttributes = EthPayloadAttributes;
    type Error = Infallible;

    fn build(&self) -> Result<Self::PayloadAttributes, Self::Error> {
        let ts = std::time::SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("cannot be earlier than UNIX_EPOCH");

        Ok(EthPayloadAttributes {
            timestamp: ts.as_secs(),
            prev_randao: B256::random(),
            suggested_fee_recipient: Address::random(),
            withdrawals: None,
            parent_beacon_block_root: None,
        })
    }
}
