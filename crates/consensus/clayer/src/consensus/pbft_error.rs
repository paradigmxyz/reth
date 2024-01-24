/// Errors returned
#[derive(Debug, thiserror::Error)]
pub enum PbftError {
    /// An error occurred while serializing or deserializing
    #[error("SerializationError {0} ,{1}")]
    /// An invalid message was received
    SerializationError(String, String),
    #[error("InvalidMessageType {0}")]
    InvalidMessage(String),
    /// An error occurred while verifying a cryptographic signature
    #[error("SigningError {0}")]
    SigningError(String),
    /// The node detected a faulty primary and started a view change
    #[error("FaultyPrimary {0}")]
    FaultyPrimary(String),
    /// Internal PBFT error (description)
    #[error("InternalError {0}")]
    InternalError(String),
    #[error("ServiceError {0} ,{1}")]
    ServiceError(String, String),
}
