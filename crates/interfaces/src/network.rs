use thiserror::Error;

/// Network Errors
#[allow(missing_docs)]
#[derive(Error, Debug, Clone, PartialEq, Eq)]
pub enum Error {
    #[error("Sender has been dropped")]
    SenderDropped,
}
