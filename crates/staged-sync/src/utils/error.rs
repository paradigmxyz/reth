use std::io;

use thiserror::Error;

/// Error thrown while parsing a socket address.
#[derive(Error, Debug)]
#[allow(missing_docs)]
pub enum SocketAddressParsingError {
    #[error("Cannot parse socket address: {0}")]
    Io(String),

    #[error("Cannot parse socket address from empty string")]
    Empty,

    #[error("Could not parse socket address from {0}")]
    Parse(String),
}

impl From<io::Error> for SocketAddressParsingError {
    fn from(err: io::Error) -> Self {
        SocketAddressParsingError::Io(err.to_string())
    }
}
