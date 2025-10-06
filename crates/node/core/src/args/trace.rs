//! Opentelemetry tracing configuration through CLI args.

use clap::Parser;
use reth_tracing_otlp::TraceOutput;
use url::Url;

/// CLI arguments for configuring `OpenTelemetry` trace and span export.
#[derive(Debug, Clone, Default, Parser)]
pub struct TraceArgs {
    /// Enable `OpenTelemetry` tracing export.
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
        value_parser = parse_trace_output,
        help_heading = "Tracing"
    )]
    pub otlp: Option<TraceOutput>,
}

fn parse_trace_output(s: &str) -> Result<TraceOutput, String> {
    if s == "stdout" {
        return Ok(TraceOutput::Stdout);
    }

    let url = Url::parse(s).map_err(|e| format!("Invalid URL: {}", e))?;

    // OTLP specification requires the `/v1/traces` path for trace endpoints
    if !url.path().ends_with("/v1/traces") {
        return Err(format!(
            "OTLP trace endpoint must end with /v1/traces, got path: {}",
            url.path()
        ));
    }

    Ok(TraceOutput::Otlp(url))
}
