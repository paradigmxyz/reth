#![cfg(feature = "otlp")]

//! Provides a tracing layer for `OpenTelemetry` that exports spans to an OTLP endpoint.
//!
//! This module simplifies the integration of `OpenTelemetry` tracing with OTLP export in Rust
//! applications. It allows for easily capturing and exporting distributed traces to compatible
//! backends like Jaeger, Zipkin, or any other OpenTelemetry-compatible tracing system.

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

/// Creates a tracing [`OpenTelemetryLayer`] that exports spans to an OTLP endpoint.
///
/// This layer can be added to a [`tracing_subscriber::Registry`] to enable `OpenTelemetry` tracing
/// with OTLP export to an url.
pub fn span_layer<S>(otlp_config: OtlpConfig) -> eyre::Result<OpenTelemetryLayer<S, SdkTracer>>
where
    for<'span> S: Subscriber + LookupSpan<'span>,
{
    global::set_text_map_propagator(TraceContextPropagator::new());

    let resource = build_resource(otlp_config.service_name);

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

    let tracer = tracer_provider.tracer("reth");
    Ok(tracing_opentelemetry::layer().with_tracer(tracer))
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
    pub sample_ratio: Option<f64>,
}

impl OtlpConfig {
    /// Creates a new OTLP configuration.
    pub fn new(
        service_name: impl Into<String>,
        endpoint: Url,
        protocol: OtlpProtocol,
        sample_ratio: Option<f64>,
    ) -> Self {
        Self { service_name: service_name.into(), endpoint, protocol, sample_ratio }
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
    /// Validate and correct the URL to match protocol requirements.
    ///
    /// For HTTP: Ensures the path ends with `/v1/traces`, appending it if necessary.
    /// For gRPC: Ensures the path does NOT include `/v1/traces`.
    pub fn validate_endpoint(&self, url: &mut Url) -> eyre::Result<()> {
        match self {
            Self::Http => {
                if !url.path().ends_with(HTTP_TRACE_ENDPOINT) {
                    let path = url.path().trim_end_matches('/');
                    url.set_path(&format!("{}{}", path, HTTP_TRACE_ENDPOINT));
                }
            }
            Self::Grpc => {
                ensure!(
                    !url.path().ends_with(HTTP_TRACE_ENDPOINT),
                    "OTLP gRPC endpoint should not include {} path, got: {}",
                    HTTP_TRACE_ENDPOINT,
                    url
                );
            }
        }
        Ok(())
    }
}
