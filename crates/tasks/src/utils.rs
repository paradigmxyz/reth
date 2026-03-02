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
/// setting `SCHED_BATCH` scheduling policy + maximum niceness on them.
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
    use thread_priority::{
        NormalThreadSchedulePolicy, ThreadPriority, ThreadSchedulePolicy, NICENESS_MIN,
    };

    let policy = ThreadSchedulePolicy::Normal(NormalThreadSchedulePolicy::Batch);

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
        let Ok(tid) = tid_str.parse::<u64>() else { continue };

        let comm_path = format!("{task_dir}/{tid_str}/comm");
        let comm = match std::fs::read_to_string(&comm_path) {
            Ok(c) => c,
            Err(_) => continue,
        };
        let comm = comm.trim();

        if !DEPRIORITIZE_THREAD_PREFIXES.iter().any(|prefix| comm.starts_with(prefix)) {
            continue;
        }

        // set_thread_priority_and_policy only works with pthread_t handles, not kernel TIDs.
        // We must use sched_setattr directly because these threads are spawned by third-party
        // libraries and we only have their kernel TIDs from /proc.
        if let Err(err) = set_thread_priority_and_policy_by_tid(tid, ThreadPriority::Min, policy) {
            tracing::debug!(tid, comm, ?err, "failed to deprioritize background thread");
            continue;
        }

        tracing::debug!(
            tid,
            comm,
            "deprioritized background thread (SCHED_BATCH, nice {})",
            NICENESS_MIN
        );
    }
}

/// Sets thread priority and scheduling policy for a thread identified by its kernel TID, using
/// the `sched_setattr` syscall.
///
/// Unlike [`set_thread_priority_and_policy`] which requires a `pthread_t` handle, this works with
/// raw kernel TIDs (e.g. from `/proc/<pid>/task/<tid>`).
#[cfg(target_os = "linux")]
fn set_thread_priority_and_policy_by_tid(
    tid: u64,
    priority: thread_priority::ThreadPriority,
    policy: thread_priority::ThreadSchedulePolicy,
) -> std::io::Result<()> {
    use thread_priority::{NormalThreadSchedulePolicy, ThreadSchedulePolicy, NICENESS_MIN};

    let (sched_policy, sched_nice) = match policy {
        ThreadSchedulePolicy::Normal(NormalThreadSchedulePolicy::Batch) => {
            (libc::SCHED_BATCH as u32, NICENESS_MIN as i32)
        }
        ThreadSchedulePolicy::Normal(NormalThreadSchedulePolicy::Idle) => {
            (libc::SCHED_IDLE as u32, 0)
        }
        ThreadSchedulePolicy::Normal(NormalThreadSchedulePolicy::Other) => {
            let nice = match priority {
                ThreadPriority::Min => NICENESS_MIN as i32,
                _ => 0,
            };
            (libc::SCHED_OTHER as u32, nice)
        }
        _ => return Err(std::io::Error::from_raw_os_error(libc::EINVAL)),
    };

    // Mirror of thread_priority::SchedAttr (which has private fields).
    #[repr(C)]
    struct SchedAttr {
        size: u32,
        sched_policy: u32,
        sched_flags: u64,
        sched_nice: i32,
        sched_priority: u32,
        sched_runtime: u64,
        sched_deadline: u64,
        sched_period: u64,
        sched_util_min: u32,
        sched_util_max: u32,
    }

    let attr = SchedAttr {
        size: std::mem::size_of::<SchedAttr>() as u32,
        sched_policy,
        sched_nice,
        sched_flags: 0,
        sched_priority: 0,
        sched_runtime: 0,
        sched_deadline: 0,
        sched_period: 0,
        sched_util_min: 0,
        sched_util_max: 0,
    };

    // SAFETY: sched_setattr is safe to call with a valid TID and a properly initialized attr.
    let ret = unsafe {
        libc::syscall(libc::SYS_sched_setattr, tid as libc::pid_t, std::ptr::from_ref(&attr), 0u32)
    };

    if ret < 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}
