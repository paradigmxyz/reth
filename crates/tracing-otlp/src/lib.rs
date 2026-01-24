#![cfg(feature = "otlp")]

//! Provides tracing layers for `OpenTelemetry` that export spans, logs, and metrics to an OTLP
//! endpoint.
//!
//! This module simplifies the integration of `OpenTelemetry` with OTLP export in Rust
//! applications. It allows for easily capturing and exporting distributed traces, logs,
//! and metrics to compatible backends like Jaeger, Zipkin, or any other
//! OpenTelemetry-compatible system.

use clap::ValueEnum;
use eyre::ensure;
use opentelemetry::{global, trace::TracerProvider, KeyValue, Value};
use opentelemetry_otlp::{SpanExporter, WithExportConfig};
use opentelemetry_sdk::{
    propagation::TraceContextPropagator,
    trace::{Sampler, SdkTracer, SdkTracerProvider},
    Resource,
};
use opentelemetry_semantic_conventions::{attribute::SERVICE_VERSION, SCHEMA_URL};
use tracing::Subscriber;
use tracing_opentelemetry::OpenTelemetryLayer;
use tracing_subscriber::registry::LookupSpan;
use url::Url;

// Otlp http endpoint is expected to end with this path.
// See also <https://opentelemetry.io/docs/languages/sdk-configuration/otlp-exporter/#otel_exporter_otlp_traces_endpoint>.
const HTTP_TRACE_ENDPOINT: &str = "/v1/traces";
const HTTP_LOGS_ENDPOINT: &str = "/v1/logs";

/// Creates a tracing [`OpenTelemetryLayer`] that exports spans to an OTLP endpoint.
///
/// This layer can be added to a [`tracing_subscriber::Registry`] to enable `OpenTelemetry` tracing
/// with OTLP export to an url.
pub fn span_layer<S>(otlp_config: OtlpConfig) -> eyre::Result<OpenTelemetryLayer<S, SdkTracer>>
where
    for<'span> S: Subscriber + LookupSpan<'span>,
{
    global::set_text_map_propagator(TraceContextPropagator::new());

    let resource = build_resource(otlp_config.service_name.clone());

    let span_builder = SpanExporter::builder();

    let span_exporter = match otlp_config.protocol {
        OtlpProtocol::Http => {
            span_builder.with_http().with_endpoint(otlp_config.endpoint.as_str()).build()?
        }
        OtlpProtocol::Grpc => {
            span_builder.with_tonic().with_endpoint(otlp_config.endpoint.as_str()).build()?
        }
    };

    let sampler = build_sampler(otlp_config.sample_ratio)?;

    let tracer_provider = SdkTracerProvider::builder()
        .with_resource(resource)
        .with_sampler(sampler)
        .with_batch_exporter(span_exporter)
        .build();

    global::set_tracer_provider(tracer_provider.clone());

    let tracer = tracer_provider.tracer(otlp_config.service_name);
    Ok(tracing_opentelemetry::layer().with_tracer(tracer))
}

/// Creates a tracing layer that exports logs to an OTLP endpoint.
///
/// This layer bridges logs emitted via the `tracing` crate to `OpenTelemetry` logs.
#[cfg(feature = "otlp-logs")]
pub fn log_layer(
    otlp_config: OtlpLogsConfig,
) -> eyre::Result<
    opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge<
        opentelemetry_sdk::logs::SdkLoggerProvider,
        opentelemetry_sdk::logs::SdkLogger,
    >,
> {
    use opentelemetry_otlp::LogExporter;
    use opentelemetry_sdk::logs::SdkLoggerProvider;

    let resource = build_resource(otlp_config.service_name.clone());

    let log_builder = LogExporter::builder();

    let log_exporter = match otlp_config.protocol {
        OtlpProtocol::Http => {
            log_builder.with_http().with_endpoint(otlp_config.endpoint.as_str()).build()?
        }
        OtlpProtocol::Grpc => {
            log_builder.with_tonic().with_endpoint(otlp_config.endpoint.as_str()).build()?
        }
    };

    let logger_provider = SdkLoggerProvider::builder()
        .with_resource(resource)
        .with_batch_exporter(log_exporter)
        .build();

    Ok(opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge::new(&logger_provider))
}

/// Configuration for OTLP trace export.
#[derive(Debug, Clone)]
pub struct OtlpConfig {
    /// Service name for trace identification
    service_name: String,
    /// Otlp endpoint URL
    endpoint: Url,
    /// Transport protocol, HTTP or gRPC
    protocol: OtlpProtocol,
    /// Optional sampling ratio, from 0.0 to 1.0
    sample_ratio: Option<f64>,
}

