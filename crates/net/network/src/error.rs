//! Possible errors when interacting with the network.

/// All error variants for the network
#[derive(Debug, thiserror::Error)]
pub enum NetworkError {
    /// General IO error.
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// IO error when creating the discovery service
    #[error("Failed to launch discovery service: {0}")]
    Discovery(std::io::Error),
}
