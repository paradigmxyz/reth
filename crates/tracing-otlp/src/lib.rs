#![cfg(feature = "otlp")]

//! Provides a tracing layer for `OpenTelemetry` that exports spans to an OTLP endpoint.
//!
//! This module simplifies the integration of `OpenTelemetry` tracing with OTLP export in Rust
//! applications. It allows for easily capturing and exporting distributed traces to compatible
//! backends like Jaeger, Zipkin, or any other OpenTelemetry-compatible tracing system.

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

/// Creates a tracing [`OpenTelemetryLayer`] that exports spans to an OTLP endpoint.
///
/// This layer can be added to a [`tracing_subscriber::Registry`] to enable `OpenTelemetry` tracing
/// with OTLP export to an url.
pub fn span_layer<S>(
    service_name: impl Into<Value>,
    exporter_endpoint: String,
) -> eyre::Result<OpenTelemetryLayer<S, SdkTracer>>
where
    for<'span> S: Subscriber + LookupSpan<'span>,
{
    global::set_text_map_propagator(TraceContextPropagator::new());
    let resource = build_resource(service_name);

    let url = format!("http://{exporter_endpoint}");

    let span_exporter = SpanExporter::builder().with_http().with_endpoint(url).build()?;

    let tracer_provider = SdkTracerProvider::builder()
        .with_resource(resource)
        .with_sampler(Sampler::AlwaysOn)
        .with_batch_exporter(span_exporter)
        .build();

    global::set_tracer_provider(tracer_provider.clone());

    let tracer = tracer_provider.tracer("reth-otlp");
    Ok(tracing_opentelemetry::layer().with_tracer(tracer))
}

// Builds OTLP resource with service information.
fn build_resource(service_name: impl Into<Value>) -> Resource {
    Resource::builder()
        .with_service_name(service_name)
        .with_schema_url([KeyValue::new(SERVICE_VERSION, env!("CARGO_PKG_VERSION"))], SCHEMA_URL)
        .build()
}
