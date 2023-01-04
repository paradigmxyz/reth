use metrics::{Counter, Gauge};
use reth_metrics_derive::Metrics;

#[derive(Metrics)]
#[metrics(scope = "network_manager")]
pub struct NetworkMetrics {
    /// Number of currently connected peers
    pub(crate) num_connected_peers: Gauge,

    /// Cumulative number of failures of pending sessions
    pub(crate) pending_session_failures: Counter,

    /// Total number of sessions closed
    pub(crate) total_closed_sessions: Counter,

    /// Total Number of incoming connections
    pub(crate) num_incoming_connections: Counter,
}
