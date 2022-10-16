use thiserror::Error;

use crate::IngressECIESValue;

/// An error that occurs while reading or writing to an ECIES stream.
#[derive(Debug, Error)]
pub enum ECIESError {
    /// Error during IO
    #[error("IO error")]
    IO(#[from] std::io::Error),
    /// Error when checking the HMAC tag against the tag on the data
    #[error("tag check failure")]
    TagCheckFailed,
    /// Error when parsing AUTH data
    #[error("invalid auth data")]
    InvalidAuthData,
    /// Error when parsing ACK data
    #[error("invalid ack data")]
    InvalidAckData,
    /// Error when reading the header if its length is <3
    #[error("invalid body data")]
    InvalidHeader,
    /// Error when interacting with secp256k1
    #[error(transparent)]
    Secp256k1(#[from] secp256k1::Error),
    /// Error when decoding RLP data
    #[error(transparent)]
    RLPDecoding(#[from] reth_rlp::DecodeError),
    /// Error when convering to integer
    #[error(transparent)]
    FromInt(#[from] std::num::TryFromIntError),
    /// Error when trying to split an array beyond its length
    #[error("requested {idx} but array len is {len}")]
    OutOfBounds {
        /// The index you are trying to split at
        idx: usize,
        /// The length of the array
        len: usize,
    },
    /// Error when handshaking with a peer (ack / auth)
    #[error("invalid handshake: expected {expected:?}, got {msg:?} instead")]
    InvalidHandshake {
        /// The expected return value from the peer
        expected: IngressECIESValue,
        /// The actual value returned from the peer
        msg: Option<IngressECIESValue>,
    },
}
