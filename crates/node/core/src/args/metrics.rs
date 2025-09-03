use clap::Parser;
use reth_cli_util::parse_socket_address;
use std::net::SocketAddr;

/// Endpoints configuration, metrics and spans can be exported
///
/// Either to prometheus endpoint either to an OTLP endpoint.
///
/// Only http is supported.
#[derive(Debug, Clone, Default, Parser)]
pub struct MetricsArgs {
    /// Enable Prometheus metrics export.
    ///
    /// The metrics will be served at the given interface and port.
    #[arg(long, value_name = "SOCKET", value_parser = parse_socket_address, help_heading = "Metrics")]
    pub prometheus: Option<SocketAddr>,

    /// Enable OTLP export.
    ///
    /// The metrics (and spans) will be exported to the given endpoint - interface and port.
    #[arg(long, value_name = "OTLP", value_parser = parse_socket_address, help_heading = "Metrics")]
    pub otlp: Option<SocketAddr>,
}
