//! Opentelemetry tracing configuration through CLI args.

use clap::Parser;
use eyre::WrapErr;
use reth_tracing_otlp::OtlpProtocol;
use tracing::Level;
use url::Url;

/// CLI arguments for configuring `Opentelemetry` trace and span export.
#[derive(Debug, Clone, Parser)]
pub struct TraceArgs {
    /// Enable `Opentelemetry` tracing export to an OTLP endpoint.
    ///
    /// If no value provided, defaults based on protocol:
    /// - HTTP: `http://localhost:4318/v1/traces`
    /// - gRPC: `http://localhost:4317`
    ///
    /// Example: --tracing-otlp=http://collector:4318/v1/traces
    #[arg(
        long = "tracing-otlp",
        global = true,
        value_name = "URL",
        num_args = 0..=1,
        default_missing_value = "http://localhost:4318/v1/traces",
        require_equals = true,
        value_parser = parse_otlp_endpoint,
        help_heading = "Tracing"
    )]
    pub otlp: Option<Url>,

    /// OTLP transport protocol to use for exporting traces.
    ///
    /// - `http`: expects endpoint path to end with `/v1/traces`
    /// - `grpc`: expects endpoint without a path
    ///
    /// Defaults to HTTP if not specified.
    #[arg(
        long = "tracing-otlp-protocol",
        global = true,
        value_name = "PROTOCOL",
        default_value = "http",
        help_heading = "Tracing"
    )]
    pub protocol: OtlpProtocol,

    /// Set the minimum log level for OTLP traces.
    ///
    /// Valid values: ERROR, WARN, INFO, DEBUG, TRACE
    ///
    /// Defaults to TRACE if not specified.
    #[arg(
        long = "tracing-otlp-level",
        global = true,
        value_name = "LEVEL",
        default_value = "TRACE",
        help_heading = "Tracing"
    )]
    pub otlp_level: Level,
}

impl Default for TraceArgs {
    fn default() -> Self {
        Self { otlp: None, protocol: OtlpProtocol::Http, otlp_level: Level::TRACE }
    }
}

impl TraceArgs {
    /// Validate the configuration
    pub fn validate(&self) -> eyre::Result<()> {
        if let Some(url) = &self.otlp {
            self.protocol.validate_endpoint(url)?;
        }
        Ok(())
    }
}

// Parses an OTLP endpoint url.
fn parse_otlp_endpoint(arg: &str) -> eyre::Result<Url> {
    Url::parse(arg).wrap_err("Invalid URL for OTLP trace output")
}
