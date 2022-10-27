/// Database Error
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Encode errors.
    #[error("A table encoding error:{0}")]
    Encode(eyre::Error),
    /// Decode errors.
    #[error("A table decoding error:{0}")]
    Decode(eyre::Error),
    /// Initialization database error.
    #[error("Initialization database error:{0}")]
    Initialization(eyre::Error),
    /// Internal DB error.
    #[error("A internal database error:{0}")]
    Internal(eyre::Error),
    /// Table not created and it does not exist.
    #[error("Table {0} is not existing")]
    TableNotExist(String),
    /// Permission denied
    #[error("Permission denied, action can't be completed.")]
    PermissionDenied,
}
