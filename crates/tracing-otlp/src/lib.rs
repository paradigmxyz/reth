#![cfg(feature = "otlp")]

//! Provides a tracing layer for `OpenTelemetry` that exports spans to an OTLP endpoint.
//!
//! This module simplifies the integration of `OpenTelemetry` tracing with OTLP export in Rust
//! applications. It allows for easily capturing and exporting distributed traces to compatible
//! backends like Jaeger, Zipkin, or any other OpenTelemetry-compatible tracing system.

use opentelemetry::{trace::TracerProvider, KeyValue, Value};
use opentelemetry_otlp::{MetricExporter, SpanExporter, WithExportConfig};
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

// Reads every 30 second-interval the metrics
const DEFAULT_METRICS_READER_INTERVAL: Duration = Duration::from_secs(30);

/// Creates a tracing [`OpenTelemetryLayer`] that exports spans to an OTLP endpoint.
///
/// This layer can be added to a [`tracing_subscriber::Registry`] to enable `OpenTelemetry` tracing
/// with OTLP export.
pub fn span_layer<S>(service_name: impl Into<Value>) -> OpenTelemetryLayer<S, SdkTracer>
where
    for<'span> S: Subscriber + LookupSpan<'span>,
{
    let resource = build_resource(service_name);

    let span_exporter = SpanExporter::builder().with_http().build().unwrap();

    let provider = SdkTracerProvider::builder()
        .with_resource(resource)
        .with_batch_exporter(span_exporter)
        .build();

    let tracer = provider.tracer("reth-otlp");
    tracing_opentelemetry::layer().with_tracer(tracer)
}

/// Creates an `OpenTelemetry` metrics provider that exports metrics to a default OTLP endpoint.
pub fn create_metrics_provider(
    service_name: impl Into<Value>,
    endpoint: &str,
) -> eyre::Result<SdkMeterProvider> {
    let resource = build_resource(service_name);

    // Metrics will be sent to the OTLP endpoint through http (and not grpc, and  with no tls)
    let metric_exporter = MetricExporter::builder().with_http().with_endpoint(endpoint).build()?;

    let metrics_reader = PeriodicReader::builder(metric_exporter)
        .with_interval(DEFAULT_METRICS_READER_INTERVAL)
        .build();

    Ok(SdkMeterProvider::builder().with_resource(resource).with_reader(metrics_reader).build())
}

// Builds OTLP resource with service information.
fn build_resource(service_name: impl Into<Value>) -> Resource {
    Resource::builder()
        .with_service_name(service_name)
        .with_schema_url([KeyValue::new(SERVICE_VERSION, env!("CARGO_PKG_VERSION"))], SCHEMA_URL)
        .build()
}
