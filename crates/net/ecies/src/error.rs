use crate::IngressECIESValue;
use std::fmt;
use thiserror::Error;

/// An error that occurs while reading or writing to an ECIES stream.
#[derive(Debug, Error)]
pub struct ECIESError {
    inner: Box<ECIESErrorImpl>,
}

impl ECIESError {
    /// Consumes the type and returns the error enum
    pub fn into_inner(self) -> ECIESErrorImpl {
        *self.inner
    }

    /// Returns a reference to the inner error
    pub const fn inner(&self) -> &ECIESErrorImpl {
        &self.inner
    }
}

impl fmt::Display for ECIESError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&*self.inner, f)
    }
}

/// An error that occurs while reading or writing to an ECIES stream.
#[derive(Debug, Error)]
pub enum ECIESErrorImpl {
    /// Error during IO
    #[error(transparent)]
    IO(std::io::Error),
    /// Error when checking the HMAC tag against the tag on the message being decrypted
    #[error("tag check failure in read_header")]
    TagCheckDecryptFailed,
    /// Error when checking the HMAC tag against the tag on the header
    #[error("tag check failure in read_header")]
    TagCheckHeaderFailed,
    /// Error when checking the HMAC tag against the tag on the body
    #[error("tag check failure in read_body")]
    TagCheckBodyFailed,
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
    Secp256k1(secp256k1::Error),
    /// Error when decoding RLP data
    #[error(transparent)]
    RLPDecoding(alloy_rlp::Error),
    /// Error when converting to integer
    #[error(transparent)]
    FromInt(std::num::TryFromIntError),
    /// The encrypted data is not large enough for all fields
    #[error("encrypted data is not large enough for all fields")]
    EncryptedDataTooSmall,
    /// The initial header body is too large.
    #[error("initial header body is {body_size} but the max is {max_body_size}")]
    InitialHeaderBodyTooLarge {
        /// The body size from the header
        body_size: usize,
        /// The max body size
        max_body_size: usize,
    },
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
    /// Error when the stream was closed by the peer for being unreadable.
    ///
    /// This exact error case happens when the wrapped stream in
    /// [`Framed`](tokio_util::codec::Framed) is closed by the peer, See
    /// [`ConnectionReset`](std::io::ErrorKind::ConnectionReset) and the ecies codec fails to
    /// decode a message from the (partially filled) buffer.
    #[error("stream closed due to not being readable")]
    UnreadableStream,
    /// Error when data is not received from peer for a prolonged period.
    #[error("never received data from remote peer")]
    StreamTimeout,
}

impl From<ECIESErrorImpl> for ECIESError {
    fn from(source: ECIESErrorImpl) -> Self {
        Self { inner: Box::new(source) }
    }
}

impl From<std::io::Error> for ECIESError {
    fn from(source: std::io::Error) -> Self {
        ECIESErrorImpl::IO(source).into()
    }
}

impl From<secp256k1::Error> for ECIESError {
    fn from(source: secp256k1::Error) -> Self {
        ECIESErrorImpl::Secp256k1(source).into()
    }
}

impl From<alloy_rlp::Error> for ECIESError {
    fn from(source: alloy_rlp::Error) -> Self {
        ECIESErrorImpl::RLPDecoding(source).into()
    }
}

impl From<std::num::TryFromIntError> for ECIESError {
    fn from(source: std::num::TryFromIntError) -> Self {
        ECIESErrorImpl::FromInt(source).into()
    }
}
