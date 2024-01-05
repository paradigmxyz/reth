use reth_payload_builder::{
    PayloadAttributesTrait, PayloadBuilderAttributes, PayloadBuilderAttributesTrait,
};
use reth_rpc_types::engine::PayloadAttributes;

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

// TODO(rjected): find a better place for this struct - we don't want cyclic deps if we'll be using
// [EngineTypes] in many places. It should be somewhere that has both rpc and payload types.
// Maybe payload builder crate?
#[derive(Debug, Clone)]
pub struct EthEngineTypes;

impl EngineTypes for EthEngineTypes {
    type PayloadAttributes = PayloadAttributes;
    type PayloadBuilderAttributes = PayloadBuilderAttributes;
}
