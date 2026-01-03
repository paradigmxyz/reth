//! Prometheus recorder

use eyre::WrapErr;
use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};
use metrics_util::layers::{PrefixLayer, Stack};
use std::sync::{atomic::AtomicBool, Mutex, OnceLock};

/// Installs the Prometheus recorder as the global recorder.
///
/// Note: This must be installed before any metrics are `described`.
///
/// Caution: This only configures the global recorder and does not spawn the exporter.
/// Callers must run [`PrometheusRecorder::spawn_upkeep`] manually.
///
/// If a builder is provided, it is registered as the one-time initialization path.
/// The default install path returns an error if the recorder cannot be installed.
pub fn install_prometheus_recorder(
    builder: Option<PrometheusBuilder>,
) -> eyre::Result<&'static PrometheusRecorder> {
    if let Some(builder) = builder {
        set_metrics_init_with_builder(builder)?;
    }

    let ran_init = run_metrics_init()?;

    if let Some(recorder) = PROMETHEUS_RECORDER_HANDLE.get() {
        return Ok(recorder);
    }

    // If a custom init ran but didn't install a global recorder, fail fast to
    // avoid silently falling back to defaults.
    if ran_init {
        return Err(eyre::eyre!(
            "Metrics init completed without installing the Prometheus recorder"
        ));
    }

    // No init ran and no recorder is installed, so fall back to the default install path.
    install_default_recorder()
}

/// The default Prometheus recorder handle. We use a global static to ensure that it is only
/// installed once.
static PROMETHEUS_RECORDER_HANDLE: OnceLock<PrometheusRecorder> = OnceLock::new();

type MetricsInit = Box<dyn FnOnce() -> eyre::Result<()> + Send + 'static>;

static METRICS_INIT: OnceLock<Mutex<Option<MetricsInit>>> = OnceLock::new();

/// Registers a custom metrics initializer.
///
/// Returns an error if an initializer has already been set or the recorder is installed.
pub fn set_metrics_init(init: MetricsInit) -> eyre::Result<()> {
    if PROMETHEUS_RECORDER_HANDLE.get().is_some() {
        return Err(eyre::eyre!("Prometheus recorder already installed"));
    }

    let slot = METRICS_INIT.get_or_init(|| Mutex::new(None));
    let mut guard = slot.lock().expect("metrics init lock poisoned");
    if guard.is_some() {
        return Err(eyre::eyre!("Metrics init already set"));
    }
    *guard = Some(init);
    Ok(())
}

/// Registers a custom metrics initializer that installs a Prometheus recorder with a builder.
pub fn set_metrics_init_with_builder(builder: PrometheusBuilder) -> eyre::Result<()> {
    set_metrics_init(Box::new(move || {
        install_prometheus_recorder_with_builder_inner(builder)?;
        Ok(())
    }))
}

/// Runs the one-shot metrics initializer if one was registered.
///
/// Returns `true` if the initializer ran, otherwise `false`.
fn run_metrics_init() -> eyre::Result<bool> {
    let Some(slot) = METRICS_INIT.get() else { return Ok(false) };
    let init = slot.lock().expect("metrics init lock poisoned").take();
    if let Some(init) = init {
        init()?;
        return Ok(true);
    }
    Ok(false)
}

/// Installs the default Prometheus recorder when none is configured.
///
/// If another call has already installed a recorder, returns the existing one.
fn install_default_recorder() -> eyre::Result<&'static PrometheusRecorder> {
    match PrometheusRecorder::install(None) {
        Ok(recorder) => {
            let _ = PROMETHEUS_RECORDER_HANDLE.set(recorder);
            Ok(PROMETHEUS_RECORDER_HANDLE.get().expect("recorder is set"))
        }
        Err(err) => {
            if let Some(recorder) = PROMETHEUS_RECORDER_HANDLE.get() {
                Ok(recorder)
            } else {
                Err(err)
            }
        }
    }
}

/// Installs a recorder using a `PrometheusBuilder` and sets the global handle.
fn install_prometheus_recorder_with_builder_inner(
    builder: PrometheusBuilder,
) -> eyre::Result<&'static PrometheusRecorder> {
    let recorder = PrometheusRecorder::install(Some(builder))?;
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
    pub fn install(builder: Option<PrometheusBuilder>) -> eyre::Result<Self> {
        match builder {
            Some(builder) => Self::install_with_builder(builder),
            None => Self::install_with_builder(PrometheusBuilder::new()),
        }
    }

    fn install_with_builder(builder: PrometheusBuilder) -> eyre::Result<Self> {
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
        let recorder = install_prometheus_recorder(None).unwrap();

        let process = metrics_process::Collector::default();
        process.describe();
        process.collect();

        let metrics = recorder.handle().render();
        assert!(metrics.contains("process_cpu_seconds_total"), "{metrics:?}");
    }
}
