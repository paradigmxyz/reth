use crate::{PayloadAttributesTrait, PayloadBuilderAttributesTrait};

/// The types that are used by the engine.
pub trait EngineTypes: Send + Sync {
    /// The RPC payload attributes type the CL node emits via the engine API.
    type PayloadAttributes: PayloadAttributesTrait + Send + Clone + Unpin;

    /// The payload attributes type that contains information about a running payload job.
    type PayloadBuilderAttributes: PayloadBuilderAttributesTrait<RpcPayloadAttributes = Self::PayloadAttributes>
        + Send
        + Clone
        + Unpin
        + std::fmt::Debug;

    // TODO(rjected): payload type
}
