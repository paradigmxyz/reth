/// This can be implemented by types that describe a currently running payload job.
pub trait PayloadBuilderAttributesTrait {
    /// The payload attributes that can be used to construct this type. Used as the argument in
    /// [PayloadBuilderAttributesTrait::try_new].
    type RpcPayloadAttributes;
    /// The error type used in [PayloadBuilderAttributesTrait::try_new].
    type Error;

    /// Creates a new payload builder for the given parent block and the attributes.
    ///
    /// Derives the unique [PayloadId] for the given parent and attributes
    fn try_new(
        parent: B256,
        rpc_payload_attributes: Self::RpcPayloadAttributes,
    ) -> Result<Self, Self::Error>
    where
        Self: Sized;

    /// Returns the [PayloadId] for the running payload job.
    fn payload_id(&self) -> PayloadId;

    /// Returns the parent block hash for the running payload job.
    fn parent(&self) -> B256;
}

/// The execution payload attribute type the CL node emits via the engine API.
/// This type should be implemented by types that could be used to spawn a payload job.
///
/// This type is emitted as part of the fork choice update call
pub trait PayloadAttributesTrait:
    serde::de::DeserializeOwned + serde::Serialize + std::fmt::Debug + Clone + Send + Sync + 'static
{
    /// Returns the timestamp to be used in the payload job.
    fn timestamp(&self) -> u64;
}
