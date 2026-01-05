//! Prometheus recorder

use eyre::WrapErr;
use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};
use metrics_util::layers::{PrefixLayer, Stack};
use std::sync::{atomic::AtomicBool, OnceLock};

/// Installs the Prometheus recorder as the global recorder.
///
/// Note: This must be installed before any metrics are `described`.
///
/// Caution: This only configures the global recorder and does not spawn the exporter.
/// Callers must run [`PrometheusRecorder::spawn_upkeep`] manually.
///
/// Use [`init_prometheus_recorder`] to install a custom recorder.
pub fn install_prometheus_recorder() -> &'static PrometheusRecorder {
    PROMETHEUS_RECORDER_HANDLE.get_or_init(|| {
        PrometheusRecorder::install().expect("Failed to install Prometheus recorder")
    })
}

/// Installs the provided recorder as the global recorder.
///
/// To customize the builder, first construct a recorder with
/// [`PrometheusRecorder::install_with_builder`], then pass it here.
///
/// # Panics
///
/// Panics if a recorder has already been installed.
pub fn init_prometheus_recorder(recorder: PrometheusRecorder) -> &'static PrometheusRecorder {
    PROMETHEUS_RECORDER_HANDLE.set(recorder).expect("Prometheus recorder already installed");
    PROMETHEUS_RECORDER_HANDLE.get().expect("Prometheus recorder is set")
}

/// The default Prometheus recorder handle. We use a global static to ensure that it is only
/// installed once.
static PROMETHEUS_RECORDER_HANDLE: OnceLock<PrometheusRecorder> = OnceLock::new();

/// Installs the Prometheus recorder with a custom builder.
///
/// Returns an error if a recorder has already been installed.
pub fn try_install_prometheus_recorder_with_builder(
    builder: PrometheusBuilder,
) -> eyre::Result<&'static PrometheusRecorder> {
    let recorder = PrometheusRecorder::install_with_builder(builder)?;
    PROMETHEUS_RECORDER_HANDLE
        .set(recorder)
        .map_err(|_| eyre::eyre!("Prometheus recorder already installed"))?;
    Ok(PROMETHEUS_RECORDER_HANDLE.get().expect("recorder is set"))
}

/// A handle to the Prometheus recorder.
///
/// This is intended to be used as the global recorder.
/// Callers must ensure that [`PrometheusRecorder::spawn_upkeep`] is called once.
#[derive(Debug)]
pub struct PrometheusRecorder {
    handle: PrometheusHandle,
    upkeep: AtomicBool,
}

impl PrometheusRecorder {
    const fn new(handle: PrometheusHandle) -> Self {
        Self { handle, upkeep: AtomicBool::new(false) }
    }

    /// Returns a reference to the [`PrometheusHandle`].
    pub const fn handle(&self) -> &PrometheusHandle {
        &self.handle
    }

    /// Spawns the upkeep task if there hasn't been one spawned already.
    ///
    /// ## Panics
    ///
    /// This method must be called from within an existing Tokio runtime or it will panic.
    ///
    /// See also [`PrometheusHandle::run_upkeep`]
    pub fn spawn_upkeep(&self) {
        if self
            .upkeep
            .compare_exchange(
                false,
                true,
                std::sync::atomic::Ordering::SeqCst,
                std::sync::atomic::Ordering::Acquire,
            )
            .is_err()
        {
            return;
        }

        let handle = self.handle.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                handle.run_upkeep();
            }
        });
    }

    /// Installs Prometheus as the metrics recorder.
    ///
    /// Caution: This only configures the global recorder and does not spawn the exporter.
    /// Callers must run [`Self::spawn_upkeep`] manually.
    pub fn install() -> eyre::Result<Self> {
        Self::install_with_builder(PrometheusBuilder::new())
    }

    /// Installs Prometheus as the metrics recorder with a custom builder.
    ///
    /// Caution: This only configures the global recorder and does not spawn the exporter.
    /// Callers must run [`Self::spawn_upkeep`] manually.
    pub fn install_with_builder(builder: PrometheusBuilder) -> eyre::Result<Self> {
        let recorder = builder.build_recorder();
        let handle = recorder.handle();

        // Build metrics stack
        Stack::new(recorder)
            .push(PrefixLayer::new("reth"))
            .install()
            .wrap_err("Couldn't set metrics recorder.")?;

        Ok(Self::new(handle))
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
        let recorder = install_prometheus_recorder();

        let process = metrics_process::Collector::default();
        process.describe();
        process.collect();

        let metrics = recorder.handle().render();
        assert!(metrics.contains("process_cpu_seconds_total"), "{metrics:?}");
    }
}
