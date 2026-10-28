//! With the `otlp` feature, provides tracing layers for `OpenTelemetry` that export spans, logs,
//! and metrics to an OTLP endpoint.
//!
//! This module simplifies the integration of `OpenTelemetry` with OTLP export in Rust
//! applications. It allows for easily capturing and exporting distributed traces, logs,
//! and metrics to compatible backends like Jaeger, Zipkin, or any other
//! OpenTelemetry-compatible system.

use clap::ValueEnum;
use eyre::ensure;
use url::Url;

#[cfg(feature = "otlp")]
mod otlp;
#[cfg(feature = "otlp")]
pub use otlp::*;

// Otlp http endpoint is expected to end with this path.
// See also <https://opentelemetry.io/docs/languages/sdk-configuration/otlp-exporter/#otel_exporter_otlp_traces_endpoint>.
const HTTP_TRACE_ENDPOINT: &str = "/v1/traces";
const HTTP_LOGS_ENDPOINT: &str = "/v1/logs";

/// OTLP transport protocol type
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum OtlpProtocol {
    /// HTTP/Protobuf transport, port 4318, requires `/v1/traces` path
    // `http/protobuf` is the OpenTelemetry-specified spelling of this value for
    // `OTEL_EXPORTER_OTLP_PROTOCOL`.
    #[value(alias = "http/protobuf")]
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
