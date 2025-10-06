use clap::Parser;
use reth_cli_util::parse_socket_address;
use std::net::SocketAddr;

/// Opentelemetry tracing configuration through CLI args.
#[derive(Debug, Clone, Default, Parser)]
pub struct TraceArgs {
    /// Enable OTLP tracing.
    ///
    /// The otlp tracing will be enabled and the traces exported to this endpoint.
    #[arg(
        long = "tracing-otlp",
        global = true,
        alias = "tracing.otlp",
        value_name = "TRACING-OTLP",
        value_parser = parse_socket_address,
        help_heading = "Tracing"
    )]
    pub otlp: Option<SocketAddr>,
}
