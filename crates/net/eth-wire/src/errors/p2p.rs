//! Error handling for [`P2PStream`](crate::P2PStream).

use crate::{
    capability::SharedCapabilityError, disconnect::UnknownDisconnectReason, DisconnectReason,
    ProtocolVersion,
};
use reth_primitives::GotExpected;
use std::io;

/// Errors when sending/receiving p2p messages. These should result in kicking the peer.
#[derive(thiserror::Error, Debug)]
pub enum P2PStreamError {
    /// I/O error.
    #[error(transparent)]
    Io(#[from] io::Error),

    /// RLP encoding/decoding error.
    #[error(transparent)]
    Rlp(#[from] alloy_rlp::Error),

    /// Error in compression/decompression using Snappy.
    #[error(transparent)]
    Snap(#[from] snap::Error),

    /// Error during the P2P handshake.
    #[error(transparent)]
    HandshakeError(#[from] P2PHandshakeError),

    /// Message size exceeds maximum length error.
    #[error("message size ({message_size}) exceeds max length ({max_size})")]
    MessageTooBig {
        /// The actual size of the message received.
        message_size: usize,
        /// The maximum allowed size for the message.
        max_size: usize,
    },

    /// Unknown reserved P2P message ID error.
    #[error("unknown reserved p2p message id: {0}")]
    UnknownReservedMessageId(u8),

    /// Empty protocol message received error.
    #[error("empty protocol message received")]
    EmptyProtocolMessage,

    /// Error related to the Pinger.
    #[error(transparent)]
    PingerError(#[from] PingerError),

    /// Ping timeout error.
    #[error("ping timed out with")]
    PingTimeout,

    /// Error parsing shared capabilities.
    #[error(transparent)]
    ParseSharedCapability(#[from] SharedCapabilityError),

    /// Capability not supported on the stream to this peer.
    #[error("capability not supported on stream to this peer")]
    CapabilityNotShared,

    /// Mismatched protocol version error.
    #[error("mismatched protocol version in Hello message: {0}")]
    MismatchedProtocolVersion(GotExpected<ProtocolVersion>),

    /// Ping started before the handshake completed.
    #[error("started ping task before the handshake completed")]
    PingBeforeHandshake,

    /// Too many messages buffered before sending.
    #[error("too many messages buffered before sending")]
    SendBufferFull,

    /// Disconnected error.
    #[error("disconnected")]
    Disconnected(DisconnectReason),

    /// Unknown disconnect reason error.
    #[error("unknown disconnect reason: {0}")]
    UnknownDisconnectReason(#[from] UnknownDisconnectReason),
}

// === impl P2PStreamError ===

impl P2PStreamError {
    /// Returns the [`DisconnectReason`] if it is the `Disconnected` variant.
    pub fn as_disconnected(&self) -> Option<DisconnectReason> {
        let reason = match self {
            P2PStreamError::HandshakeError(P2PHandshakeError::Disconnected(reason)) => reason,
            P2PStreamError::Disconnected(reason) => reason,
            _ => return None,
        };

        Some(*reason)
    }
}

/// Errors when conducting a p2p handshake.
#[derive(thiserror::Error, Debug, Clone, Eq, PartialEq)]
pub enum P2PHandshakeError {
    /// Hello message received/sent outside of handshake error.
    #[error("hello message can only be recv/sent in handshake")]
    HelloNotInHandshake,

    /// Received a non-hello message when trying to handshake.
    #[error("received non-hello message when trying to handshake")]
    NonHelloMessageInHandshake,

    /// No capabilities shared with the peer.
    #[error("no capabilities shared with peer")]
    NoSharedCapabilities,

    /// No response received when sending out handshake.
    #[error("no response received when sending out handshake")]
    NoResponse,

    /// Handshake timed out.
    #[error("handshake timed out")]
    Timeout,

    /// Disconnected by peer with a specific reason.
    #[error("disconnected by peer: {0}")]
    Disconnected(DisconnectReason),

    /// Error decoding a message during handshake.
    #[error("error decoding a message during handshake: {0}")]
    DecodeError(#[from] alloy_rlp::Error),
}

/// An error that can occur when interacting with a pinger.
#[derive(Debug, thiserror::Error)]
pub enum PingerError {
    /// An unexpected pong was received while the pinger was in the `Ready` state.
    #[error("pong received while ready")]
    UnexpectedPong,
}
