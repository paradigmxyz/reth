//! Provides a tracing layer for `OpenTelemetry` that exports spans to an OTLP endpoint.
//!
//! This module simplifies the integration of `OpenTelemetry` tracing with OTLP export in Rust
//! applications. It allows for easily capturing and exporting distributed traces to compatible
//! backends like Jaeger, Zipkin, or any other OpenTelemetry-compatible tracing system.

use opentelemetry::{trace::TracerProvider, KeyValue, Value};
use opentelemetry_otlp::{MetricExporter, SpanExporter};
use opentelemetry_sdk::{
    metrics::{PeriodicReader, SdkMeterProvider},
    trace::{SdkTracer, SdkTracerProvider},
    Resource,
};
use opentelemetry_semantic_conventions::{attribute::SERVICE_VERSION, SCHEMA_URL};
use std::time::Duration;
use tracing::Subscriber;
use tracing_opentelemetry::OpenTelemetryLayer;
use tracing_subscriber::registry::LookupSpan;

// Reads every 30 seconds the metrics
const DEFAULT_METRICS_READER_INTERVAL: Duration = Duration::from_secs(30);

/// Creates a tracing [`OpenTelemetryLayer`] that exports spans to an OTLP endpoint.
///
/// This layer can be added to a [`tracing_subscriber::Registry`] to enable `OpenTelemetry` tracing
/// with OTLP export.
pub fn layer<S>(service_name: impl Into<Value>) -> OpenTelemetryLayer<S, SdkTracer>
where
    for<'span> S: Subscriber + LookupSpan<'span>,
{
    let span_exporter = SpanExporter::builder().with_http().build().unwrap();

    let resource = build_resource(service_name);

    let provider = SdkTracerProvider::builder()
        .with_resource(resource)
        .with_batch_exporter(span_exporter)
        .build();

    let tracer = provider.tracer("reth-otlp");
    tracing_opentelemetry::layer().with_tracer(tracer)
}

/// Creates an `OpenTelemetry` metrics provider that exports metrics to a default OTLP endpoint.
pub fn metrics_provider(service_name: impl Into<Value>) -> SdkMeterProvider {
    let resource = build_resource(service_name);

    // No exporter endpoint provided, the metrics will be sent to the default OTLP endpoint through
    // http
    let metric_exporter = MetricExporter::builder().with_http().build().unwrap();

    let metrics_reader = PeriodicReader::builder(metric_exporter)
        .with_interval(DEFAULT_METRICS_READER_INTERVAL)
        .build();

    SdkMeterProvider::builder().with_resource(resource).with_reader(metrics_reader).build()
}

// Builds OTLP resource with service information.
fn build_resource(service_name: impl Into<Value>) -> Resource {
    Resource::builder()
        .with_service_name(service_name)
        .with_schema_url([KeyValue::new(SERVICE_VERSION, env!("CARGO_PKG_VERSION"))], SCHEMA_URL)
        .build()
}
