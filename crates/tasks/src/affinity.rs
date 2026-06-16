//! CPU affinity helpers for latency-sensitive engine work.

use std::sync::OnceLock;

#[derive(Debug, Clone, Copy)]
pub(crate) enum DedicatedCore {
    Engine,
    Prewarm,
}

#[derive(Debug, Clone)]
struct AffinityConfig {
    general_cores: Vec<usize>,
    engine_core: usize,
    prewarm_core: usize,
}

static AFFINITY_CONFIG: OnceLock<AffinityConfig> = OnceLock::new();

/// Reserves the last two currently allowed CPUs for engine and prewarm work.
pub fn configure_reserved_cores(reserved_cores: usize) {
    if reserved_cores < 2 {
        return
    }

    let Some(allowed_cores) = current_affinity() else {
        return
    };
    if allowed_cores.len() <= reserved_cores {
        return
    }

    let split = allowed_cores.len() - reserved_cores;
    let general_cores = allowed_cores[..split].to_vec();
    let engine_core = allowed_cores[split];
    let prewarm_core = allowed_cores[split + 1];

    let config = AffinityConfig { general_cores, engine_core, prewarm_core };
    let _ = AFFINITY_CONFIG.set(config.clone());
    apply_to_all_threads(&config.general_cores);
}

pub(crate) fn dedicated_core_for_name(name: &str) -> Option<DedicatedCore> {
    match name {
        "engine" | "payload-builder" => Some(DedicatedCore::Engine),
        "prewarm" | "prewarm-txs" | "builder-prewarm" => Some(DedicatedCore::Prewarm),
        _ => None,
    }
}

pub(crate) fn pin_current_thread(core: DedicatedCore) {
    let Some(config) = AFFINITY_CONFIG.get() else {
        return
    };
    let cpu = match core {
        DedicatedCore::Engine => config.engine_core,
        DedicatedCore::Prewarm => config.prewarm_core,
    };
    set_current_thread_affinity(&[cpu]);
}

#[cfg(target_os = "linux")]
fn current_affinity() -> Option<Vec<usize>> {
    let mut set = new_cpu_set();
    let result = unsafe {
        libc::sched_getaffinity(0, std::mem::size_of::<libc::cpu_set_t>(), &mut set)
    };
    if result != 0 {
        return None
    }

    let cores = (0..libc::CPU_SETSIZE as usize)
        .filter(|&cpu| unsafe { libc::CPU_ISSET(cpu, &set) })
        .collect::<Vec<_>>();
    (!cores.is_empty()).then_some(cores)
}

#[cfg(not(target_os = "linux"))]
fn current_affinity() -> Option<Vec<usize>> {
    None
}

#[cfg(target_os = "linux")]
fn set_current_thread_affinity(cores: &[usize]) {
    let mut set = new_cpu_set();
    for &cpu in cores {
        unsafe { libc::CPU_SET(cpu, &mut set) };
    }
    let result = unsafe {
        libc::sched_setaffinity(0, std::mem::size_of::<libc::cpu_set_t>(), &set)
    };
    if result != 0 {
        tracing::debug!(?cores, err = %std::io::Error::last_os_error(), "failed to set thread affinity");
    }
}

#[cfg(not(target_os = "linux"))]
fn set_current_thread_affinity(_cores: &[usize]) {}

#[cfg(target_os = "linux")]
fn apply_to_all_threads(cores: &[usize]) {
    let Ok(entries) = std::fs::read_dir("/proc/self/task") else {
        set_current_thread_affinity(cores);
        return
    };

    for entry in entries.flatten() {
        let Some(tid) =
            entry.file_name().to_str().and_then(|name| name.parse::<libc::pid_t>().ok())
        else {
            continue
        };
        let mut set = new_cpu_set();
        for &cpu in cores {
            unsafe { libc::CPU_SET(cpu, &mut set) };
        }
        let result = unsafe {
            libc::sched_setaffinity(tid, std::mem::size_of::<libc::cpu_set_t>(), &set)
        };
        if result != 0 {
            tracing::debug!(tid, ?cores, err = %std::io::Error::last_os_error(), "failed to set thread affinity");
        }
    }
}

#[cfg(not(target_os = "linux"))]
fn apply_to_all_threads(cores: &[usize]) {
    set_current_thread_affinity(cores);
}

#[cfg(target_os = "linux")]
fn new_cpu_set() -> libc::cpu_set_t {
    unsafe { std::mem::zeroed() }
}
