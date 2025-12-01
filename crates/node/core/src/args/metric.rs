use clap::Parser;
use reth_cli_util::{parse_duration_from_secs, parse_socket_address};
use std::{net::SocketAddr, time::Duration};

/// Metrics configuration.
#[derive(Debug, Clone, Default, Parser)]
#[command(next_help_heading = "Metrics")]
pub struct MetricArgs {
    /// Enable Prometheus metrics.
    ///
    /// The metrics will be served at the given interface and port.
    #[arg(long = "metrics", alias = "metrics.prometheus", value_name = "PROMETHEUS", value_parser = parse_socket_address)]
    pub prometheus: Option<SocketAddr>,

    /// URL for pushing Prometheus metrics to a push gateway.
    ///
    /// If set, the node will periodically push metrics to the specified push gateway URL.
    #[arg(long = "metrics.prometheus.push.url", value_name = "PUSH_GATEWAY_URL")]
    pub push_gateway_url: Option<String>,

    /// Interval in seconds for pushing metrics to push gateway.
    ///
    /// Default: 5 seconds
    #[arg(
        long = "metrics.prometheus.push.interval",
        default_value = "5",
        value_parser = parse_duration_from_secs,
        value_name = "SECONDS"
    )]
    pub push_gateway_interval: Duration,

    /// Username for HTTP Basic Authentication to the push gateway.
    ///
    /// Only applies when push gateway URL is configured.
    #[arg(
        long = "metrics.prometheus.push.username",
        value_name = "USERNAME",
        requires = "push_gateway_url",
        env = "RETH_METRICS_PUSH_USERNAME"
    )]
    pub push_gateway_username: Option<String>,

    /// Password for HTTP Basic Authentication to the push gateway.
    ///
    /// Only applies when push gateway URL and username are configured.
    #[arg(
        long = "metrics.prometheus.push.password",
        value_name = "PASSWORD",
        requires = "push_gateway_url",
        requires = "push_gateway_username",
        env = "RETH_METRICS_PUSH_PASSWORD"
    )]
    pub push_gateway_password: Option<String>,
}
