use reth_node_api::EngineTypes;
use reth_payload_builder::{OptimismPayloadBuilderAttributes, PayloadBuilderAttributes};
use reth_rpc_types::engine::{OptimismPayloadAttributes, PayloadAttributes};

/// The types used in the default mainnet ethereum beacon consensus engine.
#[derive(Debug, Clone)]
pub struct EthEngineTypes;

impl EngineTypes for EthEngineTypes {
    type PayloadAttributes = PayloadAttributes;
    type PayloadBuilderAttributes = PayloadBuilderAttributes;
}

/// The types used in the optimism beacon consensus engine.
#[derive(Debug, Clone)]
pub struct OptimismEngineTypes;

impl EngineTypes for OptimismEngineTypes {
    type PayloadAttributes = OptimismPayloadAttributes;
    type PayloadBuilderAttributes = OptimismPayloadBuilderAttributes;
}
