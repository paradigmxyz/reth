//! Task utility functions.

pub use thread_priority::{self, *};

/// Increases the current thread's priority.
///
/// Tries [`ThreadPriority::Max`] first. If that fails (e.g. missing `CAP_SYS_NICE`),
/// falls back to a moderate bump via [`ThreadPriority::Crossplatform`] (~5 nice points
/// on unix). Failures are logged at `debug` level.
pub fn increase_thread_priority() {
    if let Err(err) = ThreadPriority::Max.set_for_current() {
        tracing::debug!(?err, "failed to set max thread priority, trying moderate bump");
        // Crossplatform value 62/99 ≈ nice -5 on unix.
        let fallback = ThreadPriority::Crossplatform(
            ThreadPriorityValue::try_from(62u8).expect("62 is within the valid 0..100 range"),
        );
        if let Err(err) = fallback.set_for_current() {
            tracing::debug!(?err, "failed to set moderate thread priority");
        }
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
