use reth::rpc::types::engine::PayloadAttributes;
use reth_payload_builder::EthPayloadBuilderAttributes;
use reth_primitives::{Address, B256};

/// Helper function to create a new eth payload attributes
pub(crate) fn eth_payload_attributes(timestamp: u64) -> EthPayloadBuilderAttributes {
    let attributes = PayloadAttributes {
        timestamp,
        prev_randao: B256::ZERO,
        suggested_fee_recipient: Address::ZERO,
        withdrawals: Some(vec![]),
        parent_beacon_block_root: Some(B256::ZERO),
    };
    EthPayloadBuilderAttributes::new(B256::ZERO, attributes)
}
