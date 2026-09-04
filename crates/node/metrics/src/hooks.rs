use metrics_process::Collector;
use parking_lot::Mutex;
use reth_tasks::TaskExecutor;
use std::{
    fmt,
    panic::{catch_unwind, AssertUnwindSafe},
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
    /// off, at most once per [`background_interval`](Self::with_background_interval) and never
    /// while a previous refresh is still running, but never waits for it.
    ///
    /// Collection that walks a backing store (e.g. every static file jar) scales with the dataset
    /// and can take seconds, which would stall a scrape for its entire duration. Such hooks only
    /// set gauges, and the interval already means most scrapes render values collected by an
    /// earlier one, so serving the previous values while the refresh runs costs at most one scrape
    /// worth of freshness.
    ///
    /// Background hooks are collected sequentially in registration order. A hook that panics is
    /// logged and does not prevent the hooks registered after it from being collected.
    pub fn with_background_hook(mut self, hook: impl Hook) -> Self {
        self.background_hooks.push(Box::new(hook));
        self
    }

    /// Sets the minimum interval between the starts of two background collections.
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
                state: Mutex::default(),
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

    /// Refreshes the background hooks unless a refresh is already in flight or the previous one
    /// started less than [`with_background_interval`](HooksBuilder::with_background_interval) ago.
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
    state: Mutex<BackgroundHooksState>,
}

impl BackgroundHooks {
    /// Marks a collection as started if none is in flight and the previous one started at least
    /// `interval` ago, so that a concurrent caller can not start a second one.
    ///
    /// The interval is claimed upfront, while the in-flight flag is released once
    /// [`collect`](Self::collect) returns or unwinds, so a panicking hook delays the next
    /// collection by one interval instead of blocking it forever.
    fn claim(&self) -> bool {
        if self.hooks.is_empty() {
            return false
        }

        let mut state = self.state.lock();
        if state.in_flight ||
            state.last_collected.is_some_and(|last| last.elapsed() < self.interval)
        {
            return false
        }
        state.last_collected = Some(Instant::now());
        state.in_flight = true;

        true
    }

    /// Collects a claimed refresh and releases the in-flight flag afterwards.
    ///
    /// Each hook is collected on its own so that one that panics is logged instead of skipping the
    /// hooks registered after it.
    fn collect(&self) {
        let _in_flight = InFlightGuard(&self.state);
        for (idx, hook) in self.hooks.iter().enumerate() {
            if catch_unwind(AssertUnwindSafe(hook)).is_err() {
                tracing::error!(hook = idx, "Background metrics hook panicked");
            }
        }
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

/// Refresh state of [`BackgroundHooks`], kept under one lock so that the interval and the
/// in-flight condition are checked and claimed atomically.
#[derive(Default)]
struct BackgroundHooksState {
    /// When the last collection was started.
    last_collected: Option<Instant>,
    /// Whether a collection is currently running.
    in_flight: bool,
}

/// Releases the in-flight flag when the collection finishes, also by unwinding.
struct InFlightGuard<'a>(&'a Mutex<BackgroundHooksState>);

impl Drop for InFlightGuard<'_> {
    fn drop(&mut self) {
        self.0.lock().in_flight = false;
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

#[cfg(test)]
mod tests {
    use super::*;
    use reth_tasks::Runtime;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Keeps kicking off refreshes until `done` holds, so that a refresh that was skipped because
    /// the previous one was still in flight is retried.
    fn refresh_until(hooks: &Hooks, runtime: &Runtime, done: impl Fn() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !done() {
            assert!(Instant::now() < deadline, "background collection did not make progress");
            hooks.refresh_background(runtime);
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn background_collections_do_not_overlap() {
        let runtime = Runtime::test();
        let active = Arc::new(AtomicUsize::new(0));
        let max_active = Arc::new(AtomicUsize::new(0));
        let collections = Arc::new(AtomicUsize::new(0));
        let hooks = Hooks::builder()
            .with_background_interval(Duration::from_millis(20))
            .with_background_hook({
                let active = active.clone();
                let max_active = max_active.clone();
                let collections = collections.clone();
                move || {
                    let now_active = active.fetch_add(1, Ordering::SeqCst) + 1;
                    max_active.fetch_max(now_active, Ordering::SeqCst);
                    // outlive the interval, so that refreshes are attempted while this runs
                    std::thread::sleep(Duration::from_millis(100));
                    collections.fetch_add(1, Ordering::SeqCst);
                    active.fetch_sub(1, Ordering::SeqCst);
                }
            })
            .build();

        let started = Instant::now();
        while started.elapsed() < Duration::from_millis(350) {
            hooks.refresh_background(&runtime);
            std::thread::sleep(Duration::from_millis(5));
        }
        while active.load(Ordering::SeqCst) > 0 {
            std::thread::sleep(Duration::from_millis(5));
        }

        assert_eq!(max_active.load(Ordering::SeqCst), 1, "collections overlapped");
        assert!(
            collections.load(Ordering::SeqCst) >= 2,
            "collection did not resume once the previous one finished"
        );
    }

    #[test]
    fn panicking_background_hook_does_not_block_collection() {
        let runtime = Runtime::test();
        let collections = Arc::new(AtomicUsize::new(0));
        let hooks = Hooks::builder()
            .with_background_interval(Duration::ZERO)
            .with_background_hook(|| panic!("hook panicked"))
            .with_background_hook({
                let collections = collections.clone();
                move || {
                    collections.fetch_add(1, Ordering::SeqCst);
                }
            })
            .build();

        // the hook registered after the panicking one is still collected
        refresh_until(&hooks, &runtime, || collections.load(Ordering::SeqCst) >= 1);
        // and the panic does not leave the collection marked as in flight
        refresh_until(&hooks, &runtime, || collections.load(Ordering::SeqCst) >= 2);
    }
}
