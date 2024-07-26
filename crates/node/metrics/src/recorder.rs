//! Prometheus recorder

use eyre::WrapErr;
use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};
use metrics_util::layers::{PrefixLayer, Stack};
use std::sync::LazyLock;

/// Installs the Prometheus recorder as the global recorder.
pub fn install_prometheus_recorder() -> &'static PrometheusHandle {
    &PROMETHEUS_RECORDER_HANDLE
}

/// The default Prometheus recorder handle. We use a global static to ensure that it is only
/// installed once.
static PROMETHEUS_RECORDER_HANDLE: LazyLock<PrometheusHandle> =
    LazyLock::new(|| PrometheusRecorder::install().unwrap());

/// Prometheus recorder installer
#[derive(Debug)]
pub struct PrometheusRecorder;

impl PrometheusRecorder {
    /// Installs Prometheus as the metrics recorder.
    pub fn install() -> eyre::Result<PrometheusHandle> {
        let recorder = PrometheusBuilder::new().build_recorder();
        let handle = recorder.handle();

        // Build metrics stack
        Stack::new(recorder)
            .push(PrefixLayer::new("reth"))
            .install()
            .wrap_err("Couldn't set metrics recorder.")?;

        Ok(handle)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // Dependencies using different version of the `metrics` crate (to be exact, 0.21 vs 0.22)
    // may not be able to communicate with each other through the global recorder.
    //
    // This test ensures that `metrics-process` dependency plays well with the current
    // `metrics-exporter-prometheus` dependency version.
    #[test]
    fn process_metrics() {
        // initialize the lazy handle
        let _ = &*PROMETHEUS_RECORDER_HANDLE;

        let process = metrics_process::Collector::default();
        process.describe();
        process.collect();

        let metrics = PROMETHEUS_RECORDER_HANDLE.render();
        assert!(metrics.contains("process_cpu_seconds_total"), "{metrics:?}");
    }
}
