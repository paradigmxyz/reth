use metrics_process::Collector;
use std::{fmt, sync::Arc};

/// The simple alias for function types that are `'static`, `Send`, and `Sync`.
pub trait Hook: Fn() + Send + Sync + 'static {}
impl<T: 'static + Fn() + Send + Sync> Hook for T {}

/// A builder-like type to create a new [`Hooks`] instance.
pub struct HooksBuilder {
    hooks: Vec<Box<dyn Hook<Output = ()>>>,
}

impl HooksBuilder {
    /// Registers a [`Hook`].
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

    /// Builds the [`Hooks`] collection from the registered hooks.
    pub fn build(self) -> Hooks {
        Hooks { inner: Arc::new(self.hooks) }
    }
}

impl Default for HooksBuilder {
    fn default() -> Self {
        Self {
            hooks: vec![
                Box::new(|| Collector::default().collect()),
                Box::new(collect_memory_stats),
                Box::new(collect_io_stats),
                Box::new(collect_disk_stats),
            ],
        }
    }
}

impl std::fmt::Debug for HooksBuilder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HooksBuilder")
            .field("hooks", &format_args!("Vec<Box<dyn Hook>>, len: {}", self.hooks.len()))
            .finish()
    }
}

/// Helper type for managing hooks
#[derive(Clone)]
pub struct Hooks {
    inner: Arc<Vec<Box<dyn Hook<Output = ()>>>>,
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
}

impl fmt::Debug for Hooks {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let hooks_len = self.inner.len();
        f.debug_struct("Hooks")
            .field("inner", &format_args!("Arc<Vec<Box<dyn Hook>>>, len: {hooks_len}"))
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

#[cfg(target_os = "linux")]
fn collect_disk_stats() {
    use metrics::{counter, gauge};
    use tracing::error;

    let Ok(diskstats) = procfs::diskstats()
        .map_err(|error| error!(%error, "Failed to read disk statistics from /proc/diskstats"))
    else {
        return
    };

    for disk in diskstats {
        // Skip loop devices and ram disks typically
        if disk.name.starts_with("loop") || disk.name.starts_with("ram") {
            continue;
        }

        let device_name = &disk.name;

        // Read operations
        counter!("disk.reads", "device" => device_name.clone()).absolute(disk.reads);
        counter!("disk.merged", "device" => device_name.clone()).absolute(disk.merged);
        counter!("disk.sectors_read", "device" => device_name.clone()).absolute(disk.sectors_read);
        gauge!("disk.time_reading", "device" => device_name.clone()).set(disk.time_reading as f64);

        // Write operations
        counter!("disk.writes", "device" => device_name.clone()).absolute(disk.writes);
        counter!("disk.writes_merged", "device" => device_name.clone())
            .absolute(disk.writes_merged);
        counter!("disk.sectors_written", "device" => device_name.clone())
            .absolute(disk.sectors_written);
        gauge!("disk.time_writing", "device" => device_name.clone()).set(disk.time_writing as f64);

        // I/O operations in progress and time
        gauge!("disk.in_progress", "device" => device_name.clone()).set(disk.in_progress as f64);
        gauge!("disk.time_in_progress", "device" => device_name.clone())
            .set(disk.time_in_progress as f64);
        gauge!("disk.weighted_time_in_progress", "device" => device_name.clone())
            .set(disk.weighted_time_in_progress as f64);

        // Discard operations (if available, since kernel 4.18)
        if let Some(discards) = disk.discards {
            counter!("disk.discards", "device" => device_name.clone()).absolute(discards);
        }
        if let Some(discards_merged) = disk.discards_merged {
            counter!("disk.discards_merged", "device" => device_name.clone())
                .absolute(discards_merged);
        }
        if let Some(sectors_discarded) = disk.sectors_discarded {
            counter!("disk.sectors_discarded", "device" => device_name.clone())
                .absolute(sectors_discarded);
        }
        if let Some(time_discarding) = disk.time_discarding {
            gauge!("disk.time_discarding", "device" => device_name.clone())
                .set(time_discarding as f64);
        }

        // Flush operations (if available, since kernel 5.5)
        if let Some(flushes) = disk.flushes {
            counter!("disk.flushes", "device" => device_name.clone()).absolute(flushes);
        }
        if let Some(time_flushing) = disk.time_flushing {
            gauge!("disk.time_flushing", "device" => device_name.clone()).set(time_flushing as f64);
        }
    }
}

#[cfg(not(target_os = "linux"))]
const fn collect_disk_stats() {}
