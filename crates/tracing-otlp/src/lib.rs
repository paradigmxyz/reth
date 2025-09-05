//! Provides a tracing layer for `OpenTelemetry` that exports spans to an OTLP endpoint.
//!
//! This module simplifies the integration of `OpenTelemetry` tracing with OTLP export in Rust
//! applications. It allows for easily capturing and exporting distributed traces to compatible
//! backends like Jaeger, Zipkin, or any other OpenTelemetry-compatible tracing system.

use opentelemetry::{trace::TracerProvider, KeyValue, Value};
use opentelemetry_otlp::SpanExporter;
use opentelemetry_sdk::{
    trace::{SdkTracer, SdkTracerProvider},
    Resource,
};
use opentelemetry_semantic_conventions::{attribute::SERVICE_VERSION, SCHEMA_URL};
use tracing::Subscriber;
use tracing_opentelemetry::OpenTelemetryLayer;
use tracing_subscriber::registry::LookupSpan;

/// Creates a tracing [`OpenTelemetryLayer`] that exports spans to an OTLP endpoint.
///
/// This layer can be added to a [`tracing_subscriber::Registry`] to enable `OpenTelemetry` tracing
/// with OTLP export.
pub fn layer<S>(service_name: impl Into<Value>) -> OpenTelemetryLayer<S, SdkTracer>
where
    for<'span> S: Subscriber + LookupSpan<'span>,
{
    let exporter = SpanExporter::builder().with_http().build().unwrap();

    let resource = Resource::builder()
        .with_service_name(service_name)
        .with_schema_url([KeyValue::new(SERVICE_VERSION, env!("CARGO_PKG_VERSION"))], SCHEMA_URL)
        .build();

    let provider =
        SdkTracerProvider::builder().with_resource(resource).with_batch_exporter(exporter).build();

    let tracer = provider.tracer("reth-otlp");
    tracing_opentelemetry::layer().with_tracer(tracer)
}
