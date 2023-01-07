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
