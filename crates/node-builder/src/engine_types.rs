use reth_payload_builder::{
    PayloadAttributes, PayloadAttributesTrait, PayloadBuilderAttributes,
    PayloadBuilderAttributesTrait,
};

// TODO(rjected): find a better place for this trait
/// The types that are used by the engine.
pub trait EngineTypes {
    /// The RPC payload attributes type the CL node emits via the engine API.
    type PayloadAttributes: PayloadAttributesTrait + Clone;

    /// The payload attributes type that contains information about a running payload job.
    type PayloadBuilderAttributes: PayloadBuilderAttributesTrait<RpcPayloadAttributes = Self::PayloadAttributes>
        + Clone
        + std::fmt::Debug;

    // TODO(rjected): payload type
}

// TODO(rjected): find a better place for this struct
#[derive(Debug, Clone)]
pub struct EthEngineTypes;

impl EngineTypes for EthEngineTypes {
    type PayloadAttributes = PayloadAttributes;
    type PayloadBuilderAttributes = PayloadBuilderAttributes;
}
