//! Error cases when handling a [`crate::EthStream`]
use std::io;

use reth_primitives::{Chain, ValidationError, H256};

use crate::{
    capability::SharedCapabilityError, disconnect::UnknownDisconnectReason, DisconnectReason,
};

/// Errors when sending/receiving messages
#[derive(thiserror::Error, Debug)]
#[allow(missing_docs)]
pub enum EthStreamError {
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error(transparent)]
    Rlp(#[from] reth_rlp::DecodeError),
    #[error(transparent)]
    P2PStreamError(#[from] P2PStreamError),
    #[error(transparent)]
    HandshakeError(#[from] HandshakeError),
    #[error("message size ({0}) exceeds max length (10MB)")]
    MessageTooBig(usize),
}

// === impl EthStreamError ===

impl EthStreamError {
    /// Returns the [`DisconnectReason`] if the error is a disconnect message
    pub fn as_disconnected(&self) -> Option<DisconnectReason> {
        if let EthStreamError::P2PStreamError(err) = self {
            err.as_disconnected()
        } else {
            None
        }
    }
}

#[derive(thiserror::Error, Debug)]
#[allow(missing_docs)]
pub enum HandshakeError {
    #[error("status message can only be recv/sent in handshake")]
    StatusNotInHandshake,
    #[error("received non-status message when trying to handshake")]
    NonStatusMessageInHandshake,
    #[error("no response received when sending out handshake")]
    NoResponse,
    #[error(transparent)]
    InvalidFork(#[from] ValidationError),
    #[error("mismatched genesis in Status message. expected: {expected:?}, got: {got:?}")]
    MismatchedGenesis { expected: H256, got: H256 },
    #[error("mismatched protocol version in Status message. expected: {expected:?}, got: {got:?}")]
    MismatchedProtocolVersion { expected: u8, got: u8 },
    #[error("mismatched chain in Status message. expected: {expected:?}, got: {got:?}")]
    MismatchedChain { expected: Chain, got: Chain },
}

/// Errors when sending/receiving p2p messages. These should result in kicking the peer.
#[derive(thiserror::Error, Debug)]
#[allow(missing_docs)]
pub enum P2PStreamError {
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error(transparent)]
    Rlp(#[from] reth_rlp::DecodeError),
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
    ParseVersionError(#[from] SharedCapabilityError),
    #[error("mismatched protocol version in Hello message. expected: {expected:?}, got: {got:?}")]
    MismatchedProtocolVersion { expected: u8, got: u8 },
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
    #[error("Disconnected by peer: {0}")]
    Disconnected(DisconnectReason),
    #[error("error decoding a message during handshake: {0}")]
    DecodeError(#[from] reth_rlp::DecodeError),
}

/// An error that can occur when interacting with a [`Pinger`].
#[derive(Debug, thiserror::Error)]
pub enum PingerError {
    /// An unexpected pong was received while the pinger was in the `Ready` state.
    #[error("pong received while ready")]
    UnexpectedPong,
}
