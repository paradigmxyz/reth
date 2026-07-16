//! Opentelemetry tracing and logging configuration through CLI args.

use clap::{builder::Resettable, Parser};
use eyre::WrapErr;
use reth_tracing::{tracing_subscriber::EnvFilter, Layers};
use reth_tracing_otlp::OtlpProtocol;
use std::sync::OnceLock;
use url::Url;

static TRACE_DEFAULTS: OnceLock<DefaultTraceValues> = OnceLock::new();

/// Overridable defaults for OTLP trace configuration.
///
/// Downstream binaries that embed reth can call
/// `DefaultTraceValues::default().with_service_name("myapp").try_init()` before CLI parsing to
/// change the defaults that clap will use.
#[derive(Debug, Clone)]
pub struct DefaultTraceValues {
    otlp: Option<Url>,
    logs_otlp: Option<Url>,
    otlp_default_endpoint: String,
    logs_otlp_default_endpoint: String,
    protocol: OtlpProtocol,
    service_name: String,
    service_version: Option<String>,
    otlp_filter: String,
    logs_otlp_filter: String,
    sample_ratio: Option<f64>,
}

impl Default for DefaultTraceValues {
    fn default() -> Self {
        Self {
            otlp: None,
            logs_otlp: None,
            otlp_default_endpoint: "http://localhost:4318/v1/traces".to_string(),
            logs_otlp_default_endpoint: "http://localhost:4318/v1/logs".to_string(),
            protocol: OtlpProtocol::Http,
            service_name: "reth".to_string(),
            service_version: None,
            otlp_filter: "debug".to_string(),
            logs_otlp_filter: "info".to_string(),
            sample_ratio: None,
        }
    }
}

impl DefaultTraceValues {
    /// Initialize the global trace defaults with this configuration.
    pub fn try_init(self) -> Result<(), Self> {
        TRACE_DEFAULTS.set(self)
    }

    /// Get a reference to the global trace defaults.
    pub fn get_global() -> &'static Self {
        TRACE_DEFAULTS.get_or_init(Self::default)
    }

    /// Set the default OTLP tracing endpoint.
    pub fn with_otlp(mut self, otlp: Option<Url>) -> Self {
        self.otlp = otlp;
        self
    }

    /// Set the default OTLP logs endpoint.
    pub fn with_logs_otlp(mut self, logs_otlp: Option<Url>) -> Self {
        self.logs_otlp = logs_otlp;
        self
    }

    /// Set the endpoint used when `--tracing-otlp` is provided without a value.
    pub fn with_otlp_default_endpoint(mut self, endpoint: impl Into<String>) -> Self {
        self.otlp_default_endpoint = endpoint.into();
        self
    }

    /// Set the endpoint used when `--logs-otlp` is provided without a value.
    pub fn with_logs_otlp_default_endpoint(mut self, endpoint: impl Into<String>) -> Self {
        self.logs_otlp_default_endpoint = endpoint.into();
        self
    }

    /// Set the default OTLP transport protocol.
    pub const fn with_protocol(mut self, protocol: OtlpProtocol) -> Self {
        self.protocol = protocol;
        self
    }

    /// Set the default service name.
    pub fn with_service_name(mut self, name: impl Into<String>) -> Self {
        self.service_name = name.into();
        self
    }

    /// Set the default service version.
    pub fn with_service_version(mut self, version: impl Into<String>) -> Self {
        self.service_version = Some(version.into());
        self
    }

    /// Set the default OTLP tracing filter.
    pub fn with_otlp_filter(mut self, filter: impl Into<String>) -> Self {
        self.otlp_filter = filter.into();
        self
    }

    /// Set the default OTLP logs filter.
    pub fn with_logs_otlp_filter(mut self, filter: impl Into<String>) -> Self {
        self.logs_otlp_filter = filter.into();
        self
    }

    /// Set the default OTLP trace sampling ratio.
    pub const fn with_sample_ratio(mut self, sample_ratio: Option<f64>) -> Self {
        self.sample_ratio = sample_ratio;
        self
    }

    const fn protocol_as_str(&self) -> &'static str {
        match self.protocol {
            OtlpProtocol::Http => "http",
            OtlpProtocol::Grpc => "grpc",
        }
    }
}

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
        default_value = Resettable::from(DefaultTraceValues::get_global().otlp.as_ref().map(|url| url.as_str().into())),
        default_missing_value = DefaultTraceValues::get_global().otlp_default_endpoint.as_str(),
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
        default_value = Resettable::from(DefaultTraceValues::get_global().logs_otlp.as_ref().map(|url| url.as_str().into())),
        default_missing_value = DefaultTraceValues::get_global().logs_otlp_default_endpoint.as_str(),
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
        default_value = DefaultTraceValues::get_global().protocol_as_str(),
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
        default_value = DefaultTraceValues::get_global().otlp_filter.as_str(),
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
        default_value = DefaultTraceValues::get_global().logs_otlp_filter.as_str(),
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
        default_value = DefaultTraceValues::get_global().service_name.as_str(),
        hide = true,
        help_heading = "Tracing"
    )]
    pub service_name: String,

    /// Service version to use for OTLP tracing export.
    ///
    /// Overrides the default version reported in the `service.version` OTLP resource attribute.
    /// Falls back to the crate's `CARGO_PKG_VERSION` if not specified.
    #[arg(
        long = "tracing-otlp.service-version",
        env = "OTEL_SERVICE_VERSION",
        global = true,
        value_name = "VERSION",
        default_value = Resettable::from(DefaultTraceValues::get_global().service_version.as_ref().map(|version| version.as_str().into())),
        hide = true,
        help_heading = "Tracing"
    )]
    pub service_version: Option<String>,

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
        default_value = Resettable::from(DefaultTraceValues::get_global().sample_ratio.map(|ratio| ratio.to_string().into())),
        help_heading = "Tracing"
    )]
    pub sample_ratio: Option<f64>,
}

