//! Error types that can occur in this crate.

use tokio::sync::{mpsc::error::SendError, oneshot::error::RecvError};

/// Error thrown when decoding a UDP packet.
#[derive(Debug, thiserror::Error)]
pub enum DecodePacketError {
    /// Failed to RLP decode the packet.
    #[error("failed to rlp decode: {0}")]
    /// Indicates a failure to RLP decode the packet.
    Rlp(#[from] alloy_rlp::Error),
    /// Received packet length is too short.
    #[error("received packet length is too short")]
    /// Indicates the received packet length is insufficient.
    PacketTooShort,
    /// Header/data hash mismatch.
    #[error("header/data hash mismatch")]
    /// Indicates a mismatch between header and data hashes.
    HashMismatch,
    /// Unsupported message ID.
    #[error("message ID {0} is not supported")]
    /// Indicates an unsupported message ID.
    UnknownMessage(u8),
    /// Failed to recover public key.
    #[error("failed to recover public key: {0}")]
    /// Indicates a failure to recover the public key.
    Secp256k1(#[from] secp256k1::Error),
}

/// High level errors that can occur when interacting with the discovery service
#[derive(Debug, thiserror::Error)]
pub enum Discv4Error {
    /// Failed to send a command over the channel
    #[error("failed to send on a closed channel")]
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
