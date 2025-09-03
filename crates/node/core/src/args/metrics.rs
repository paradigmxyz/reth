use clap::Parser;
use reth_cli_util::parse_socket_address;
use std::net::SocketAddr;

/// Telemetry exporters configuration.
///
/// - Prometheus: serves metrics over http.
/// - OTLP: pushes metrics and spans to an OpenTelemetry collector via http.
#[derive(Debug, Clone, Default, Parser)]
pub struct MetricsArgs {
    /// Enable Prometheus metrics.
    ///
    /// The metrics will be served at the given interface and port.
    #[arg(long, value_name = "SOCKET", value_parser = parse_socket_address, help_heading = "Metrics")]
    pub prometheus: Option<SocketAddr>,

    /// Enable OTLP export.
    ///
    /// The spans and metrics will be exported at the given endpoint - interface and port.
    #[arg(long, value_name = "OTLP", value_parser = parse_socket_address, help_heading = "Metrics")]
    pub otlp: Option<SocketAddr>,
}
