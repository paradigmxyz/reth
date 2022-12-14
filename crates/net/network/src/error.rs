//! Possible errors when interacting with the network.

use reth_eth_wire::{
    error::{EthStreamError, HandshakeError, P2PHandshakeError, P2PStreamError},
    DisconnectReason,
};

/// All error variants for the network
#[derive(Debug, thiserror::Error)]
pub enum NetworkError {
    /// General IO error.
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// IO error when creating the discovery service
    #[error("Failed to launch discovery service: {0}")]
    Discovery(std::io::Error),
}

/// Returns true if the error indicates that the corresponding peer should be removed from peer
/// discover
pub(crate) fn error_merits_discovery_ban(err: &EthStreamError) -> bool {
    match err {
        EthStreamError::P2PStreamError(P2PStreamError::HandshakeError(
            P2PHandshakeError::HelloNotInHandshake,
        )) |
        EthStreamError::P2PStreamError(P2PStreamError::HandshakeError(
            P2PHandshakeError::NonHelloMessageInHandshake,
        )) => true,
        EthStreamError::HandshakeError(err) => !matches!(err, HandshakeError::NoResponse),
        _ => false,
    }
}

/// Returns true if the error indicates that we'll never be able to establish a connection to that
/// peer. For example, not matching capabilities or a mismatch in protocols.
pub(crate) fn is_fatal_protocol_error(err: &EthStreamError) -> bool {
    match err {
        EthStreamError::P2PStreamError(err) => {
            matches!(
                err,
                P2PStreamError::HandshakeError(P2PHandshakeError::NoSharedCapabilities) |
                    P2PStreamError::UnknownReservedMessageId(_) |
                    P2PStreamError::EmptyProtocolMessage |
                    P2PStreamError::ParseVersionError(_) |
                    P2PStreamError::Disconnected(DisconnectReason::UselessPeer) |
                    P2PStreamError::MismatchedProtocolVersion { .. }
            )
        }
        EthStreamError::HandshakeError(err) => !matches!(err, HandshakeError::NoResponse),
        _ => false,
    }
}
