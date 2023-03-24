use metrics::{Counter, Gauge};
use reth_metrics_derive::Metrics;

/// Metrics for the entire network, handled by NetworkManager
#[derive(Metrics)]
#[metrics(scope = "network")]
pub struct NetworkMetrics {
    /// Number of currently connected peers
    pub(crate) connected_peers: Gauge,

    /// Number of peers known to the node
    pub(crate) tracked_peers: Gauge,

    /// Cumulative number of failures of pending sessions
    pub(crate) pending_session_failures: Counter,

    /// Total number of sessions closed
    pub(crate) closed_sessions: Counter,

    /// Number of active incoming connections
    pub(crate) incoming_connections: Gauge,

    /// Number of active outgoing connections
    pub(crate) outgoing_connections: Gauge,

    /// Total Number of incoming connections handled
    pub(crate) total_incoming_connections: Counter,

    /// Total Number of outgoing connections established
    pub(crate) total_outgoing_connections: Counter,

    /// Number of invalid/malformed messages received from peers
    pub(crate) invalid_messages_received: Counter,
}

/// Metrics for the TransactionsManager
#[derive(Metrics)]
#[metrics(scope = "network")]
pub struct TransactionsManagerMetrics {
    /// Total number of propagated transactions
    pub(crate) propagated_transactions: Counter,
}

/// Metrics for Disconnection types
///
/// These are just counters, and ideally we would implement these metrics on a peer-by-peer basis,
/// in that we do not double-count peers for `TooManyPeers` if we make an outgoing connection and
/// get disconnected twice
#[derive(Metrics)]
#[metrics(scope = "network")]
pub struct DisconnectMetrics {
    /// Number of peer disconnects due to DisconnectRequestd (0x00)
    pub(crate) disconnect_requested: Counter,

    /// Number of peer disconnects due to TcpSubsystemError (0x01)
    pub(crate) tcp_subsystem_error: Counter,

    /// Number of peer disconnects due to ProtocolBreach (0x02)
    pub(crate) protocol_breach: Counter,

    /// Number of peer disconnects due to UselessPeer (0x03)
    pub(crate) useless_peer: Counter,

    /// Number of peer disconnects due to TooManyPeers (0x04)
    pub(crate) too_many_peers: Counter,

    /// Number of peer disconnects due to AlreadyConnected (0x05)
    pub(crate) already_connected: Counter,

    /// Number of peer disconnects due to IncompatibleP2PProtocolVersion (0x06)
    pub(crate) incompatible: Counter,

    /// Number of peer disconnects due to NullNodeIdentity (0x07)
    pub(crate) null_node_identity: Counter,

    /// Number of peer disconnects due to ClientQuitting (0x08)
    pub(crate) client_quitting: Counter,

    /// Number of peer disconnects due to UnexpectedHandshakeIdentity (0x09)
    pub(crate) unexpected_identity: Counter,

    /// Number of peer disconnects due to ConnectedToSelf (0x0a)
    pub(crate) connected_to_self: Counter,

    /// Number of peer disconnects due to PingTimeout (0x0b)
    pub(crate) ping_timeout: Counter,

    /// Number of peer disconnects due to SubprotocolSpecific (0x10)
    pub(crate) subprotocol_specific: Counter,
}
