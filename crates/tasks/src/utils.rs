//! Task utility functions.

use std::sync::atomic::{AtomicUsize, Ordering};

pub use thread_priority::{self, *};

/// Increases the current thread's priority.
///
/// Tries [`ThreadPriority::Max`] first. If that fails (e.g. missing `CAP_SYS_NICE`),
/// falls back to a moderate bump via [`ThreadPriority::Crossplatform`] (~5 nice points
/// on unix). Failures are logged at `debug` level.
pub fn increase_thread_priority() {
    let thread_name = std::thread::current().name().unwrap_or("unnamed").to_string();
    if let Err(err) = ThreadPriority::Max.set_for_current() {
        tracing::debug!(%thread_name, ?err, "failed to set max thread priority, trying moderate bump; grant CAP_SYS_NICE to the process to enable this");
        // Crossplatform value 62/99 ≈ nice -5 on unix.
        let fallback = ThreadPriority::Crossplatform(
            ThreadPriorityValue::try_from(62u8).expect("62 is within the valid 0..100 range"),
        );
        if let Err(err) = fallback.set_for_current() {
            tracing::debug!(%thread_name, ?err, "failed to set moderate thread priority");
        }
    }
}

/// Pins the current thread to a dedicated CPU core.
///
/// Each call assigns a different core in round-robin order from the set of available (online) CPUs.
/// This reduces context-switch overhead and improves cache locality for latency-sensitive threads
/// like the engine and sparse-trie tasks.
///
/// Failures are logged at `debug` level and are not fatal — the thread will simply run unpinned.
///
/// No-op on non-Linux platforms.
#[allow(clippy::missing_const_for_fn)]
pub fn pin_current_thread_to_core() {
    #[cfg(target_os = "linux")]
    _pin_current_thread_to_core();
}

/// Counter used to round-robin across available cores.
static CORE_INDEX: AtomicUsize = AtomicUsize::new(0);

#[cfg(target_os = "linux")]
fn _pin_current_thread_to_core() {
    use std::mem;

    let thread_name = std::thread::current().name().unwrap_or("unnamed").to_string();

    // Get the current process affinity mask (respects cgroups / taskset).
    let mut current_set: libc::cpu_set_t = unsafe { mem::zeroed() };
    let ret = unsafe {
        libc::sched_getaffinity(0, mem::size_of::<libc::cpu_set_t>(), &raw mut current_set)
    };
    if ret != 0 {
        tracing::debug!(
            %thread_name,
            err = std::io::Error::last_os_error().to_string(),
            "failed to get CPU affinity, skipping core pinning"
        );
        return;
    }

    // Collect available CPUs.
    let available: Vec<usize> = (0..libc::CPU_SETSIZE as usize)
        .filter(|&cpu| unsafe { libc::CPU_ISSET(cpu, &current_set) })
        .collect();

    if available.is_empty() {
        tracing::debug!(%thread_name, "no available CPUs found, skipping core pinning");
        return;
    }

    // Pick the next core in round-robin order.
    let idx = CORE_INDEX.fetch_add(1, Ordering::Relaxed) % available.len();
    let core = available[idx];

    // Build a single-CPU mask and apply it.
    let mut set: libc::cpu_set_t = unsafe { mem::zeroed() };
    unsafe { libc::CPU_SET(core, &mut set) };

    let ret =
        unsafe { libc::sched_setaffinity(0, mem::size_of::<libc::cpu_set_t>(), &raw const set) };
    if ret != 0 {
        tracing::debug!(
            %thread_name,
            core,
            err = std::io::Error::last_os_error().to_string(),
            "failed to pin thread to core"
        );
    } else {
        tracing::debug!(%thread_name, core, "pinned thread to core");
    }
}

/// Deprioritizes known background threads spawned by third-party libraries (`OpenTelemetry`,
/// `tracing-appender`, `reqwest`) by scanning `/proc/<pid>/task/` for matching thread names and
/// setting `SCHED_IDLE` scheduling policy + maximum niceness on them.
///
/// This is a hack: these threads are spawned by libraries that do not expose a way to hook into
/// thread initialization or expose the TIDs, so we have to discover them after the fact by
/// reading `/proc`.
///
/// Should be called once after tracing is initialized.
///
/// No-op on non-Linux platforms.
#[allow(clippy::missing_const_for_fn)]
pub fn deprioritize_background_threads() {
    #[cfg(target_os = "linux")]
    _deprioritize_background_threads();
}

/// Thread name prefixes to deprioritize.
#[cfg(target_os = "linux")]
const DEPRIORITIZE_THREAD_PREFIXES: &[&str] =
    &["OpenTelemetry.T", "tracing-appende", "reqwest-interna"];

#[cfg(target_os = "linux")]
fn _deprioritize_background_threads() {
    let pid = std::process::id();
    let task_dir = format!("/proc/{pid}/task");

    let entries = match std::fs::read_dir(&task_dir) {
        Ok(entries) => entries,
        Err(err) => {
            tracing::debug!(%err, "failed to read /proc task directory");
            return;
        }
    };

    for entry in entries.filter_map(Result::ok) {
        let tid_str = entry.file_name();
        let Some(tid_str) = tid_str.to_str() else { continue };
        let Ok(tid) = tid_str.parse::<i32>() else { continue };

        let comm_path = format!("{task_dir}/{tid_str}/comm");
        let comm = match std::fs::read_to_string(&comm_path) {
            Ok(c) => c,
            Err(_) => continue,
        };
        let comm = comm.trim();

        if !DEPRIORITIZE_THREAD_PREFIXES.iter().any(|prefix| comm.starts_with(prefix)) {
            continue;
        }

        // SCHED_IDLE is the lowest-priority scheduling class. The kernel will only schedule these
        // threads when no other (SCHED_OTHER/SCHED_BATCH/RT) threads need the CPU.
        // SAFETY: sched_setscheduler is safe to call with a valid TID.
        unsafe {
            let param = libc::sched_param { sched_priority: 0 };
            if libc::sched_setscheduler(tid, libc::SCHED_IDLE, std::ptr::from_ref(&param)) != 0 {
                tracing::debug!(
                    tid,
                    comm,
                    err = std::io::Error::last_os_error().to_string(),
                    "failed to set SCHED_IDLE"
                );
            }
        }

        tracing::debug!(tid, comm, "deprioritized background thread (SCHED_IDLE)");
    }
}
