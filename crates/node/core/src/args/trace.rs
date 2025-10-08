//! Opentelemetry tracing configuration through CLI args.

use clap::Parser;
use reth_tracing_otlp::TraceOutput;

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
        value_parser = TraceOutput::parse,
        help_heading = "Tracing"
    )]
    pub otlp: Option<TraceOutput>,
}
