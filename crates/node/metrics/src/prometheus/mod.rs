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

use std::{ops::Add, time::Duration};

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

/// Monotonic clock for histogram sample timestamps.
#[derive(Clone, Copy, Debug)]
struct Clock {
    start: std::time::Instant,
}

impl Clock {
    fn new() -> Self {
        Self { start: std::time::Instant::now() }
    }

    fn now(&self) -> Timestamp {
        Timestamp(saturating_nanos(self.start.elapsed()))
    }
}

/// Nanoseconds since the recorder's [`Clock`] started.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Timestamp(u64);

impl Timestamp {
    #[cfg(test)]
    const fn from_nanos(nanos: u64) -> Self {
        Self(nanos)
    }

    /// Returns `None` if `duration` reaches back before the clock started.
    fn checked_sub(self, duration: Duration) -> Option<Self> {
        self.0.checked_sub(saturating_nanos(duration)).map(Self)
    }
}

impl Add<Duration> for Timestamp {
    type Output = Self;

    fn add(self, duration: Duration) -> Self {
        Self(self.0.saturating_add(saturating_nanos(duration)))
    }
}

fn saturating_nanos(duration: Duration) -> u64 {
    u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX)
}
