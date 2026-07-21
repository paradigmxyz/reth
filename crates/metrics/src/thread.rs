//! Per-thread resource usage measurement.

use std::{marker::PhantomData, rc::Rc, time::Duration};

/// A snapshot used to measure resource usage on the current thread.
///
/// This type is thread-bound because [`Self::elapsed`] must sample the same thread as
/// [`Self::now`].
#[derive(Debug)]
#[must_use = "call elapsed() to obtain the resource usage delta"]
pub struct ThreadResourceUsage {
    start: Option<ThreadResourceUsageSnapshot>,
    _not_send: PhantomData<Rc<()>>,
}

impl ThreadResourceUsage {
    /// Captures the current thread's resource usage.
    pub fn now() -> Self {
        Self { start: platform::current_thread_resource_usage(), _not_send: PhantomData }
    }

    /// Returns the resource usage accumulated by the current thread since this snapshot.
    ///
    /// Returns `None` when per-thread resource usage is unsupported or could not be sampled.
    pub fn elapsed(&self) -> Option<ThreadResourceUsageDelta> {
        let start = self.start?;
        Some(platform::current_thread_resource_usage()?.saturating_delta(start))
    }
}

/// Resource usage accumulated by a thread over a measured interval.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ThreadResourceUsageDelta {
    /// Time spent executing in user mode.
    pub user_cpu_time: Duration,
    /// Time spent executing in kernel mode.
    pub system_cpu_time: Duration,
    /// Page faults serviced without I/O.
    pub minor_page_faults: u64,
    /// Page faults that required I/O.
    pub major_page_faults: u64,
    /// Voluntary context switches.
    pub voluntary_context_switches: u64,
    /// Involuntary context switches.
    pub involuntary_context_switches: u64,
    /// Block input operations.
    pub block_input_operations: u64,
    /// Block output operations.
    pub block_output_operations: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct ThreadResourceUsageSnapshot {
    user_cpu_time: Duration,
    system_cpu_time: Duration,
    minor_page_faults: u64,
    major_page_faults: u64,
    voluntary_context_switches: u64,
    involuntary_context_switches: u64,
    block_input_operations: u64,
    block_output_operations: u64,
}

impl ThreadResourceUsageSnapshot {
    const fn saturating_delta(self, earlier: Self) -> ThreadResourceUsageDelta {
        ThreadResourceUsageDelta {
            user_cpu_time: self.user_cpu_time.saturating_sub(earlier.user_cpu_time),
            system_cpu_time: self.system_cpu_time.saturating_sub(earlier.system_cpu_time),
            minor_page_faults: self.minor_page_faults.saturating_sub(earlier.minor_page_faults),
            major_page_faults: self.major_page_faults.saturating_sub(earlier.major_page_faults),
            voluntary_context_switches: self
                .voluntary_context_switches
                .saturating_sub(earlier.voluntary_context_switches),
            involuntary_context_switches: self
                .involuntary_context_switches
                .saturating_sub(earlier.involuntary_context_switches),
            block_input_operations: self
                .block_input_operations
                .saturating_sub(earlier.block_input_operations),
            block_output_operations: self
                .block_output_operations
                .saturating_sub(earlier.block_output_operations),
        }
    }
}

#[cfg(target_os = "linux")]
mod platform {
    use super::ThreadResourceUsageSnapshot;
    use std::{mem::MaybeUninit, time::Duration};

    pub(super) fn current_thread_resource_usage() -> Option<ThreadResourceUsageSnapshot> {
        let mut usage = MaybeUninit::<libc::rusage>::uninit();
        // SAFETY: `usage` is a valid writable pointer and is only initialized when getrusage
        // succeeds.
        if unsafe { libc::getrusage(libc::RUSAGE_THREAD, usage.as_mut_ptr()) } != 0 {
            return None
        }
        // SAFETY: getrusage returned success and initialized the structure.
        let usage = unsafe { usage.assume_init() };

        Some(ThreadResourceUsageSnapshot {
            user_cpu_time: timeval_to_duration(usage.ru_utime)?,
            system_cpu_time: timeval_to_duration(usage.ru_stime)?,
            minor_page_faults: usage.ru_minflt.try_into().ok()?,
            major_page_faults: usage.ru_majflt.try_into().ok()?,
            voluntary_context_switches: usage.ru_nvcsw.try_into().ok()?,
            involuntary_context_switches: usage.ru_nivcsw.try_into().ok()?,
            block_input_operations: usage.ru_inblock.try_into().ok()?,
            block_output_operations: usage.ru_oublock.try_into().ok()?,
        })
    }