impl OtlpConfig {
    /// Creates a new OTLP configuration.
    pub fn new(
        service_name: impl Into<String>,
        endpoint: Url,
        protocol: OtlpProtocol,
        sample_ratio: Option<f64>,
    ) -> eyre::Result<Self> {
        if let Some(ratio) = sample_ratio {
            ensure!(
                (0.0..=1.0).contains(&ratio),
                "Sample ratio must be between 0.0 and 1.0, got: {}",
                ratio
            );
        }

        Ok(Self { service_name: service_name.into(), endpoint, protocol, sample_ratio })
    }

    /// Returns the service name.
    pub fn service_name(&self) -> &str {
        &self.service_name
    }

    /// Returns the OTLP endpoint URL.
    pub const fn endpoint(&self) -> &Url {
        &self.endpoint
    }

    /// Returns the transport protocol.
    pub const fn protocol(&self) -> OtlpProtocol {
        self.protocol
    }

    /// Returns the sampling ratio.
    pub const fn sample_ratio(&self) -> Option<f64> {
        self.sample_ratio
    }
}

/// Configuration for OTLP logs export.
#[derive(Debug, Clone)]
pub struct OtlpLogsConfig {
    /// Service name for log identification
    service_name: String,
    /// Otlp endpoint URL
    endpoint: Url,
    /// Transport protocol, HTTP or gRPC
    protocol: OtlpProtocol,
}

impl OtlpLogsConfig {
    /// Creates a new OTLP logs configuration.
    pub fn new(
        service_name: impl Into<String>,
        endpoint: Url,
        protocol: OtlpProtocol,
    ) -> eyre::Result<Self> {
        Ok(Self { service_name: service_name.into(), endpoint, protocol })
    }

    /// Returns the service name.
    pub fn service_name(&self) -> &str {
        &self.service_name
    }

    /// Returns the OTLP endpoint URL.
    pub const fn endpoint(&self) -> &Url {
        &self.endpoint
    }

    /// Returns the transport protocol.
    pub const fn protocol(&self) -> OtlpProtocol {
        self.protocol
    }
}

// Builds OTLP resource with service information.
fn build_resource(service_name: impl Into<Value>) -> Resource {
    Resource::builder()
        .with_service_name(service_name)
        .with_schema_url([KeyValue::new(SERVICE_VERSION, env!("CARGO_PKG_VERSION"))], SCHEMA_URL)
        .build()
}

/// Builds the appropriate sampler based on the sample ratio.
fn build_sampler(sample_ratio: Option<f64>) -> eyre::Result<Sampler> {
    match sample_ratio {
        // Default behavior: sample all traces
        None | Some(1.0) => Ok(Sampler::ParentBased(Box::new(Sampler::AlwaysOn))),
        // Don't sample anything
        Some(0.0) => Ok(Sampler::ParentBased(Box::new(Sampler::AlwaysOff))),
        // Sample based on trace ID ratio
        Some(ratio) => Ok(Sampler::ParentBased(Box::new(Sampler::TraceIdRatioBased(ratio)))),
    }
}

/// OTLP transport protocol type
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum OtlpProtocol {
    /// HTTP/Protobuf transport, port 4318, requires `/v1/traces` path
    Http,
    /// gRPC transport, port 4317
    Grpc,
}

impl OtlpProtocol {
    /// Validate and correct the URL to match protocol requirements for traces.
    ///
    /// For HTTP: Ensures the path ends with `/v1/traces`, appending it if necessary.
    /// For gRPC: Ensures the path does NOT include `/v1/traces`.
    pub fn validate_endpoint(&self, url: &mut Url) -> eyre::Result<()> {
        self.validate_endpoint_with_path(url, HTTP_TRACE_ENDPOINT)
    }

    /// Validate and correct the URL to match protocol requirements for logs.
    ///
    /// For HTTP: Ensures the path ends with `/v1/logs`, appending it if necessary.
    /// For gRPC: Ensures the path does NOT include `/v1/logs`.
    pub fn validate_logs_endpoint(&self, url: &mut Url) -> eyre::Result<()> {
        self.validate_endpoint_with_path(url, HTTP_LOGS_ENDPOINT)
    }

    fn validate_endpoint_with_path(&self, url: &mut Url, http_path: &str) -> eyre::Result<()> {
        match self {
            Self::Http => {
                if !url.path().ends_with(http_path) {
                    let path = url.path().trim_end_matches('/');
                    url.set_path(&format!("{}{}", path, http_path));
                }
            }
            Self::Grpc => {
                ensure!(
                    !url.path().ends_with(http_path),
                    "OTLP gRPC endpoint should not include {} path, got: {}",
                    http_path,
                    url
                );
            }
        }
        Ok(())
    }
}