impl Default for TraceArgs {
    fn default() -> Self {
        let defaults = DefaultTraceValues::get_global();
        Self {
            otlp: defaults.otlp.clone(),
            logs_otlp: defaults.logs_otlp.clone(),
            protocol: defaults.protocol,
            otlp_filter: defaults.otlp_filter.parse().expect("valid filter"),
            logs_otlp_filter: defaults.logs_otlp_filter.parse().expect("valid filter"),
            sample_ratio: defaults.sample_ratio,
            service_name: defaults.service_name.clone(),
            service_version: defaults.service_version.clone(),
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
                    let mut config = reth_tracing_otlp::OtlpConfig::new(
                        self.service_name.clone(),
                        endpoint.clone(),
                        self.protocol,
                        self.sample_ratio,
                    )?;
                    if let Some(version) = &self.service_version {
                        config = config.with_service_version(version.clone());
                    }

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
                let mut config = reth_tracing_otlp::OtlpLogsConfig::new(
                    self.service_name.clone(),
                    endpoint.clone(),
                    self.protocol,
                )?;
                if let Some(version) = &self.service_version {
                    config = config.with_service_version(version.clone());
                }

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

#[cfg(test)]
mod tests {
    use super::{DefaultTraceValues, TraceArgs};
    use reth_tracing_otlp::OtlpProtocol;

    #[test]
    fn default_trace_values_can_override_filters() {
        let defaults =
            DefaultTraceValues::default().with_otlp_filter("trace").with_logs_otlp_filter("debug");

        assert_eq!(defaults.otlp_filter, "trace");
        assert_eq!(defaults.logs_otlp_filter, "debug");
    }

    #[test]
    fn default_trace_values_can_override_all_defaults() {
        let otlp = "http://localhost:4318/v1/traces".parse().unwrap();
        let logs_otlp = "http://localhost:4318/v1/logs".parse().unwrap();
        let defaults = DefaultTraceValues::default()
            .with_otlp(Some(otlp))
            .with_logs_otlp(Some(logs_otlp))
            .with_otlp_default_endpoint("http://collector:4318/v1/traces")
            .with_logs_otlp_default_endpoint("http://collector:4318/v1/logs")
            .with_protocol(OtlpProtocol::Grpc)
            .with_service_name("custom")
            .with_service_version("1.2.3")
            .with_otlp_filter("info")
            .with_logs_otlp_filter("debug")
            .with_sample_ratio(Some(0.5));

        let args = TraceArgs {
            otlp: defaults.otlp.clone(),
            logs_otlp: defaults.logs_otlp.clone(),
            protocol: defaults.protocol,
            otlp_filter: defaults.otlp_filter.parse().unwrap(),
            logs_otlp_filter: defaults.logs_otlp_filter.parse().unwrap(),
            service_name: defaults.service_name.clone(),
            service_version: defaults.service_version.clone(),
            sample_ratio: defaults.sample_ratio,
        };

        assert!(args.otlp.is_some());
        assert!(args.logs_otlp.is_some());
        assert_eq!(args.protocol, OtlpProtocol::Grpc);
        assert_eq!(args.service_name, "custom");
        assert_eq!(args.service_version.as_deref(), Some("1.2.3"));
        assert_eq!(args.sample_ratio, Some(0.5));
        assert_eq!(defaults.otlp_default_endpoint, "http://collector:4318/v1/traces");
        assert_eq!(defaults.logs_otlp_default_endpoint, "http://collector:4318/v1/logs");
    }
}
