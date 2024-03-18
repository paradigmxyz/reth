//! Error handling for (`EthStream`)[crate::EthStream]

use crate::{
    errors::{MuxDemuxError, P2PStreamError},
    version::ParseVersionError,
    DisconnectReason, EthMessageID, EthVersion,
};
use alloy_chains::Chain;
use reth_primitives::{GotExpected, GotExpectedBoxed, ValidationError, B256};
use std::io;

/// Errors when sending/receiving messages
#[derive(thiserror::Error, Debug)]
pub enum EthStreamError {
    #[error(transparent)]
    /// Error of the underlying P2P connection.
    P2PStreamError(#[from] P2PStreamError),
    #[error(transparent)]
    /// Error of the underlying de-/muxed P2P connection.
    MuxDemuxError(#[from] MuxDemuxError),
    #[error(transparent)]
    /// Failed to parse peer's version.
    ParseVersionError(#[from] ParseVersionError),
    #[error(transparent)]
    /// Failed Ethereum handshake.
    EthHandshakeError(#[from] EthHandshakeError),
    #[error("message id {1:?} is invalid for version {0:?}")]
    /// Flags an unrecognized message ID for a given protocol version.
    EthInvalidMessageError(EthVersion, EthMessageID),
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
}

// === impl EthStreamError ===

impl EthStreamError {
    /// Returns the [`DisconnectReason`] if the error is a disconnect message
    pub fn as_disconnected(&self) -> Option<DisconnectReason> {
        if let EthStreamError::P2PStreamError(err) = self {
            err.as_disconnected()
        } else if let EthStreamError::MuxDemuxError(MuxDemuxError::P2PStreamError(err)) = self {
            err.as_disconnected()
        } else {
            None
        }
    }

    /// Returns the [io::Error] if it was caused by IO
    pub fn as_io(&self) -> Option<&io::Error> {
        if let EthStreamError::P2PStreamError(P2PStreamError::Io(io)) = self {
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

impl From<alloy_rlp::Error> for EthStreamError {
    fn from(err: alloy_rlp::Error) -> Self {
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
