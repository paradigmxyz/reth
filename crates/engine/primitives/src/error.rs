/// Represents all error cases when handling a new payload.
///
/// This represents all possible error cases that must be returned as JSON RCP errors back to the
/// beacon node.
#[derive(Debug, thiserror::Error)]
pub enum BeaconOnNewPayloadError {
    /// Thrown when the engine task is unavailable/stopped.
    #[error("beacon consensus engine task stopped")]
    EngineUnavailable,
    /// An internal error occurred, not necessarily related to the payload.
    #[error(transparent)]
    Internal(Box<dyn core::error::Error + Send + Sync>),
}

impl BeaconOnNewPayloadError {
    /// Create a new internal error.
    pub fn internal<E: core::error::Error + Send + Sync + 'static>(e: E) -> Self {
        Self::Internal(Box::new(e))
    }
}
