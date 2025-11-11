//! Opentelemetry tracing configuration through CLI args.

use clap::Parser;
use eyre::WrapErr;
use reth_tracing::{tracing_subscriber::EnvFilter, Layers};
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
}

impl Default for TraceArgs {
    fn default() -> Self {
        Self {
            otlp: None,
            protocol: OtlpProtocol::Http,
            otlp_filter: EnvFilter::from_default_env(),
        }
    }
}

impl TraceArgs {
    /// Initialize OTLP tracing with the given layers and runner.
    ///
    /// This method handles OTLP tracing initialization based on the configured options,
    /// including validation, protocol selection, and feature flag checking.
    ///
    /// Returns the initialization status to allow callers to log appropriate messages.
    ///
    /// Note: even though this function is async, it does not actually perform any async operations.
    /// It's needed only to be able to initialize the gRPC transport of OTLP tracing that needs to
    /// be called inside a tokio runtime context.
    pub async fn init_otlp_tracing(
        &mut self,
        _layers: &mut Layers,
    ) -> eyre::Result<OtlpInitStatus> {
        if let Some(endpoint) = self.otlp.as_mut() {
            self.protocol.validate_endpoint(endpoint)?;

            #[cfg(feature = "otlp")]
            {
                _layers.with_span_layer(
                    "reth".to_string(),
                    endpoint.clone(),
                    self.otlp_filter.clone(),
                    self.protocol,
                )?;
                Ok(OtlpInitStatus::Started(endpoint.clone()))
            }
            #[cfg(not(feature = "otlp"))]
            {
                Ok(OtlpInitStatus::NoFeature)
            }
        } else {
            Ok(OtlpInitStatus::Disabled)
        }
    }
}

/// Status of OTLP tracing initialization.
#[derive(Debug)]
pub enum OtlpInitStatus {
    /// OTLP tracing was successfully started with the given endpoint.
    Started(Url),
    /// OTLP tracing is disabled (no endpoint configured).
    Disabled,
    /// OTLP arguments provided but feature is not compiled.
    NoFeature,
}

// Parses an OTLP endpoint url.
fn parse_otlp_endpoint(arg: &str) -> eyre::Result<Url> {
    Url::parse(arg).wrap_err("Invalid URL for OTLP trace output")
}
