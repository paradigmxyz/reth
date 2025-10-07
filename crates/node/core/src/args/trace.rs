//! Opentelemetry tracing configuration through CLI args.

use clap::Parser;
use eyre::{ensure, WrapErr};
use reth_tracing_otlp::TraceOutput;
use url::Url;

/// CLI arguments for configuring `Opentelemetry` trace and span export.
#[derive(Debug, Clone, Default, Parser)]
pub struct TraceArgs {
    /// Enable `Opentelemetry` tracing export.
    ///
    /// If no value provided, traces are printed to stdout.
    /// If a URL is provided, traces are exported to that OTLP endpoint.
    /// Example: --tracing-otlp=http://localhost:4318/v1/traces
    #[arg(
        long = "tracing-otlp",
        global = true,
        value_name = "URL",
        num_args = 0..=1,
        default_missing_value = "stdout",
        require_equals = true,
        value_parser = parse_trace_output,
        help_heading = "Tracing"
    )]
    pub otlp: Option<TraceOutput>,
}

// Parses the trace output destination from a string.
fn parse_trace_output(s: &str) -> eyre::Result<TraceOutput> {
    if s == "stdout" {
        return Ok(TraceOutput::Stdout);
    }

    let url = Url::parse(s).wrap_err("Invalid URL for trace output")?;

    // OTLP specification requires the `/v1/traces` path for trace endpoints
    ensure!(
        url.path().ends_with("/v1/traces"),
        "OTLP trace endpoint must end with /v1/traces, got path: {}",
        url.path()
    );

    Ok(TraceOutput::Otlp(url))
}
