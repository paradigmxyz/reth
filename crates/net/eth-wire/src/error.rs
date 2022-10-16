//! Error cases when handling a [`crate::EthStream`]
use std::io;

/// Errors when sending/receiving messages
#[derive(thiserror::Error, Debug)]
#[allow(missing_docs)]
pub enum EthStreamError {
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error(transparent)]
    Rlp(#[from] reth_rlp::DecodeError),
    #[error("status message can only be recv/sent in handshake")]
    StatusInHandshake,
}
