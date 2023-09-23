//! Error types that can occur in this crate.

use tokio::sync::{mpsc::error::SendError, oneshot::error::RecvError};

/// Error thrown when decoding a UDP packet.
#[derive(Debug, thiserror::Error)]
#[allow(missing_docs)]
pub enum DecodePacketError {
    #[error("Failed to rlp decode: {0:?}")]
    Rlp(#[from] alloy_rlp::Error),
    #[error("Received packet len too short.")]
    PacketTooShort,
    #[error("Hash of the header not equals to the hash of the data.")]
    HashMismatch,
    #[error("Message id {0} is not supported.")]
    UnknownMessage(u8),
    #[error("Failed to recover public key: {0:?}")]
    Secp256k1(#[from] secp256k1::Error),
}

/// High level errors that can occur when interacting with the discovery service
#[derive(Debug, thiserror::Error)]
pub enum Discv4Error {
    /// Failed to send a command over the channel
    #[error("Failed to send on a closed channel")]
    Send,
    /// Failed to receive a command response
    #[error(transparent)]
    Receive(#[from] RecvError),
}

impl<T> From<SendError<T>> for Discv4Error {
    fn from(_: SendError<T>) -> Self {
        Discv4Error::Send
    }
}
