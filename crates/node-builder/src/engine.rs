use reth_node_api::EngineTypes;
use reth_payload_builder::PayloadBuilderAttributes;
use reth_rpc_types::engine::PayloadAttributes;

/// The types used in the default mainnet ethereum beacon consensus engine.
#[derive(Debug, Clone)]
pub struct EthEngineTypes;

impl EngineTypes for EthEngineTypes {
    type PayloadAttributes = PayloadAttributes;
    type PayloadBuilderAttributes = PayloadBuilderAttributes;
}
