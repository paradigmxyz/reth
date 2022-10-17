//! Error cases when handling a [`crate::EthStream`]
use std::io;

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
}

#[derive(thiserror::Error, Debug)]
#[allow(missing_docs)]
pub enum HandshakeError {
    #[error("status message can only be recv/sent in handshake")]
    StatusInHandshake,
    #[error("received non-status message when trying to handshake")]
    NonStatusMessageInHandshake,
    #[error("no response received when sending out handshake")]
    NoResponse,
    #[error(transparent)]
    InvalidFork(#[from] ValidationError),
}
