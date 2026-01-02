use clap::Parser;
use reth_cli_util::{parse_duration_from_secs, parse_socket_address};
use std::{net::SocketAddr, time::Duration};

/// Metrics configuration.
#[derive(Debug, Clone, Default, Parser)]
pub struct MetricArgs {
    /// Enable Prometheus metrics.
    ///
    /// The metrics will be served at the given interface and port.
    #[arg(long="metrics", alias = "metrics.prometheus", value_name = "PROMETHEUS", value_parser = parse_socket_address, help_heading = "Metrics")]
    pub prometheus: Option<SocketAddr>,

    /// URL for pushing Prometheus metrics to a push gateway.
    ///
    /// If set, the node will periodically push metrics to the specified push gateway URL.
    #[arg(
        long = "metrics.prometheus.push.url",
        value_name = "PUSH_GATEWAY_URL",
        help_heading = "Metrics"
    )]
    pub push_gateway_url: Option<String>,

    /// Interval in seconds for pushing metrics to push gateway.
    ///
    /// Default: 5 seconds
    #[arg(
        long = "metrics.prometheus.push.interval",
        default_value = "5",
        value_parser = parse_duration_from_secs,
        value_name = "SECONDS",
        help_heading = "Metrics"
    )]
    pub push_gateway_interval: Duration,
}
