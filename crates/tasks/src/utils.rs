//! Task utility functions.

pub use thread_priority::{self, *};

/// Runs the given closure exactly once per call site.
///
/// Each invocation expands to its own `static Once`, so two `once!` calls in the same function
/// are independent. Useful for one-time thread setup (priority, affinity) on threads that may
/// be re-entered (e.g. tokio blocking pool).
#[macro_export]
macro_rules! once {
    ($e:expr) => {{
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once($e);
    }};
}

/// CPUs reserved for latency-critical threads, set by [`reserve_critical_cpus`].
static RESERVED_CPUS: std::sync::OnceLock<Vec<usize>> = std::sync::OnceLock::new();

/// Reserves the first `count` CPUs of the process's affinity mask for latency-critical threads
/// (the engine and the payload builder). Their hyperthread siblings are left idle, and the
/// calling thread, and every thread it spawns later, is restricted to the remaining CPUs.
///
/// Call it at startup, before any other thread is spawned. Linux only; a no-op elsewhere or if
/// fewer than `count` + 2 cores remain.
pub fn reserve_critical_cpus(count: usize) {
    #[cfg(target_os = "linux")]
    // SAFETY: sched_getaffinity/sched_setaffinity on the calling thread with a valid cpu_set_t.
    unsafe {
        let mut set: libc::cpu_set_t = std::mem::zeroed();
        if libc::sched_getaffinity(0, std::mem::size_of::<libc::cpu_set_t>(), &mut set) != 0 {
            return
        }
        let allowed: Vec<usize> =
            (0..libc::CPU_SETSIZE as usize).filter(|&c| libc::CPU_ISSET(c, &set)).collect();
        let siblings = |cpu: usize| -> Vec<usize> {
            let path = format!("/sys/devices/system/cpu/cpu{cpu}/topology/thread_siblings_list");
            let Ok(list) = std::fs::read_to_string(path) else { return vec![cpu] };
            list.trim()
                .split(',')
                .flat_map(|part| match part.split_once('-') {
                    Some((a, b)) => {
                        let (a, b) = (a.parse().unwrap_or(cpu), b.parse().unwrap_or(cpu));
                        (a..=b).collect::<Vec<_>>()
                    }
                    None => vec![part.parse().unwrap_or(cpu)],
                })
                .collect()
        };
        let mut reserved = Vec::new();
        let mut excluded = std::collections::HashSet::new();
        for &cpu in &allowed {
            if reserved.len() == count {
                break
            }
            if excluded.contains(&cpu) {
                continue
            }
            reserved.push(cpu);
            excluded.extend(siblings(cpu));
            excluded.insert(cpu);
        }
        let rest: Vec<usize> = allowed.iter().copied().filter(|c| !excluded.contains(c)).collect();
        if reserved.len() < count || rest.len() < 4 {
            return
        }
        let mut rest_set: libc::cpu_set_t = std::mem::zeroed();
        for &c in &rest {
            libc::CPU_SET(c, &mut rest_set);
        }
        if libc::sched_setaffinity(0, std::mem::size_of::<libc::cpu_set_t>(), &rest_set) == 0 {
            tracing::info!(?reserved, ?rest, "reserved CPUs for critical threads");
            let _ = RESERVED_CPUS.set(reserved);
        }
    }
}

/// Restricts the calling thread, and every thread it spawns later, to one hardware thread per
/// physical core of the process's affinity mask, so no two threads share a core. Linux only.
pub fn restrict_to_physical_cores() {
    #[cfg(target_os = "linux")]
    // SAFETY: sched_getaffinity/sched_setaffinity on the calling thread with a valid cpu_set_t.
    unsafe {
        let mut set: libc::cpu_set_t = std::mem::zeroed();
        if libc::sched_getaffinity(0, std::mem::size_of::<libc::cpu_set_t>(), &mut set) != 0 {
            return
        }
        let mut seen = std::collections::HashSet::new();
        let mut keep: libc::cpu_set_t = std::mem::zeroed();
        let mut kept = 0;
        for cpu in 0..libc::CPU_SETSIZE as usize {
            if !libc::CPU_ISSET(cpu, &set) {
                continue
            }
            let path = format!("/sys/devices/system/cpu/cpu{cpu}/topology/thread_siblings_list");
            let key = std::fs::read_to_string(path).unwrap_or_else(|_| cpu.to_string());
            if seen.insert(key.trim().to_string()) {
                libc::CPU_SET(cpu, &mut keep);
                kept += 1;
            }
        }
        if kept >= 2 {
            libc::sched_setaffinity(0, std::mem::size_of::<libc::cpu_set_t>(), &keep);
        }
    }
}

/// Pins the calling thread to reserved critical CPU `slot`, if [`reserve_critical_cpus`] reserved
/// one. No-op otherwise.
pub fn pin_current_thread_to_reserved_cpu(slot: usize) {
    #[cfg(target_os = "linux")]
    if let Some(&cpu) = RESERVED_CPUS.get().and_then(|cpus| cpus.get(slot)) {
        // SAFETY: sched_setaffinity on the calling thread with a valid cpu_set_t.
        unsafe {
            let mut set: libc::cpu_set_t = std::mem::zeroed();
            libc::CPU_SET(cpu, &mut set);
            libc::sched_setaffinity(0, std::mem::size_of::<libc::cpu_set_t>(), &set);
        }
    }
    #[cfg(not(target_os = "linux"))]
    let _ = slot;
}

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

/// Deprioritizes known background threads spawned by third-party libraries (`tracing-appender`,
/// `reqwest`) by scanning `/proc/<pid>/task/` for matching thread names and setting `SCHED_IDLE`
/// scheduling policy on them.
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
///
/// Do not re-add the OTLP span exporter (`OpenTelemetry.T`) to avoid CPU spinning.
/// See <https://github.com/paradigmxyz/reth/pull/27370>.
#[cfg(target_os = "linux")]
const DEPRIORITIZE_THREAD_PREFIXES: &[&str] = &["tracing-appende", "reqwest-interna"];

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
