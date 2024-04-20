use reth::rpc::types::engine::PayloadAttributes;
use reth_node_optimism::OptimismPayloadBuilderAttributes;
use reth_payload_builder::EthPayloadBuilderAttributes;
use reth_primitives::{Address, B256};

/// Helper function to create a new eth payload attributes
pub(crate) fn optimism_payload_attributes(timestamp: u64) -> OptimismPayloadBuilderAttributes {
    let attributes = PayloadAttributes {
        timestamp,
        prev_randao: B256::ZERO,
        suggested_fee_recipient: Address::ZERO,
        withdrawals: Some(vec![]),
        parent_beacon_block_root: Some(B256::ZERO),
    };

    OptimismPayloadBuilderAttributes {
        payload_attributes: EthPayloadBuilderAttributes::new(B256::ZERO, attributes),
        transactions: vec![],
        no_tx_pool: false,
        gas_limit: Some(30_000_000),
    }
}
