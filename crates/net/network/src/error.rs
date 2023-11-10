//! Possible errors when interacting with the network.

use crate::session::PendingSessionHandshakeError;
use reth_dns_discovery::resolver::ResolveError;
use reth_eth_wire::{
    errors::{EthHandshakeError, EthStreamError, P2PHandshakeError, P2PStreamError},
    DisconnectReason,
};
use std::{fmt, io, io::ErrorKind, net::SocketAddr};

/// Service kind.
#[derive(Debug, PartialEq)]
pub enum ServiceKind {
    /// Listener service.
    Listener(SocketAddr),
    /// Discovery service.
    Discovery(SocketAddr),
}

impl fmt::Display for ServiceKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ServiceKind::Listener(addr) => write!(f, "{addr} (listener service)"),
            ServiceKind::Discovery(addr) => write!(f, "{addr} (discovery service)"),
        }
    }
}

/// All error variants for the network
#[derive(Debug, thiserror::Error)]
pub enum NetworkError {
    /// General IO error.
    #[error(transparent)]
    Io(#[from] io::Error),
    /// Error when an address is already in use.
    #[error("address {kind} is already in use (os error 98)")]
    AddressAlreadyInUse {
        /// Service kind.
        kind: ServiceKind,
        /// IO error.
        error: io::Error,
    },
    /// IO error when creating the discovery service
    #[error("failed to launch discovery service: {0}")]
    Discovery(io::Error),
    /// Error when setting up the DNS resolver failed
    ///
    /// See also [DnsResolver](reth_dns_discovery::DnsResolver::from_system_conf)
    #[error("failed to configure DNS resolver: {0}")]
    DnsResolver(#[from] ResolveError),
}

impl NetworkError {
    /// Converts a `std::io::Error` to a more descriptive `NetworkError`.
    pub fn from_io_error(err: io::Error, kind: ServiceKind) -> Self {
        match err.kind() {
            ErrorKind::AddrInUse => NetworkError::AddressAlreadyInUse { kind, error: err },
            _ => {
                if let ServiceKind::Discovery(_) = kind {
                    return NetworkError::Discovery(err)
                }
                NetworkError::Io(err)
            }
        }
    }
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
    /// a superset of [`Self::merits_discovery_ban`] which checks if the peer should not be part
    /// of the gossip network.
    fn is_fatal_protocol_error(&self) -> bool;

    /// Whether we should backoff.
    ///
    /// Returns the severity of the backoff that should be applied, or `None`, if no backoff should
    /// be applied.
    ///
    /// In case of `Some(BackoffKind)` will temporarily prevent additional
    /// connection attempts.
    fn should_backoff(&self) -> Option<BackoffKind>;
}

/// Describes the type of backoff should be applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackoffKind {
    /// Use the lowest configured backoff duration.
    ///
    /// This applies to connection problems where there is a chance that they will be resolved
    /// after the short duration.
    Low,
    /// Use a slightly higher duration to put a peer in timeout
    ///
    /// This applies to more severe connection problems where there is a lower chance that they
    /// will be resolved.
    Medium,
    /// Use the max configured backoff duration.
    ///
    /// This is intended for spammers, or bad peers in general.
    High,
}

// === impl BackoffKind ===

impl BackoffKind {
    /// Returns true if the backoff is considered severe.
    pub(crate) fn is_severe(&self) -> bool {
        matches!(self, BackoffKind::Medium | BackoffKind::High)
    }
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
            EthStreamError::EthHandshakeError(err) => !matches!(err, EthHandshakeError::NoResponse),
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
                        P2PStreamError::ParseSharedCapability(_) |
                        P2PStreamError::CapabilityNotShared |
                        P2PStreamError::Disconnected(DisconnectReason::UselessPeer) |
                        P2PStreamError::Disconnected(
                            DisconnectReason::IncompatibleP2PProtocolVersion
                        ) |
                        P2PStreamError::Disconnected(DisconnectReason::ProtocolBreach) |
                        P2PStreamError::MismatchedProtocolVersion { .. }
                )
            }
            EthStreamError::EthHandshakeError(err) => !matches!(err, EthHandshakeError::NoResponse),
            _ => false,
        }
    }

    fn should_backoff(&self) -> Option<BackoffKind> {
        if let Some(err) = self.as_io() {
            return err.should_backoff()
        }

        if let Some(err) = self.as_disconnected() {
            return match err {
                DisconnectReason::TooManyPeers |
                DisconnectReason::AlreadyConnected |
                DisconnectReason::PingTimeout |
                DisconnectReason::DisconnectRequested |
                DisconnectReason::TcpSubsystemError => Some(BackoffKind::Low),

                DisconnectReason::ProtocolBreach |
                DisconnectReason::UselessPeer |
                DisconnectReason::IncompatibleP2PProtocolVersion |
                DisconnectReason::NullNodeIdentity |
                DisconnectReason::ClientQuitting |
                DisconnectReason::UnexpectedHandshakeIdentity |
                DisconnectReason::ConnectedToSelf |
                DisconnectReason::SubprotocolSpecific => {
                    // These are considered fatal, and are handled by the
                    // [`SessionError::is_fatal_protocol_error`]
                    Some(BackoffKind::High)
                }
            }
        }

        // This only checks for a subset of error variants, the counterpart of
        // [`SessionError::is_fatal_protocol_error`]
        match self {
            // timeouts
            EthStreamError::EthHandshakeError(EthHandshakeError::NoResponse) |
            EthStreamError::P2PStreamError(P2PStreamError::HandshakeError(
                P2PHandshakeError::NoResponse,
            )) |
            EthStreamError::P2PStreamError(P2PStreamError::PingTimeout) => Some(BackoffKind::Low),
            // malformed messages
            EthStreamError::P2PStreamError(P2PStreamError::Rlp(_)) |
            EthStreamError::P2PStreamError(P2PStreamError::UnknownReservedMessageId(_)) |
            EthStreamError::P2PStreamError(P2PStreamError::UnknownDisconnectReason(_)) |
            EthStreamError::P2PStreamError(P2PStreamError::MessageTooBig { .. }) |
            EthStreamError::P2PStreamError(P2PStreamError::EmptyProtocolMessage) |
            EthStreamError::P2PStreamError(P2PStreamError::PingerError(_)) |
            EthStreamError::P2PStreamError(P2PStreamError::Snap(_)) => Some(BackoffKind::Medium),
            _ => None,
        }
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

    fn should_backoff(&self) -> Option<BackoffKind> {
        match self {
            PendingSessionHandshakeError::Eth(eth) => eth.should_backoff(),
            PendingSessionHandshakeError::Ecies(_) => Some(BackoffKind::Low),
        }
    }
}

impl SessionError for io::Error {
    fn merits_discovery_ban(&self) -> bool {
        false
    }

    fn is_fatal_protocol_error(&self) -> bool {
        false
    }

    fn should_backoff(&self) -> Option<BackoffKind> {
        match self.kind() {
            // these usually happen when the remote instantly drops the connection, for example
            // if the previous connection isn't properly cleaned up yet and the peer is temp.
            // banned.
            ErrorKind::ConnectionReset | ErrorKind::BrokenPipe => Some(BackoffKind::Low),
            ErrorKind::ConnectionRefused => {
                // peer is unreachable, e.g. port not open or down
                Some(BackoffKind::High)
            }
            _ => Some(BackoffKind::Medium),
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
        assert_eq!(err.should_backoff(), Some(BackoffKind::Low));

        let err = EthStreamError::P2PStreamError(P2PStreamError::HandshakeError(
            P2PHandshakeError::NoResponse,
        ));
        assert_eq!(err.should_backoff(), Some(BackoffKind::Low));
    }
}
