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
        env = "OTEL_EXPORTER_OTLP_PROTOCOL",
        global = true,
        value_name = "PROTOCOL",
        default_value = "http",
        help_heading = "Tracing"
    )]
    pub protocol: OtlpProtocol,

    /// Set a filter directive for the OTLP tracer. This controls the verbosity
    /// of spans and events sent to the OTLP endpoint. It follows the same
    /// syntax as the `RUST_LOG` environment variable.
    ///
    /// Example: --tracing-otlp.filter=info,reth=debug,hyper_util=off
    ///
    /// Defaults to TRACE if not specified.
    #[arg(
        long = "tracing-otlp.filter",
        global = true,
        value_name = "FILTER",
        default_value = "debug",
        help_heading = "Tracing"
    )]
    pub otlp_filter: EnvFilter,

    /// Trace sampling ratio to control the percentage of traces to export.
    ///
    /// Valid range: 0.0 to 1.0
    /// - 1.0, default: Sample all traces
    /// - 0.01: Sample 1% of traces
    /// - 0.0: Disable sampling
    ///
    /// Example: --tracing-otlp-sample-ratio=0.0.
    #[arg(
        long = "tracing-otlp-sample-ratio",
        env = "OTEL_TRACES_SAMPLER_ARG",
        global = true,
        value_name = "RATIO",
        help_heading = "Tracing"
    )]
    pub sample_ratio: Option<f64>,
}

impl Default for TraceArgs {
    fn default() -> Self {
        Self {
            otlp: None,
            protocol: OtlpProtocol::Http,
            otlp_filter: EnvFilter::from_default_env(),
            sample_ratio: None,
        }
    }
}

impl TraceArgs {
    /// Validate the configuration
    pub fn validate(&mut self) -> eyre::Result<()> {
        if let Some(url) = &mut self.otlp {
            self.protocol.validate_endpoint(url)?;
        }

        if let Some(ratio) = &mut self.sample_ratio {
            ensure!(
                (0.0..=1.0).contains(ratio),
                "Sample ratio must be between 0.0 and 1.0, got: {}",
                ratio
            )
        }
        Ok(())
    }
}

// Parses an OTLP endpoint url.
fn parse_otlp_endpoint(arg: &str) -> eyre::Result<Url> {
    Url::parse(arg).wrap_err("Invalid URL for OTLP trace output")
}
