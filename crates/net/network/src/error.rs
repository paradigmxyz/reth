//! Possible errors when interacting with the network.

use crate::session::PendingSessionHandshakeError;
use reth_eth_wire::{
    error::{EthStreamError, HandshakeError, P2PHandshakeError, P2PStreamError},
    DisconnectReason,
};
use std::fmt;

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

/// Abstraction over errors that can lead to a failed session
#[auto_impl::auto_impl(&)]
pub(crate) trait SessionError: fmt::Debug {
    /// Returns true if the error indicates that the corresponding peer should be removed from peer
    /// discovery, for example if it's using a different genesis hash.
    fn merits_discovery_ban(&self) -> bool;

    /// Returns true if the error indicates that we'll never be able to establish a connection to
    /// that peer. For example, not matching capabilities or a mismatch in protocols.
    ///
    /// Note: This does not necessarily mean that either of the peers are in violation of the
    /// protocol but rather that they'll never be able to connect with each other. This check is
    /// a superset of [`error_merits_discovery_ban`] which checks if the peer should not be part
    /// of the gossip network.
    fn is_fatal_protocol_error(&self) -> bool;

    /// Returns true if the error should lead to backoff, temporarily preventing additional
    /// connection attempts
    fn should_backoff(&self) -> bool;
}

impl SessionError for EthStreamError {
    fn merits_discovery_ban(&self) -> bool {
        match self {
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

    fn is_fatal_protocol_error(&self) -> bool {
        match self {
            EthStreamError::P2PStreamError(err) => {
                matches!(
                    err,
                    P2PStreamError::HandshakeError(P2PHandshakeError::NoSharedCapabilities) |
                        P2PStreamError::HandshakeError(P2PHandshakeError::HelloNotInHandshake) |
                        P2PStreamError::HandshakeError(
                            P2PHandshakeError::NonHelloMessageInHandshake
                        ) |
                        P2PStreamError::HandshakeError(P2PHandshakeError::Disconnected(
                            DisconnectReason::UselessPeer
                        )) |
                        P2PStreamError::HandshakeError(P2PHandshakeError::Disconnected(
                            DisconnectReason::IncompatibleP2PProtocolVersion
                        )) |
                        P2PStreamError::HandshakeError(P2PHandshakeError::Disconnected(
                            DisconnectReason::ProtocolBreach
                        )) |
                        P2PStreamError::UnknownReservedMessageId(_) |
                        P2PStreamError::EmptyProtocolMessage |
                        P2PStreamError::ParseVersionError(_) |
                        P2PStreamError::Disconnected(DisconnectReason::UselessPeer) |
                        P2PStreamError::Disconnected(
                            DisconnectReason::IncompatibleP2PProtocolVersion
                        ) |
                        P2PStreamError::Disconnected(DisconnectReason::ProtocolBreach) |
                        P2PStreamError::MismatchedProtocolVersion { .. }
                )
            }
            EthStreamError::HandshakeError(err) => !matches!(err, HandshakeError::NoResponse),
            _ => false,
        }
    }

    fn should_backoff(&self) -> bool {
        self.as_disconnected()
            .map(|reason| {
                matches!(
                    reason,
                    DisconnectReason::TooManyPeers | DisconnectReason::AlreadyConnected
                )
            })
            .unwrap_or_default()
    }
}

impl SessionError for PendingSessionHandshakeError {
    fn merits_discovery_ban(&self) -> bool {
        match self {
            PendingSessionHandshakeError::Eth(eth) => eth.merits_discovery_ban(),
            PendingSessionHandshakeError::Ecies(_) => true,
        }
    }

    fn is_fatal_protocol_error(&self) -> bool {
        match self {
            PendingSessionHandshakeError::Eth(eth) => eth.is_fatal_protocol_error(),
            PendingSessionHandshakeError::Ecies(_) => true,
        }
    }

    fn should_backoff(&self) -> bool {
        match self {
            PendingSessionHandshakeError::Eth(eth) => eth.should_backoff(),
            PendingSessionHandshakeError::Ecies(_) => true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_fatal_disconnect() {
        let err = PendingSessionHandshakeError::Eth(EthStreamError::P2PStreamError(
            P2PStreamError::HandshakeError(P2PHandshakeError::Disconnected(
                DisconnectReason::UselessPeer,
            )),
        ));

        assert!(err.is_fatal_protocol_error());
    }

    #[test]
    fn test_should_backoff() {
        let err = EthStreamError::P2PStreamError(P2PStreamError::HandshakeError(
            P2PHandshakeError::Disconnected(DisconnectReason::TooManyPeers),
        ));

        assert_eq!(err.as_disconnected(), Some(DisconnectReason::TooManyPeers));
        assert!(err.should_backoff());
    }
}
