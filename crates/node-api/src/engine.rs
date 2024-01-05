use crate::{PayloadAttributesTrait, PayloadBuilderAttributesTrait};

/// The types that are used by the engine.
pub trait EngineTypes {
    /// The RPC payload attributes type the CL node emits via the engine API.
    type PayloadAttributes: PayloadAttributesTrait + Send + Clone;

    /// The payload attributes type that contains information about a running payload job.
    type PayloadBuilderAttributes: PayloadBuilderAttributesTrait<RpcPayloadAttributes = Self::PayloadAttributes>
        + Send
        + Clone
        + std::fmt::Debug;

    // TODO(rjected): payload type
}
