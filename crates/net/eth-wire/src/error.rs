//! Error cases when handling a [`crate::EthStream`]
use std::io;

use reth_primitives::{Chain, H256};

use crate::types::forkid::ValidationError;

/// Errors when sending/receiving messages
#[derive(thiserror::Error, Debug)]
#[allow(missing_docs)]
pub enum EthStreamError {
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error(transparent)]
    Rlp(#[from] reth_rlp::DecodeError),
    #[error(transparent)]
    HandshakeError(#[from] HandshakeError),
    #[error("message size ({0}) exceeds max length (10MB)")]
    MessageTooBig(usize),
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
