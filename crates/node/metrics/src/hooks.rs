use metrics_process::Collector;
use parking_lot::Mutex;
use reth_tasks::TaskExecutor;
use std::{
    fmt,
    sync::Arc,
    time::{Duration, Instant},
};

/// The simple alias for function types that are `'static`, `Send`, and `Sync`.
pub trait Hook: Fn() + Send + Sync + 'static {}
impl<T: 'static + Fn() + Send + Sync> Hook for T {}

/// A builder-like type to create a new [`Hooks`] instance.
pub struct HooksBuilder {
    hooks: Vec<Box<dyn Hook<Output = ()>>>,
    background_hooks: Vec<Box<dyn Hook<Output = ()>>>,
    background_interval: Duration,
}

impl HooksBuilder {
    /// Default interval at which background hooks are refreshed.
    pub const DEFAULT_BACKGROUND_INTERVAL: Duration = Duration::from_secs(5 * 60);

    /// Registers a [`Hook`] that runs while metrics are collected.
    ///
    /// Only suitable for cheap collection; anything that can take longer than a scrape timeout
    /// belongs in [`with_background_hook`](Self::with_background_hook).
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

    /// Registers a [`Hook`] whose collection is refreshed out of band: metrics collection kicks it
    /// off, at most once per
    /// [`background_interval`](Self::with_background_interval), but never waits for it.
    ///
    /// Collection that walks a backing store (e.g. every static file jar) scales with the dataset
    /// and can take seconds, which would stall a scrape for its entire duration. Such hooks only
    /// set gauges, and the interval already means most scrapes render values collected by an
    /// earlier one, so serving the previous values while the refresh runs costs at most one scrape
    /// worth of freshness.
    ///
    /// Background hooks are collected sequentially in registration order.
    pub fn with_background_hook(mut self, hook: impl Hook) -> Self {
        self.background_hooks.push(Box::new(hook));
        self
    }

    /// Sets the minimum interval between two background collections.
    pub const fn with_background_interval(mut self, interval: Duration) -> Self {
        self.background_interval = interval;
        self
    }

    /// Builds the [`Hooks`] collection from the registered hooks.
    pub fn build(self) -> Hooks {
        Hooks {
            inner: Arc::new(self.hooks),
            background: Arc::new(BackgroundHooks {
                hooks: self.background_hooks,
                interval: self.background_interval,
                last_collected: Mutex::new(None),
            }),
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
            background_hooks: Vec::new(),
            background_interval: Self::DEFAULT_BACKGROUND_INTERVAL,
        }
    }
}

impl std::fmt::Debug for HooksBuilder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HooksBuilder")
            .field("hooks", &format_args!("Vec<Box<dyn Hook>>, len: {}", self.hooks.len()))
            .field("background_hooks", &self.background_hooks.len())
            .field("background_interval", &self.background_interval)
            .finish()
    }
}

/// Helper type for managing hooks
#[derive(Clone)]
pub struct Hooks {
    inner: Arc<Vec<Box<dyn Hook<Output = ()>>>>,
    background: Arc<BackgroundHooks>,
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

    /// Refreshes the background hooks unless they were collected less than
    /// [`with_background_interval`](HooksBuilder::with_background_interval) ago, or a refresh is
    /// already in flight.
    ///
    /// Returns without waiting for the collection to finish: the caller renders the values of the
    /// previous refresh.
    pub(crate) fn refresh_background(&self, executor: &TaskExecutor) {
        if !self.background.claim() {
            return
        }

        let background = self.background.clone();
        executor.spawn_blocking(move || background.collect());
    }
}

impl fmt::Debug for Hooks {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let hooks_len = self.inner.len();
        f.debug_struct("Hooks")
            .field("inner", &format_args!("Arc<Vec<Box<dyn Hook>>>, len: {hooks_len}"))
            .field("background", &self.background)
            .finish()
    }
}

/// The [`Hook`]s that are collected out of band, see
/// [`with_background_hook`](HooksBuilder::with_background_hook).
struct BackgroundHooks {
    hooks: Vec<Box<dyn Hook<Output = ()>>>,
    interval: Duration,
    last_collected: Mutex<Option<Instant>>,
}

impl BackgroundHooks {
    /// Marks a collection as started if one is due, which also claims the interval: a concurrent
    /// caller can not start a second collection until the interval has elapsed again.
    ///
    /// The interval is claimed upfront rather than on completion so that a hook that panics only
    /// delays the next collection instead of blocking it forever.
    fn claim(&self) -> bool {
        if self.hooks.is_empty() {
            return false
        }

        let mut last_collected = self.last_collected.lock();
        if last_collected.is_some_and(|last| last.elapsed() < self.interval) {
            return false
        }
        *last_collected = Some(Instant::now());

        true
    }

    fn collect(&self) {
        self.hooks.iter().for_each(|hook| hook());
    }
}

impl fmt::Debug for BackgroundHooks {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BackgroundHooks")
            .field("hooks", &format_args!("Vec<Box<dyn Hook>>, len: {}", self.hooks.len()))
            .field("interval", &self.interval)
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
