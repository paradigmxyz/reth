//! Opentelemetry tracing configuration through CLI args.

use clap::Parser;
use eyre::{ensure, WrapErr};
use reth_tracing::tracing_subscriber::EnvFilter;
use reth_tracing_otlp::OtlpProtocol;
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
        // Per specification.
        env = "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT",
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
    /// Set a filter directive for the OTLP tracer. This controls the verbosity
    /// of spans and events sent to the OTLP endpoint. It follows the same
    /// syntax as the `RUST_LOG` environment variable.
    ///
    /// Example: --tracing-otlp-level=info,reth=debug,hyper_util=off
    ///
    /// Defaults to TRACE if not specified.
    #[arg(
        long = "tracing-otlp-level",
        global = true,
        value_name = "FILTER",
        default_value = "TRACE",
        help_heading = "Tracing"
    )]
    pub otlp_level: EnvFilter,
}

impl Default for TraceArgs {
    fn default() -> Self {
        Self { otlp: None, protocol: OtlpProtocol::Http, otlp_level: EnvFilter::from_default_env() }
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

// Parses and validates an OTLP endpoint url.
fn parse_otlp_endpoint(arg: &str) -> eyre::Result<Url> {
    let mut url = Url::parse(arg).wrap_err("Invalid URL for OTLP trace output")?;

    // If the path is empty, we set the path.
    if url.path() == "/" {
        url.set_path("/v1/traces")
    }

    // OTLP url must end with `/v1/traces` per the OTLP specification.
    ensure!(
        url.path().ends_with("/v1/traces"),
        "OTLP trace endpoint must end with /v1/traces, got path: {}",
        url.path()
    );

    Ok(url)
}
