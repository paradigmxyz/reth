//! Last-level-cache (LLC) aware placement for jobs handed to Reth's thread pools.
//!
//! Exchanging cache lines costs far more between threads in different LLC domains (for example
//! the CCDs of an AMD EPYC, or the sockets of a dual-socket host), and the scheduler places threads
//! without knowing which of them share data. A job handed to a pool usually exchanges data with
//! the thread that submitted it, so [`JobDomain::capture`] records the submitter's domain and
//! [`JobDomain::run`] moves the pool thread that runs the job into that domain: it narrows the
//! thread's affinity mask to the domain, which migrates it there, then widens the mask again at
//! once. The thread stays where it landed unless the scheduler balances it away, so a job keeps its
//! locality while the domain has room, and the scheduler can still move it out of a crowded domain.
//!
//! A pin never widens a mask past operator restrictions (taskset, cgroups, isolated cores), nested
//! jobs run under the outer pin, and each domain grants at most one slot fewer than the CPUs a
//! pinning thread may use there, so pinned jobs alone never fill it.
//!
//! Ported from the per-job pin in commonware's `commonware-parallel` topology module
//! (<https://github.com/commonwarexyz/monorepo/pull/5377>), where the pin alone took a parallel
//! execution and merklization benchmark from ~12.2 ms to ~9.5 ms on an 8-domain EPYC.
//!
//! Domains come from the kernel's sysfs cache topology. On non-Linux platforms, hosts with a single
//! domain, or when `RETH_CACHE_AFFINITY=0` is set, capturing records nothing and running a job
//! changes no affinity.

