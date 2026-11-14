//! Prometheus recorder tailored to reth's metrics endpoint.
//!
//! Records through the [`metrics`] facade and renders the Prometheus text exposition format with
//! the same output as `metrics-exporter-prometheus` 0.18.3 configured the way reth uses it. The
//! exporter's HTTP listener, push gateway, protobuf and native histogram support, idle metric
//! expiry and unit suffixes are not implemented: reth serves and pushes metrics itself.
//!
//! The aggregation, formatting and lock-free sample storage are ported from
//! `metrics-exporter-prometheus` 0.18.3 and `metrics-util` 0.20.4
//! (<https://github.com/metrics-rs/metrics>), MIT licensed, Copyright (c) 2021 Metrics
//! Contributors.

mod bucket;
mod builder;
mod distribution;
mod formatting;
mod recorder;
mod registry;
#[cfg(test)]
mod tests;

pub use builder::{BuildError, Matcher, PrometheusBuilder};
pub use recorder::{PrometheusHandle, PrometheusRecorder};
