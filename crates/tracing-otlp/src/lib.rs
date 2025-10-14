#![cfg(feature = "otlp")]

//! Provides a tracing layer for `OpenTelemetry` that exports spans to an OTLP endpoint.
//!
//! This module simplifies the integration of `OpenTelemetry` tracing with OTLP export in Rust
//! applications. It allows for easily capturing and exporting distributed traces to compatible
//! backends like Jaeger, Zipkin, or any other OpenTelemetry-compatible tracing system.

use eyre::ensure;
use opentelemetry::{global, trace::TracerProvider, KeyValue, Value};
use opentelemetry_otlp::{SpanExporter, WithExportConfig};
use opentelemetry_sdk::{
    propagation::TraceContextPropagator,
    trace::{SdkTracer, SdkTracerProvider},
    Resource,
};
use opentelemetry_semantic_conventions::{attribute::SERVICE_VERSION, SCHEMA_URL};
use std::str::FromStr;
use tracing::Subscriber;
use tracing_opentelemetry::OpenTelemetryLayer;
use tracing_subscriber::registry::LookupSpan;
use url::Url;

/// Creates a tracing [`OpenTelemetryLayer`] that exports spans to an OTLP endpoint.
///
/// This layer can be added to a [`tracing_subscriber::Registry`] to enable `OpenTelemetry` tracing
/// with OTLP export to an url.
pub fn span_layer<S>(
    service_name: impl Into<Value>,
    endpoint: &Url,
    protocol: OtlpProtocol,
) -> eyre::Result<OpenTelemetryLayer<S, SdkTracer>>
where
    for<'span> S: Subscriber + LookupSpan<'span>,
{
    global::set_text_map_propagator(TraceContextPropagator::new());

    let resource = build_resource(service_name);

    let span_builder = SpanExporter::builder();

    let span_exporter = match protocol {
        OtlpProtocol::Http => span_builder.with_http().with_endpoint(endpoint.as_str()).build()?,
        OtlpProtocol::Grpc => span_builder.with_tonic().with_endpoint(endpoint.as_str()).build()?,
    };

    let tracer_provider = SdkTracerProvider::builder()
        .with_resource(resource)
        .with_batch_exporter(span_exporter)
        .build();

    global::set_tracer_provider(tracer_provider.clone());

    let tracer = tracer_provider.tracer("reth");
    Ok(tracing_opentelemetry::layer().with_tracer(tracer))
}

// Builds OTLP resource with service information.
fn build_resource(service_name: impl Into<Value>) -> Resource {
    Resource::builder()
        .with_service_name(service_name)
        .with_schema_url([KeyValue::new(SERVICE_VERSION, env!("CARGO_PKG_VERSION"))], SCHEMA_URL)
        .build()
}

/// OTLP transport protocol type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OtlpProtocol {
    /// HTTP/Protobuf transport, port 4318, requires `/v1/traces` path
    Http,
    /// gRPC transport, port 4317
    Grpc,
}

impl FromStr for OtlpProtocol {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "http" => Ok(Self::Http),
            "grpc" => Ok(Self::Grpc),
            _ => Err(format!("Invalid protocol '{}'. Valid values: http, grpc", s)),
        }
    }
}

impl OtlpProtocol {
    /// Validate that an url matches the requirements for each protocol
    pub fn validate_endpoint(&self, url: &Url) -> eyre::Result<()> {
        match self {
            Self::Http => {
                ensure!(
                    url.path().ends_with("/v1/traces"),
                    "OTLP trace endpoint must end with /v1/traces, got path: {}",
                    url.path()
                );
            }
            Self::Grpc => {
                ensure!(
                    !url.path().ends_with("/v1/traces"),
                    "OTLP gRPC endpoint should not include /v1/traces path, got: {}",
                    url
                );
            }
        }
        Ok(())
    }
}