    fn timeval_to_duration(timeval: libc::timeval) -> Option<Duration> {
        let seconds = timeval.tv_sec.try_into().ok()?;
        let microseconds: u32 = timeval.tv_usec.try_into().ok()?;
        if microseconds >= 1_000_000 {
            return None
        }
        Some(Duration::new(seconds, microseconds * 1_000))
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::thread::ThreadResourceUsage;

        #[test]
        fn samples_current_thread() {
            assert!(ThreadResourceUsage::now().elapsed().is_some());
        }

        #[test]
        fn measures_forced_minor_page_fault() {
            let mapping_len = 4096;
            // SAFETY: The mapping is private and anonymous, has a non-zero length, and the returned
            // address is checked before use.
            let mapping = unsafe {
                libc::mmap(
                    std::ptr::null_mut(),
                    mapping_len,
                    libc::PROT_READ | libc::PROT_WRITE,
                    libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
                    -1,
                    0,
                )
            };
            assert_ne!(mapping, libc::MAP_FAILED);

            let usage = ThreadResourceUsage::now();
            // SAFETY: `mapping` points to a valid writable mapping. Its first write requires Linux
            // to populate the anonymous page, which incurs a minor page fault.
            unsafe { mapping.cast::<u8>().write_volatile(1) };
            let elapsed = usage.elapsed();

            // SAFETY: `mapping` and `mapping_len` identify the live mapping created above.
            assert_eq!(unsafe { libc::munmap(mapping, mapping_len) }, 0);
            let elapsed = elapsed.expect("per-thread resource usage is supported on Linux");
            assert!(
                elapsed.minor_page_faults >= 1,
                "expected a minor page fault after touching a new anonymous mapping: {elapsed:?}"
            );
        }

        #[test]
        fn converts_timeval_to_duration() {
            let timeval = libc::timeval { tv_sec: 2, tv_usec: 345_678 };
            assert_eq!(timeval_to_duration(timeval), Some(Duration::new(2, 345_678_000)));
        }
    }
}

#[cfg(not(target_os = "linux"))]
mod platform {
    use super::ThreadResourceUsageSnapshot;

    #[allow(clippy::missing_const_for_fn)]
    pub(super) fn current_thread_resource_usage() -> Option<ThreadResourceUsageSnapshot> {
        None
    }

    #[cfg(test)]
    mod tests {
        use crate::thread::ThreadResourceUsage;

        #[test]
        fn unsupported_platform_returns_none() {
            assert!(ThreadResourceUsage::now().elapsed().is_none());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn computes_resource_usage_delta() {
        let earlier = ThreadResourceUsageSnapshot {
            user_cpu_time: Duration::from_micros(10),
            system_cpu_time: Duration::from_micros(20),
            minor_page_faults: 30,
            major_page_faults: 40,
            voluntary_context_switches: 50,
            involuntary_context_switches: 60,
            block_input_operations: 70,
            block_output_operations: 80,
        };
        let current = ThreadResourceUsageSnapshot {
            user_cpu_time: Duration::from_micros(11),
            system_cpu_time: Duration::from_micros(22),
            minor_page_faults: 33,
            major_page_faults: 44,
            voluntary_context_switches: 55,
            involuntary_context_switches: 66,
            block_input_operations: 77,
            block_output_operations: 88,
        };

        assert_eq!(
            current.saturating_delta(earlier),
            ThreadResourceUsageDelta {
                user_cpu_time: Duration::from_micros(1),
                system_cpu_time: Duration::from_micros(2),
                minor_page_faults: 3,
                major_page_faults: 4,
                voluntary_context_switches: 5,
                involuntary_context_switches: 6,
                block_input_operations: 7,
                block_output_operations: 8,
            }
        );
    }

    #[test]
    fn resource_usage_delta_saturates() {
        let earlier = ThreadResourceUsageSnapshot {
            user_cpu_time: Duration::from_secs(1),
            system_cpu_time: Duration::from_secs(1),
            minor_page_faults: 1,
            major_page_faults: 1,
            voluntary_context_switches: 1,
            involuntary_context_switches: 1,
            block_input_operations: 1,
            block_output_operations: 1,
        };

        assert_eq!(
            ThreadResourceUsageSnapshot::default().saturating_delta(earlier),
            ThreadResourceUsageDelta::default()
        );
    }
}
