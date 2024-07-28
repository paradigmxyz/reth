use metrics_process::Collector;
use reth_db_api::database_metrics::DatabaseMetrics;
use reth_provider::providers::StaticFileProvider;
use std::{fmt, sync::Arc};
pub(crate) trait Hook: Fn() + Send + Sync {}
impl<T: Fn() + Send + Sync> Hook for T {}

impl fmt::Debug for Hooks {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let hooks_len = self.inner.len();
        f.debug_struct("Hooks")
            .field("inner", &format!("Arc<Vec<Box<dyn Hook>>>, len: {}", hooks_len))
            .finish()
    }
}

/// Helper type for managing hooks
#[derive(Clone)]
pub struct Hooks {
    inner: Arc<Vec<Box<dyn Hook<Output = ()>>>>,
}

impl Hooks {
    /// Create a new set of hooks
    pub fn new<Metrics: DatabaseMetrics + 'static + Send + Sync>(
        db: Metrics,
        static_file_provider: StaticFileProvider,
    ) -> Self {
        let hooks: Vec<Box<dyn Hook<Output = ()>>> = vec![
            Box::new(move || db.report_metrics()),
            Box::new(move || {
                let _ = static_file_provider.report_metrics().map_err(
                    |error| tracing::error!(%error, "Failed to report static file provider metrics"),
                );
            }),
            Box::new(move || Collector::default().collect()),
            Box::new(collect_memory_stats),
            Box::new(collect_io_stats),
        ];
        Self { inner: Arc::new(hooks) }
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = &Box<dyn Hook<Output = ()>>> {
        self.inner.iter()
    }
}

#[cfg(all(feature = "jemalloc", unix))]
fn collect_memory_stats() {
    use metrics::gauge;
    use tikv_jemalloc_ctl::{epoch, stats};
    use tracing::error;

    if epoch::advance().map_err(|error| error!(%error, "Failed to advance jemalloc epoch")).is_err()
    {
        return
    }

    if let Ok(value) = stats::active::read()
        .map_err(|error| error!(%error, "Failed to read jemalloc.stats.active"))
    {
        gauge!("jemalloc.active").set(value as f64);
    }

    if let Ok(value) = stats::allocated::read()
        .map_err(|error| error!(%error, "Failed to read jemalloc.stats.allocated"))
    {
        gauge!("jemalloc.allocated").set(value as f64);
    }

    if let Ok(value) = stats::mapped::read()
        .map_err(|error| error!(%error, "Failed to read jemalloc.stats.mapped"))
    {
        gauge!("jemalloc.mapped").set(value as f64);
    }

    if let Ok(value) = stats::metadata::read()
        .map_err(|error| error!(%error, "Failed to read jemalloc.stats.metadata"))
    {
        gauge!("jemalloc.metadata").set(value as f64);
    }

    if let Ok(value) = stats::resident::read()
        .map_err(|error| error!(%error, "Failed to read jemalloc.stats.resident"))
    {
        gauge!("jemalloc.resident").set(value as f64);
    }

    if let Ok(value) = stats::retained::read()
        .map_err(|error| error!(%error, "Failed to read jemalloc.stats.retained"))
    {
        gauge!("jemalloc.retained").set(value as f64);
    }
}

#[cfg(not(all(feature = "jemalloc", unix)))]
const fn collect_memory_stats() {}

#[cfg(target_os = "linux")]
fn collect_io_stats() {
    use metrics::counter;
    use tracing::error;

    let Ok(process) = procfs::process::Process::myself()
        .map_err(|error| error!(%error, "Failed to get currently running process"))
    else {
        return
    };

    let Ok(io) = process.io().map_err(
        |error| error!(%error, "Failed to get IO stats for the currently running process"),
    ) else {
        return
    };

    counter!("io.rchar").absolute(io.rchar);
    counter!("io.wchar").absolute(io.wchar);
    counter!("io.syscr").absolute(io.syscr);
    counter!("io.syscw").absolute(io.syscw);
    counter!("io.read_bytes").absolute(io.read_bytes);
    counter!("io.write_bytes").absolute(io.write_bytes);
    counter!("io.cancelled_write_bytes").absolute(io.cancelled_write_bytes);
}

#[cfg(not(target_os = "linux"))]
const fn collect_io_stats() {}
