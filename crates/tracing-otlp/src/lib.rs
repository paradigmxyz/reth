//! Provides a tracing layer for OpenTelemetry that exports spans to an OTLP endpoint.
//!
//! This module simplifies the integration of OpenTelemetry tracing with OTLP export in Rust
//! applications. It allows for easily capturing and exporting distributed traces to compatible
//! backends like Jaeger, Zipkin, or any other OpenTelemetry-compatible tracing system.

use opentelemetry::trace::TracerProvider;
use opentelemetry_otlp::SpanExporter;
use opentelemetry_sdk::trace::{SdkTracer, SdkTracerProvider};
use tracing::Subscriber;
use tracing_opentelemetry::OpenTelemetryLayer;
use tracing_subscriber::registry::LookupSpan;

/// Creates a tracing [OpenTelemetryLayer] that exports spans to an OTLP endpoint.
///
/// This layer can be added to a [tracing_subscriber::Registry] to enable OpenTelemetry tracing
/// with OTLP export.
pub fn layer<S>() -> OpenTelemetryLayer<S, SdkTracer>
where
    for<'span> S: Subscriber + LookupSpan<'span>,
{
    let exporter = SpanExporter::builder().with_http().build().unwrap();

    let provider = SdkTracerProvider::builder().with_batch_exporter(exporter).build();

    let tracer = provider.tracer("reth-otlp");
    tracing_opentelemetry::layer().with_tracer(tracer)
}
