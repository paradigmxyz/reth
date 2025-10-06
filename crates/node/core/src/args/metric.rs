use clap::Parser;
use reth_cli_util::parse_socket_address;
use std::net::SocketAddr;

/// Telemetry exporters configuration.
///
/// - Prometheus: serves metrics over http.
/// - OTLP: pushes metrics to an `OpenTelemetry` collector via http.
#[derive(Debug, Clone, Default, Parser)]
pub struct MetricArgs {
    /// Enable Prometheus metrics.
    ///
    /// The metrics will be served at the given interface and port.
    #[arg(long="metrics", alias = "metrics.prometheus", value_name = "PROMETHEUS", value_parser = parse_socket_address, help_heading = "Metrics")]
    pub prometheus: Option<SocketAddr>,
}
