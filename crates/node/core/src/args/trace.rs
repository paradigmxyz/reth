//! Opentelemetry tracing configuration through CLI args.

use clap::Parser;
use eyre::{ensure, WrapErr};
use tracing::Level;
use url::Url;

/// CLI arguments for configuring `Opentelemetry` trace and span export.
#[derive(Debug, Clone, Parser)]
pub struct TraceArgs {
    /// Enable `Opentelemetry` tracing export to an OTLP endpoint.
    ///
    /// If no value provided, defaults to `http://localhost:4318/v1/traces`.
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
        Self { otlp: None, otlp_level: Level::TRACE }
    }
}

// Parses and validates an OTLP endpoint url.
fn parse_otlp_endpoint(arg: &str) -> eyre::Result<Url> {
    let url = Url::parse(arg).wrap_err("Invalid URL for OTLP trace output")?;

    // OTLP url must end with `/v1/traces` per the OTLP specification.
    ensure!(
        url.path().ends_with("/v1/traces"),
        "OTLP trace endpoint must end with /v1/traces, got path: {}",
        url.path()
    );

    Ok(url)
}
