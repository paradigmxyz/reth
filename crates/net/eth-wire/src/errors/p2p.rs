//! Error handling for [`P2PStream`](crate::P2PStream).

use crate::{
    capability::SharedCapabilityError, disconnect::UnknownDisconnectReason, DisconnectReason,
    ProtocolVersion,
};
use reth_primitives::GotExpected;
use std::io;

/// Errors when sending/receiving p2p messages. These should result in kicking the peer.
#[derive(thiserror::Error, Debug)]
#[allow(missing_docs)]
pub enum P2PStreamError {
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error(transparent)]
    Rlp(#[from] alloy_rlp::Error),
    #[error(transparent)]
    Snap(#[from] snap::Error),
    #[error(transparent)]
    HandshakeError(#[from] P2PHandshakeError),
    #[error("message size ({message_size}) exceeds max length ({max_size})")]
    MessageTooBig { message_size: usize, max_size: usize },
    #[error("unknown reserved p2p message id: {0}")]
    UnknownReservedMessageId(u8),
    #[error("empty protocol message received")]
    EmptyProtocolMessage,
    #[error(transparent)]
    PingerError(#[from] PingerError),
    #[error("ping timed out with")]
    PingTimeout,
    #[error(transparent)]
    ParseSharedCapability(#[from] SharedCapabilityError),
    #[error("capability not supported on stream to this peer")]
    CapabilityNotShared,
    #[error("mismatched protocol version in Hello message: {0}")]
    MismatchedProtocolVersion(GotExpected<ProtocolVersion>),
    #[error("started ping task before the handshake completed")]
    PingBeforeHandshake,
    #[error("too many messages buffered before sending")]
    SendBufferFull,
    #[error("disconnected")]
    Disconnected(DisconnectReason),
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

/// Errors when conducting a p2p handshake
#[derive(thiserror::Error, Debug, Clone, Eq, PartialEq)]
#[allow(missing_docs)]
pub enum P2PHandshakeError {
    #[error("hello message can only be recv/sent in handshake")]
    HelloNotInHandshake,
    #[error("received non-hello message when trying to handshake")]
    NonHelloMessageInHandshake,
    #[error("no capabilities shared with peer")]
    NoSharedCapabilities,
    #[error("no response received when sending out handshake")]
    NoResponse,
    #[error("handshake timed out")]
    Timeout,
    #[error("disconnected by peer: {0}")]
    Disconnected(DisconnectReason),
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
