use metrics_process::Collector;
use std::{fmt, sync::Arc, time::Duration};

/// The simple alias for function types that are `'static`, `Send`, and `Sync`.
pub trait Hook: Fn() + Send + Sync + 'static {}
impl<T: 'static + Fn() + Send + Sync> Hook for T {}

/// A builder-like type to create a new [`Hooks`] instance.
pub struct HooksBuilder {
    hooks: Vec<Box<dyn Hook<Output = ()>>>,
    periodic_hooks: Vec<Box<dyn Hook<Output = ()>>>,
    periodic_interval: Duration,
}

impl HooksBuilder {
    /// Default interval at which periodic hooks are collected.
    pub const DEFAULT_PERIODIC_INTERVAL: Duration = Duration::from_secs(5 * 60);

    /// Registers a [`Hook`] that runs on every scrape.
    ///
    /// Only suitable for cheap collection; anything that can take longer than a scrape timeout
    /// belongs in [`with_periodic_hook`](Self::with_periodic_hook).
    pub fn with_hook(self, hook: impl Hook) -> Self {
        self.with_boxed_hook(Box::new(hook))
    }

    /// Registers a [`Hook`] by calling the provided closure.
    pub fn install_hook<F, H>(self, f: F) -> Self
    where
        F: FnOnce() -> H,
        H: Hook,
    {
        self.with_hook(f())
    }

    /// Registers a [`Hook`].
    #[inline]
    pub fn with_boxed_hook(mut self, hook: Box<dyn Hook<Output = ()>>) -> Self {
        self.hooks.push(hook);
        self
    }

    /// Registers a [`Hook`] that a background task collects on
    /// [`periodic_interval`](Self::with_periodic_interval) instead of on the scrape path.
    ///
    /// Collection that walks a backing store (e.g. every static file jar) scales with the dataset
    /// and can take seconds, which would stall the scrape for its entire duration. Because such
    /// hooks only set gauges, collecting them out of band keeps the rendered values just as
    /// accurate while making scrape latency independent of the dataset size.
    ///
    /// Periodic hooks are collected sequentially in registration order.
    pub fn with_periodic_hook(mut self, hook: impl Hook) -> Self {
        self.periodic_hooks.push(Box::new(hook));
        self
    }

    /// Sets the interval at which the periodic hooks are collected.
    pub const fn with_periodic_interval(mut self, interval: Duration) -> Self {
        self.periodic_interval = interval;
        self
    }

    /// Builds the [`Hooks`] collection from the registered hooks.
    pub fn build(self) -> Hooks {
        Hooks {
            inner: Arc::new(self.hooks),
            periodic: Arc::new(self.periodic_hooks),
            periodic_interval: self.periodic_interval,
        }
    }
}

impl Default for HooksBuilder {
    fn default() -> Self {
        Self {
            hooks: vec![
                Box::new(|| Collector::default().collect()),
                Box::new(collect_memory_stats),
                Box::new(collect_io_stats),
            ],
            periodic_hooks: Vec::new(),
            periodic_interval: Self::DEFAULT_PERIODIC_INTERVAL,
        }
    }
}

impl std::fmt::Debug for HooksBuilder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HooksBuilder")
            .field("hooks", &format_args!("Vec<Box<dyn Hook>>, len: {}", self.hooks.len()))
            .field("periodic_hooks", &self.periodic_hooks.len())
            .finish()
    }
}

/// Helper type for managing hooks
#[derive(Clone)]
pub struct Hooks {
    inner: Arc<Vec<Box<dyn Hook<Output = ()>>>>,
    periodic: Arc<Vec<Box<dyn Hook<Output = ()>>>>,
    periodic_interval: Duration,
}

impl Hooks {
    /// Creates a new [`HooksBuilder`] instance.
    #[inline]
    pub fn builder() -> HooksBuilder {
        HooksBuilder::default()
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = &Box<dyn Hook<Output = ()>>> {
        self.inner.iter()
    }

    /// Runs all hooks registered with
    /// [`with_periodic_hook`](HooksBuilder::with_periodic_hook), in registration order.
    pub(crate) fn collect_periodic(&self) {
        self.periodic.iter().for_each(|hook| hook());
    }

    pub(crate) fn has_periodic(&self) -> bool {
        !self.periodic.is_empty()
    }

    pub(crate) const fn periodic_interval(&self) -> Duration {
        self.periodic_interval
    }
}

impl fmt::Debug for Hooks {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let hooks_len = self.inner.len();
        f.debug_struct("Hooks")
            .field("inner", &format_args!("Arc<Vec<Box<dyn Hook>>>, len: {hooks_len}"))
            .field("periodic", &self.periodic.len())
            .field("periodic_interval", &self.periodic_interval)
            .finish()
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
