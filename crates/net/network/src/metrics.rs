use metrics::{Counter, Gauge};
use reth_metrics_derive::Metrics;

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

    /// Total Number of incoming connections
    pub(crate) incoming_connections: Counter,

    /// Total Number of outgoing connections
    pub(crate) outgoing_connections: Counter,

    /// Number of invalid/malformed messages received from peers
    pub(crate) invalid_messages_received: Counter,
}