/// The LLC domain of the thread that submitted a job, captured at submission and consumed on the
/// thread that runs it.
#[derive(Clone, Copy, Debug, Default)]
pub struct JobDomain(#[cfg(target_os = "linux")] Option<u16>);

impl JobDomain {
    /// Captures the calling thread's current LLC domain. Captures nothing when the host has fewer
    /// than two domains, placement is disabled, or the domain is unknown.
    #[inline]
    pub fn capture() -> Self {
        #[cfg(target_os = "linux")]
        {
            let topology = imp::topology();
            if topology.domains() <= 1 {
                return Self(None)
            }
            Self(topology.current_domain())
        }
        #[cfg(not(target_os = "linux"))]
        {
            Self()
        }
    }

    /// Runs `f` on the calling thread after moving the thread into the captured domain, holding
    /// one of the domain's slots until `f` returns or unwinds. Runs `f` in place when nothing
    /// was captured, the thread already holds a pin, or the domain is full.
    #[inline]
    pub fn run<R>(self, f: impl FnOnce() -> R) -> R {
        #[cfg(target_os = "linux")]
        let _guard = self.0.and_then(|domain| imp::AffinityGuard::pin_in(imp::topology(), domain));
        f()
    }
}

impl JobDomain {
    /// Moves the calling thread into the captured domain without holding a slot, for a long-lived
    /// loop whose jobs pin themselves with [`Self::run`]. The mask is narrowed and widened back at
    /// once, so the scheduler may later balance the thread elsewhere. Does nothing when nothing was
    /// captured, the thread holds a pin, or it already runs in the domain.
    #[inline]
    pub fn place(self) {
        #[cfg(target_os = "linux")]
        if let Some(domain) = self.0 {
            imp::place_in(imp::topology(), domain);
        }
    }
}

/// Returns the number of LLC domains detected for placement, or zero when placement is disabled
/// or unavailable on this host.
pub fn llc_domains() -> usize {
    #[cfg(target_os = "linux")]
    {
        let domains = imp::topology().domains();
        if domains > 1 {
            return domains
        }
    }
    0
}

/// Returns the LLC domain the calling thread currently runs on, for sharding shared state by
/// domain. Advisory: the thread may migrate right after. Returns `None` when the host has fewer
/// than two domains, placement is disabled, or the domain is unknown.
#[inline]
pub fn current_domain() -> Option<usize> {
    #[cfg(target_os = "linux")]
    {
        let topology = imp::topology();
        if topology.domains() > 1 {
            return topology.current_domain().map(usize::from)
        }
    }
    None
}

#[cfg(target_os = "linux")]
mod imp {
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        OnceLock,
    };

    /// Byte size of `cpu_set_t`, as passed to the affinity syscalls.
    const CPU_SET_BYTES: usize = std::mem::size_of::<libc::cpu_set_t>();

    /// A CPU affinity mask: a safe wrapper over `libc::cpu_set_t`.
    #[derive(Clone, Copy)]
    pub(super) struct CpuSet(libc::cpu_set_t);

    impl CpuSet {
        pub(super) const fn empty() -> Self {
            // SAFETY: an all-zero cpu_set_t is the empty set.
            Self(unsafe { std::mem::zeroed() })
        }

        pub(super) fn set(&mut self, cpu: usize) {
            assert!(cpu < libc::CPU_SETSIZE as usize, "cpu must be below CPU_SETSIZE");
            // SAFETY: cpu is in range and the set is valid.
            unsafe { libc::CPU_SET(cpu, &mut self.0) };
        }

        pub(super) fn contains(&self, cpu: usize) -> bool {
            if cpu >= libc::CPU_SETSIZE as usize {
                return false
            }
            // SAFETY: cpu is in range and the set is valid.
            unsafe { libc::CPU_ISSET(cpu, &self.0) }
        }

        fn is_subset_of(&self, other: &Self) -> bool {
            (0..libc::CPU_SETSIZE as usize).all(|cpu| !self.contains(cpu) || other.contains(cpu))
        }

        /// The CPUs in both sets, and how many there are.
        pub(super) fn within(&self, allowed: &Self) -> (Self, usize) {
            let mut both = Self::empty();
            let mut count = 0usize;
            for cpu in 0..(libc::CPU_SETSIZE as usize) {
                if self.contains(cpu) && allowed.contains(cpu) {
                    both.set(cpu);
                    count += 1;
                }
            }
            (both, count)
        }
    }

    /// The calling thread's affinity mask, or `None` when the kernel refuses the query.
    pub(super) fn thread_affinity() -> Option<CpuSet> {
        let mut mask = CpuSet::empty();
        // SAFETY: pid 0 is the calling thread, and the set is a valid cpu_set_t.
        if unsafe { libc::sched_getaffinity(0, CPU_SET_BYTES, &raw mut mask.0) } != 0 {
            return None
        }
        Some(mask)
    }

    /// Applies `mask` to the calling thread. Returns `false` when the kernel refuses.
    pub(super) fn set_thread_affinity(mask: &CpuSet) -> bool {
        // SAFETY: pid 0 is the calling thread, and the set is a valid cpu_set_t.
        unsafe { libc::sched_setaffinity(0, CPU_SET_BYTES, &raw const mask.0) == 0 }
    }

    /// The CPU the calling thread runs on. Advisory: the thread may migrate right after.
    fn current_cpu() -> Option<usize> {
        // SAFETY: sched_getcpu takes no arguments and returns -1 on error.
        usize::try_from(unsafe { libc::sched_getcpu() }).ok()
    }

    /// The process-wide topology, detected on first use.
    pub(super) fn topology() -> &'static Topology {
        static TOPOLOGY: OnceLock<Topology> = OnceLock::new();
        TOPOLOGY.get_or_init(Topology::detect)
    }

    /// The host's LLC domains, with the live pins of each.
    pub(super) struct Topology {
        /// Domain of each CPU, or `None` when its cache topology was unreadable.
        cpu_to_domain: Box<[Option<u16>]>,
        /// The CPUs of each domain.
        domain_sets: Box<[CpuSet]>,
        /// Live pins per domain.
        pins: Box<[AtomicUsize]>,
    }

    impl Topology {
        fn detect() -> Self {
            if std::env::var("RETH_CACHE_AFFINITY").is_ok_and(|v| v == "0" || v == "false") {
                tracing::info!(target: "tasks", "LLC-aware job placement disabled by RETH_CACHE_AFFINITY");
                return Self::empty()
            }
            // The affinity syscalls fail on every call when the possible-CPU count exceeds
            // cpu_set_t's capacity, so report no domains then.
            if thread_affinity().is_none() {
                return Self::empty()
            }
            let topology = Self::from_sysfs().unwrap_or_else(Self::empty);
            tracing::info!(target: "tasks", llc_domains = topology.domains(), "Detected LLC domains for job placement");
            topology
        }

        fn empty() -> Self {
            Self { cpu_to_domain: Box::new([]), domain_sets: Box::new([]), pins: Box::new([]) }
        }

        pub(super) fn domains(&self) -> usize {
            self.domain_sets.len()
        }

        fn domain_of(&self, cpu: usize) -> Option<u16> {
            self.cpu_to_domain.get(cpu).copied().flatten()
        }

        pub(super) fn current_domain(&self) -> Option<u16> {
            self.domain_of(current_cpu()?)
        }

        /// The `shared_cpu_list` of the CPU's highest-level Data or Unified cache leaf.
        fn llc_list(cpu_dir: &std::path::Path) -> Option<String> {
            let mut best: Option<(u32, String)> = None;
            for entry in std::fs::read_dir(cpu_dir.join("cache")).ok()?.flatten() {
                if !entry.file_name().to_string_lossy().starts_with("index") {
                    continue
                }
                let leaf = entry.path();
                let Ok(kind) = std::fs::read_to_string(leaf.join("type")) else { continue };
                let kind = kind.trim();
                if kind != "Data" && kind != "Unified" {
                    continue
                }
                let Some(level) = std::fs::read_to_string(leaf.join("level"))
                    .ok()
                    .and_then(|s| s.trim().parse::<u32>().ok())
                else {
                    continue
                };
                let Ok(list) = std::fs::read_to_string(leaf.join("shared_cpu_list")) else {
                    continue
                };
                let list = list.trim();
                if !list.is_empty() && best.as_ref().is_none_or(|(l, _)| level > *l) {
                    best = Some((level, list.to_string()));
                }
            }
            best.map(|(_, list)| list)
        }

        fn from_sysfs() -> Option<Self> {
            let mut lists: Vec<(usize, String)> = Vec::new();
            for entry in std::fs::read_dir("/sys/devices/system/cpu").ok()?.flatten() {
                let name = entry.file_name();
                let Some(id) = name
                    .to_string_lossy()
                    .strip_prefix("cpu")
                    .and_then(|s| s.parse::<usize>().ok())
                else {
                    continue
                };
                if let Some(list) = Self::llc_list(&entry.path()) {
                    lists.push((id, list));
                }
            }
            Self::from_lists(&lists)
        }

        /// Builds the tables from `(cpu, shared_cpu_list)` pairs, one domain per distinct list.
        /// Returns `None` for a single domain, all-private caches, a CPU at or past
        /// `CPU_SETSIZE`, or more domains than a `u16` indexes.
        pub(super) fn from_lists(lists: &[(usize, String)]) -> Option<Self> {
            if lists.is_empty() || lists.iter().any(|(cpu, _)| *cpu >= libc::CPU_SETSIZE as usize) {
                return None
            }
            let max_cpu = lists.iter().map(|(cpu, _)| *cpu).max()?;
            let mut table: Vec<Option<u16>> = vec![None; max_cpu + 1];
            let mut domains: Vec<&str> = Vec::new();
            let mut sets: Vec<CpuSet> = Vec::new();
            let mut widths: Vec<usize> = Vec::new();
            for (cpu, list) in lists {
                let domain = domains.iter().position(|&d| d == list).unwrap_or_else(|| {
                    domains.push(list);
                    sets.push(CpuSet::empty());
                    widths.push(0);
                    domains.len() - 1
                });
                table[*cpu] = Some(u16::try_from(domain).ok()?);
                sets[domain].set(*cpu);
                widths[domain] += 1;
            }
            if domains.len() <= 1 || widths.iter().all(|&width| width < 2) {
                return None
            }
            let pins = (0..sets.len()).map(|_| AtomicUsize::new(0)).collect();
            Some(Self {
                cpu_to_domain: table.into_boxed_slice(),
                domain_sets: sets.into_boxed_slice(),
                pins,
            })
        }

        #[cfg(test)]
        pub(super) fn domain_set(&self, domain: usize) -> CpuSet {
            self.domain_sets[domain]
        }

        #[cfg(test)]
        pub(super) fn live_pins(&self, domain: usize) -> usize {
            self.pins[domain].load(Ordering::Acquire)
        }
    }

    std::thread_local! {
        /// Whether this thread holds a live [`AffinityGuard`]. Nested jobs run under the outer pin.
        static PINNED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    }

    /// Holds one of a domain's slots for the calling thread after [`Self::pin_in`] moved it there.
    pub(super) struct AffinityGuard {
        /// The pre-pin mask and the pin mask, kept only when widening failed so drop retries it.
        pending: Option<(CpuSet, CpuSet)>,
        topology: &'static Topology,
        domain: usize,
        /// The guard acts on the creating thread's mask, so it must not move between threads.
        _thread_bound: std::marker::PhantomData<*const ()>,
    }

    impl AffinityGuard {
        /// Moves the calling thread onto the CPUs of `domain` its current mask allows, then
        /// widens the mask back at once. Returns `None` without pinning when the thread already
        /// holds a pin, the mask cannot be read or applied, fewer than two of the domain's CPUs
        /// are allowed, or the domain already holds one pin fewer than it has allowed CPUs.
        pub(super) fn pin_in(topology: &'static Topology, domain: u16) -> Option<Self> {
            if PINNED.get() {
                return None
            }
            let domain = domain as usize;
            let target = topology.domain_sets.get(domain)?;
            let saved = thread_affinity()?;
            let (effective, allowed) = target.within(&saved);
            if allowed <= 1 {
                return None
            }

            // At most `allowed - 1` live pins, so pinned jobs never crowd out the submitter.
            let cap = allowed - 1;
            let pins = &topology.pins[domain];
            let mut live = pins.load(Ordering::Acquire);
            loop {
                if live >= cap {
                    return None
                }
                match pins.compare_exchange_weak(
                    live,
                    live + 1,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                ) {
                    Ok(_) => break,
                    Err(current) => live = current,
                }
            }
            // A thread already running in the domain needs no move, which saves both syscalls on
            // the common path of a worker that served this domain before.
            if topology.current_domain() == Some(domain as u16) {
                PINNED.set(true);
                return Some(Self {
                    pending: None,
                    topology,
                    domain,
                    _thread_bound: std::marker::PhantomData,
                })
            }
            if !set_thread_affinity(&effective) {
                pins.fetch_sub(1, Ordering::AcqRel);
                return None
            }

            // Narrowing moved the thread into the domain; widening again leaves it there while
            // letting the scheduler balance it away later. A failed widening is retried at drop.
            let pending = (!set_thread_affinity(&saved)).then_some((saved, effective));
            PINNED.set(true);
            Some(Self { pending, topology, domain, _thread_bound: std::marker::PhantomData })
        }
    }

    /// Moves the calling thread onto the CPUs of `domain` its mask allows and widens the mask
    /// back at once, without taking a slot. Skips the move when the thread holds a pin, already
    /// runs in the domain, or fewer than two of the domain's CPUs are allowed.
    pub(super) fn place_in(topology: &'static Topology, domain: u16) {
        if PINNED.get() || topology.current_domain() == Some(domain) {
            return
        }
        let Some(target) = topology.domain_sets.get(domain as usize) else { return };
        let Some(saved) = thread_affinity() else { return };
        let (effective, allowed) = target.within(&saved);
        if allowed > 1 && set_thread_affinity(&effective) {
            set_thread_affinity(&saved);
        }
    }

    impl Drop for AffinityGuard {
        fn drop(&mut self) {
            // Retry a failed widening only while the mask is still within the pin: a mask outside
            // it was rewritten mid-job by an operator, and restoring would revive removed CPUs.
            if let Some((saved, effective)) = self.pending &&
                thread_affinity().is_some_and(|current| current.is_subset_of(&effective))
            {
                set_thread_affinity(&saved);
            }
            PINNED.set(false);
            self.topology.pins[self.domain].fetch_sub(1, Ordering::AcqRel);
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn allowed_cpus() -> Vec<usize> {
            let mask = thread_affinity().unwrap();
            (0..libc::CPU_SETSIZE as usize).filter(|cpu| mask.contains(*cpu)).collect()
        }

        /// Splits the process's allowed CPUs into two synthetic domains.
        fn synthetic_split() -> Option<&'static Topology> {
            let cpus = allowed_cpus();
            if cpus.len() < 4 {
                return None
            }
            let (a, b) = cpus.split_at(cpus.len() / 2);
            let list =
                |half: &[usize]| half.iter().map(|c| c.to_string()).collect::<Vec<_>>().join(",");
            let (la, lb) = (list(a), list(b));
            let lists: Vec<_> = a
                .iter()
                .map(|c| (*c, la.clone()))
                .chain(b.iter().map(|c| (*c, lb.clone())))
                .collect();
            Some(Box::leak(Box::new(Topology::from_lists(&lists)?)))
        }

        #[test]
        fn from_lists_groups_domains() {
            let lists = [
                (0, "0-1".to_string()),
                (1, "0-1".to_string()),
                (2, "2-3".to_string()),
                (3, "2-3".to_string()),
            ];
            let topology = Topology::from_lists(&lists).unwrap();
            assert_eq!(topology.domains(), 2);
            assert_eq!(topology.domain_of(1), Some(0));
            assert_eq!(topology.domain_of(2), Some(1));
            assert_eq!(topology.domain_of(9), None);
        }

        #[test]
        fn from_lists_rejects_single_and_private_domains() {
            assert!(Topology::from_lists(&[(0, "0-1".into()), (1, "0-1".into())]).is_none());
            assert!(Topology::from_lists(&[(0, "0".into()), (1, "1".into())]).is_none());
        }

        #[test]
        fn pin_moves_into_domain_widens_and_releases() {
            let Some(topology) = synthetic_split() else { return };
            std::thread::spawn(move || {
                let before = allowed_cpus();
                let guard = AffinityGuard::pin_in(topology, 1).expect("pin");
                // Widened again right away.
                assert_eq!(allowed_cpus(), before);
                // Moved into the domain.
                let cpu = current_cpu().unwrap();
                assert!(topology.domain_set(1).contains(cpu), "cpu {cpu} outside domain");
                assert_eq!(topology.live_pins(1), 1);
                // Nested pins are refused.
                assert!(AffinityGuard::pin_in(topology, 0).is_none());
                drop(guard);
                assert_eq!(topology.live_pins(1), 0);
            })
            .join()
            .unwrap();
        }

        #[test]
        fn pins_are_capped_below_domain_width() {
            let Some(topology) = synthetic_split() else { return };
            let width = topology.domain_set(0).within(&thread_affinity().unwrap()).1;
            let (tx, rx) = std::sync::mpsc::channel();
            let release = std::sync::Arc::new(std::sync::Barrier::new(width + 1));
            let handles: Vec<_> = (0..width)
                .map(|_| {
                    let (tx, release) = (tx.clone(), release.clone());
                    std::thread::spawn(move || {
                        let guard = AffinityGuard::pin_in(topology, 0);
                        tx.send(guard.is_some()).unwrap();
                        release.wait();
                    })
                })
                .collect();
            let pinned = (0..width).filter(|_| rx.recv().unwrap()).count();
            assert_eq!(pinned, width - 1);
            release.wait();
            for handle in handles {
                handle.join().unwrap();
            }
            assert_eq!(topology.live_pins(0), 0);
        }
    }
}
