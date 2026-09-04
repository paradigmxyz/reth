//! Atomic bookkeeping shared by payload leases and their concurrency models.

use std::sync::atomic::{AtomicUsize, Ordering};

/// Counts payload jobs that may still read an in-memory overlay.
///
/// The engine must serialize acquisition with its decision to reclaim an overlay. Other threads
/// may release leases concurrently, but a zero observation cannot exclude a later acquisition.
/// Completion notifications are hints: the engine must check this counter again before reclaiming.
#[derive(Debug, Default)]
pub(super) struct PayloadBuildCounter<A = AtomicUsize>(A);

impl<A: CounterAtomic> PayloadBuildCounter<A> {
    pub(super) fn acquire(&self) {
        self.0.fetch_add(1, Ordering::AcqRel);
    }

    /// Releases one lease and returns whether a completion notification should be sent.
    pub(super) fn release(&self) -> bool {
        let previous = self.0.fetch_sub(1, Ordering::AcqRel);
        debug_assert!(previous > 0, "payload build lease count underflow");
        previous == 1
    }

    pub(super) fn is_active(&self) -> bool {
        self.0.load(Ordering::Acquire) != 0
    }
}

/// Allows Loom to instrument the production ordering and transition logic without replacing
/// atomics in the rest of the engine or its dependencies.
pub(super) trait CounterAtomic {
    fn fetch_add(&self, value: usize, ordering: Ordering) -> usize;
    fn fetch_sub(&self, value: usize, ordering: Ordering) -> usize;
    fn load(&self, ordering: Ordering) -> usize;
}

impl CounterAtomic for AtomicUsize {
    fn fetch_add(&self, value: usize, ordering: Ordering) -> usize {
        Self::fetch_add(self, value, ordering)
    }

    fn fetch_sub(&self, value: usize, ordering: Ordering) -> usize {
        Self::fetch_sub(self, value, ordering)
    }

    fn load(&self, ordering: Ordering) -> usize {
        Self::load(self, ordering)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use loom::{
        sync::{atomic::AtomicUsize, Arc},
        thread,
    };

    impl CounterAtomic for AtomicUsize {
        fn fetch_add(&self, value: usize, ordering: Ordering) -> usize {
            Self::fetch_add(self, value, ordering)
        }

        fn fetch_sub(&self, value: usize, ordering: Ordering) -> usize {
            Self::fetch_sub(self, value, ordering)
        }

        fn load(&self, ordering: Ordering) -> usize {
            Self::load(self, ordering)
        }
    }

    #[test]
    fn loom_final_release_notifies_once() {
        loom::model(|| {
            let counter = Arc::new(PayloadBuildCounter(AtomicUsize::new(0)));
            let notifications = Arc::new(AtomicUsize::new(0));
            let mut jobs = Vec::new();
            for _ in 0..2 {
                counter.acquire();
            }
            for _ in 0..2 {
                let counter = Arc::clone(&counter);
                let notifications = Arc::clone(&notifications);
                jobs.push(thread::spawn(move || {
                    if counter.release() {
                        notifications.fetch_add(1, Ordering::Relaxed);
                    }
                }));
            }
            for job in jobs {
                job.join().unwrap();
            }
            assert!(!counter.is_active());
            assert_eq!(notifications.load(Ordering::Relaxed), 1);
        });
    }

    #[test]
    fn loom_inactive_observer_sees_all_completed_jobs() {
        loom::model(|| {
            let counter = Arc::new(PayloadBuildCounter(AtomicUsize::new(0)));
            let writes = Arc::new([AtomicUsize::new(0), AtomicUsize::new(0)]);
            counter.acquire();
            counter.acquire();
            let mut jobs = Vec::new();
            for index in 0..2 {
                let counter = Arc::clone(&counter);
                let writes = Arc::clone(&writes);
                jobs.push(thread::spawn(move || {
                    writes[index].store(1, Ordering::Relaxed);
                    counter.release();
                }));
            }
            // Observe before joining: a join would publish the writes independently of the
            // counter and hide missing synchronization in its release/acquire operations.
            if !counter.is_active() {
                assert_eq!(writes[0].load(Ordering::Relaxed), 1);
                assert_eq!(writes[1].load(Ordering::Relaxed), 1);
            }
            for job in jobs {
                job.join().unwrap();
            }
        });
    }

    #[test]
    fn loom_reacquire_keeps_stale_completion_from_reclaiming_state() {
        loom::model(|| {
            let counter = Arc::new(PayloadBuildCounter(AtomicUsize::new(0)));
            let notifications = Arc::new(AtomicUsize::new(0));
            counter.acquire();
            let old_counter = Arc::clone(&counter);
            let old_notifications = Arc::clone(&notifications);
            let job = thread::spawn(move || {
                if old_counter.release() {
                    old_notifications.fetch_add(1, Ordering::Relaxed);
                }
            });

            // Acquisition and the handoff decision stay on the engine thread, while the old
            // job's final release and its notification can occur on either side of acquisition.
            counter.acquire();
            let _notification = notifications.load(Ordering::Relaxed);
            assert!(counter.is_active());
            if counter.release() {
                notifications.fetch_add(1, Ordering::Relaxed);
            }
            job.join().unwrap();
            assert!(!counter.is_active());
            assert!((1..=2).contains(&notifications.load(Ordering::Relaxed)));
        });
    }
}
