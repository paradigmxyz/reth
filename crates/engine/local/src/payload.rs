use reth_ethereum_engine_primitives::EthPayloadAttributes;
use reth_payload_primitives::{PayloadAttributesBuilder, PayloadTypes};
use std::convert::Infallible;

/// The attributes builder for Ethereum payload.
#[derive(Debug)]
pub struct EthPayloadAttributesBuilder;

impl PayloadAttributesBuilder for EthPayloadAttributesBuilder {
    type PayloadAttributes = EthPayloadAttributes;
    type Error = Infallible;

    fn build(&self) -> Result<Self::PayloadAttributes, Self::Error> {

        Ok(EthPayloadAttributes {
            timestamp: ,
            prev_randao: Default::default(),
            suggested_fee_recipient: Default::default(),
            withdrawals: None,
            parent_beacon_block_root: None,
        })
    }
}
