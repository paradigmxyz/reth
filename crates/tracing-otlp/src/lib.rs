#![cfg(feature = "otlp")]

//! Provides a tracing layer for `OpenTelemetry` that exports spans to an OTLP endpoint.
//!
//! This module simplifies the integration of `OpenTelemetry` tracing with OTLP export in Rust
//! applications. It allows for easily capturing and exporting distributed traces to compatible
//! backends like Jaeger, Zipkin, or any other OpenTelemetry-compatible tracing system.

use eyre::{ensure, WrapErr};
use opentelemetry::{global, trace::TracerProvider, KeyValue, Value};
use opentelemetry_otlp::{SpanExporter, WithExportConfig};
use opentelemetry_sdk::{
    propagation::TraceContextPropagator,
    trace::{SdkTracer, SdkTracerProvider},
    Resource,
};
use opentelemetry_semantic_conventions::{attribute::SERVICE_VERSION, SCHEMA_URL};
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
    output: TraceOutput,
) -> eyre::Result<OpenTelemetryLayer<S, SdkTracer>>
where
    for<'span> S: Subscriber + LookupSpan<'span>,
{
    global::set_text_map_propagator(TraceContextPropagator::new());

    let resource = build_resource(service_name);

    let mut provider_builder = SdkTracerProvider::builder().with_resource(resource);

    provider_builder = if let TraceOutput::Otlp(url) = output {
        let span_exporter =
            SpanExporter::builder().with_http().with_endpoint(url.to_string()).build()?;
        provider_builder.with_batch_exporter(span_exporter)
    } else {
        let stdout_exporter = opentelemetry_stdout::SpanExporter::default();
        provider_builder.with_simple_exporter(stdout_exporter)
    };

    let tracer_provider = provider_builder.build();

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

/// Destination for exported trace spans.
#[derive(Debug, Clone)]
pub enum TraceOutput {
    /// Export traces as JSON to stdout.
    Stdout,
    /// Export traces to an OTLP collector at the specified URL.
    Otlp(Url),
}

impl TraceOutput {
    /// Parses the trace output destination from a string.
    ///
    /// Returns `TraceOutput::Stdout` for "stdout", or `TraceOutput::Otlp` for valid OTLP URLs.
    /// OTLP URLs must end with `/v1/traces` per the OTLP specification.
    pub fn parse(s: &str) -> eyre::Result<Self> {
        if s == "stdout" {
            return Ok(Self::Stdout);
        }

        let url = Url::parse(s).wrap_err("Invalid URL for trace output")?;

        // OTLP specification requires the `/v1/traces` path for trace endpoints
        ensure!(
            url.path().ends_with("/v1/traces"),
            "OTLP trace endpoint must end with /v1/traces, got path: {}",
            url.path()
        );

        Ok(Self::Otlp(url))
    }
}
