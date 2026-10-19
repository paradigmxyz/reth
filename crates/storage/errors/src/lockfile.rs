use alloc::string::{String, ToString};

/// Storage lock error.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StorageLockError {
    /// Write lock taken
    #[error("storage directory is currently in use as read-write by another process: PID {_0}")]
    Taken(usize),
    /// Indicates other unspecified errors.
    #[error("{_0}")]
    Other(String),
}

impl StorageLockError {
    /// Converts any error into the `Other` variant of `StorageLockError`.
    pub fn other<E: core::error::Error>(err: E) -> Self {
        Self::Other(err.to_string())
    }
}
