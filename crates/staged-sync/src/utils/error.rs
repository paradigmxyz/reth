use std::io;

use thiserror::Error;

/// Socket parsing error.
#[derive(Error, Debug)]
pub enum SocketParsingError {
    /// Cannot parse socket address from empty string.
    #[error("Cannot parse socket address: {0}")]
    Io(String),

    /// Cannot parse socket address from empty string.
    #[error("Cannot parse socket address from empty string")]
    Empty,

    /// Could not parse socket address from {0}.
    #[error("Could not parse socket address from {0}")]
    Parse(String),
}

impl From<io::Error> for SocketParsingError {
    fn from(err: io::Error) -> Self {
        SocketParsingError::Io(err.to_string())
    }
}
