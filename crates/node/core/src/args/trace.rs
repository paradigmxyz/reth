//! Opentelemetry tracing and logging configuration through CLI args.

use clap::Parser;
use eyre::WrapErr;
use reth_tracing::{tracing_subscriber::EnvFilter, Layers};
use reth_tracing_otlp::OtlpProtocol;
use url::Url;

/// CLI arguments for configuring `Opentelemetry` trace and logs export.
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

    /// Enable `Opentelemetry` logs export to an OTLP endpoint.
    ///
    /// If no value provided, defaults based on protocol:
    /// - HTTP: `http://localhost:4318/v1/logs`
    /// - gRPC: `http://localhost:4317`
    ///
    /// Example: --logs-otlp=http://collector:4318/v1/logs
    #[arg(
        long = "logs-otlp",
        env = "OTEL_EXPORTER_OTLP_LOGS_ENDPOINT",
        global = true,
        value_name = "URL",
        num_args = 0..=1,
        default_missing_value = "http://localhost:4318/v1/logs",
        require_equals = true,
        value_parser = parse_otlp_endpoint,
        help_heading = "Logging"
    )]
    pub logs_otlp: Option<Url>,

    /// OTLP transport protocol to use for exporting traces and logs.
    ///
    /// - `http`: expects endpoint path to end with `/v1/traces` or `/v1/logs`
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

    /// Set a filter directive for the OTLP logs exporter. This controls the verbosity
    /// of logs sent to the OTLP endpoint. It follows the same syntax as the
    /// `RUST_LOG` environment variable.
    ///
    /// Example: --logs-otlp.filter=info,reth=debug
    ///
    /// Defaults to INFO if not specified.
    #[arg(
        long = "logs-otlp.filter",
        global = true,
        value_name = "FILTER",
        default_value = "info",
        help_heading = "Logging"
    )]
    pub logs_otlp_filter: EnvFilter,

    /// Service name to use for OTLP tracing export.
    ///
    /// This name will be used to identify the service in distributed tracing systems
    /// like Jaeger or Zipkin. Useful for differentiating between multiple reth instances.
    ///
    /// Set via `OTEL_SERVICE_NAME` environment variable. Defaults to "reth" if not specified.
    #[arg(
        long = "tracing-otlp.service-name",
        env = "OTEL_SERVICE_NAME",
        global = true,
        value_name = "NAME",
        default_value = "reth",
        hide = true,
        help_heading = "Tracing"
    )]
    pub service_name: String,

    /// Trace sampling ratio to control the percentage of traces to export.
    ///
    /// Valid range: 0.0 to 1.0
    /// - 1.0, default: Sample all traces
    /// - 0.01: Sample 1% of traces
    /// - 0.0: Disable sampling
    ///
    /// Example: --tracing-otlp.sample-ratio=0.0.
    #[arg(
        long = "tracing-otlp.sample-ratio",
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
            logs_otlp: None,
            protocol: OtlpProtocol::Http,
            otlp_filter: EnvFilter::from_default_env(),
            logs_otlp_filter: EnvFilter::try_new("info").expect("valid filter"),
            sample_ratio: None,
            service_name: "reth".to_string(),
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
                {
                    let config = reth_tracing_otlp::OtlpConfig::new(
                        self.service_name.clone(),
                        endpoint.clone(),
                        self.protocol,
                        self.sample_ratio,
                    )?;

                    _layers.with_span_layer(config.clone(), self.otlp_filter.clone())?;

                    Ok(OtlpInitStatus::Started(config.endpoint().clone()))
                }
            }
            #[cfg(not(feature = "otlp"))]
            {
                Ok(OtlpInitStatus::NoFeature)
            }
        } else {
            Ok(OtlpInitStatus::Disabled)
        }
    }

    /// Initialize OTLP logs export with the given layers.
    ///
    /// This method handles OTLP logs initialization based on the configured options,
    /// including validation and protocol selection.
    ///
    /// Returns the initialization status to allow callers to log appropriate messages.
    pub async fn init_otlp_logs(&mut self, _layers: &mut Layers) -> eyre::Result<OtlpLogsStatus> {
        if let Some(endpoint) = self.logs_otlp.as_mut() {
            self.protocol.validate_logs_endpoint(endpoint)?;

            #[cfg(feature = "otlp-logs")]
            {
                let config = reth_tracing_otlp::OtlpLogsConfig::new(
                    self.service_name.clone(),
                    endpoint.clone(),
                    self.protocol,
                )?;

                _layers.with_log_layer(config.clone(), self.logs_otlp_filter.clone())?;

                Ok(OtlpLogsStatus::Started(config.endpoint().clone()))
            }
            #[cfg(not(feature = "otlp-logs"))]
            {
                Ok(OtlpLogsStatus::NoFeature)
            }
        } else {
            Ok(OtlpLogsStatus::Disabled)
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

/// Status of OTLP logs initialization.
#[derive(Debug)]
pub enum OtlpLogsStatus {
    /// OTLP logs export was successfully started with the given endpoint.
    Started(Url),
    /// OTLP logs export is disabled (no endpoint configured).
    Disabled,
    /// OTLP logs arguments provided but feature is not compiled.
    NoFeature,
}

// Parses an OTLP endpoint url.
fn parse_otlp_endpoint(arg: &str) -> eyre::Result<Url> {
    Url::parse(arg).wrap_err("Invalid URL for OTLP trace output")
}
