//! Error handling for (`EthStream`)[`crate::EthStream`]

use crate::{
    errors::P2PStreamError, message::MessageError, version::ParseVersionError, DisconnectReason,
};
use alloy_primitives::B256;
use reth_chainspec::Chain;
use reth_primitives::{GotExpected, GotExpectedBoxed, ValidationError};
use std::io;

/// Errors when sending/receiving messages
#[derive(thiserror::Error, Debug)]
pub enum EthStreamError {
    #[error(transparent)]
    /// Error of the underlying P2P connection.
    P2PStreamError(#[from] P2PStreamError),
    #[error(transparent)]
    /// Failed to parse peer's version.
    ParseVersionError(#[from] ParseVersionError),
    #[error(transparent)]
    /// Failed Ethereum handshake.
    EthHandshakeError(#[from] EthHandshakeError),
    /// Thrown when decoding a message message failed.
    #[error(transparent)]
    InvalidMessage(#[from] MessageError),
    #[error("message size ({0}) exceeds max length (10MB)")]
    /// Received a message whose size exceeds the standard limit.
    MessageTooBig(usize),
    #[error("TransactionHashes invalid len of fields: hashes_len={hashes_len} types_len={types_len} sizes_len={sizes_len}")]
    /// Received malformed transaction hashes message with discrepancies in field lengths.
    TransactionHashesInvalidLenOfFields {
        /// The number of transaction hashes.
        hashes_len: usize,
        /// The number of transaction types.
        types_len: usize,
        /// The number of transaction sizes.
        sizes_len: usize,
    },
    /// Error when data is not received from peer for a prolonged period.
    #[error("never received data from remote peer")]
    StreamTimeout,
}

// === impl EthStreamError ===

impl EthStreamError {
    /// Returns the [`DisconnectReason`] if the error is a disconnect message
    pub const fn as_disconnected(&self) -> Option<DisconnectReason> {
        if let Self::P2PStreamError(err) = self {
            err.as_disconnected()
        } else {
            None
        }
    }

    /// Returns the [`io::Error`] if it was caused by IO
    pub const fn as_io(&self) -> Option<&io::Error> {
        if let Self::P2PStreamError(P2PStreamError::Io(io)) = self {
            return Some(io)
        }
        None
    }
}

impl From<io::Error> for EthStreamError {
    fn from(err: io::Error) -> Self {
        P2PStreamError::from(err).into()
    }
}

/// Error  that can occur during the `eth` sub-protocol handshake.
#[derive(thiserror::Error, Debug)]
pub enum EthHandshakeError {
    /// Status message received or sent outside of the handshake process.
    #[error("status message can only be recv/sent in handshake")]
    StatusNotInHandshake,
    /// Receiving a non-status message during the handshake phase.
    #[error("received non-status message when trying to handshake")]
    NonStatusMessageInHandshake,
    #[error("no response received when sending out handshake")]
    /// No response received during the handshake process.
    NoResponse,
    #[error(transparent)]
    /// Invalid fork data.
    InvalidFork(#[from] ValidationError),
    #[error("mismatched genesis in status message: {0}")]
    /// Mismatch in the genesis block during status exchange.
    MismatchedGenesis(GotExpectedBoxed<B256>),
    #[error("mismatched protocol version in status message: {0}")]
    /// Mismatched protocol versions in status messages.
    MismatchedProtocolVersion(GotExpected<u8>),
    #[error("mismatched chain in status message: {0}")]
    /// Mismatch in chain details in status messages.
    MismatchedChain(GotExpected<Chain>),
    #[error("total difficulty bitlen is too large: got {got}, maximum {maximum}")]
    /// Excessively large total difficulty bit lengths.
    TotalDifficultyBitLenTooLarge {
        /// The actual bit length of the total difficulty.
        got: usize,
        /// The maximum allowed bit length for the total difficulty.
        maximum: usize,
    },
}
