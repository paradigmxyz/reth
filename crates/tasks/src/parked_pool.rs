//! A thread pool whose idle threads sleep until a broadcast wakes them.

use std::{
    any::Any,
    collections::VecDeque,
    panic::{catch_unwind, resume_unwind, AssertUnwindSafe},
    ptr::NonNull,
    sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock},
    thread,
};

/// A fixed set of threads that sleep on a condition variable until
/// [`broadcast`](Self::broadcast) hands them a job.
///
/// A broadcast wakes only as many threads as it runs on, and an idle thread never scans other
/// threads for work. This suits jobs that hold a thread for a long time and block on their own
/// channels, such as proof workers: on a rayon pool, every broadcast wakes every thread, and each
/// idle thread scans every other thread's queue before it sleeps again.
///
/// The threads are spawned on first use and exit when the pool is dropped.
#[derive(Debug)]
pub struct ParkedThreadPool {
    shared: OnceLock<Arc<Shared>>,
    num_threads: usize,
    thread_name_prefix: &'static str,
}

impl ParkedThreadPool {
    /// Creates a lazy pool with the given number of threads and a thread name prefix.
    ///
    /// Thread names follow the pattern `"{prefix}-{index:02}"`.
    pub const fn new(num_threads: usize, thread_name_prefix: &'static str) -> Self {
        Self { shared: OnceLock::new(), num_threads, thread_name_prefix }
    }

    /// Returns the number of threads the pool is configured with, without spawning them.
    pub const fn num_threads(&self) -> usize {
        self.num_threads
    }

    /// Returns `true` if the pool's threads have been spawned.
    pub fn is_initialized(&self) -> bool {
        self.shared.get().is_some()
    }

    /// Returns the number of threads in the pool, spawning them on first access.
    pub fn current_num_threads(&self) -> usize {
        self.shared();
        self.num_threads
    }

    /// Returns the state shared with the threads, spawning them on first access.
    fn shared(&self) -> &Arc<Shared> {
        self.shared.get_or_init(|| {
            let prefix = self.thread_name_prefix;
            assert!(self.num_threads > 0, "{prefix} pool needs at least one thread");
            let shared = Arc::new(Shared {
                state: Mutex::new(State {
                    queue: VecDeque::with_capacity(self.num_threads),
                    shutdown: false,
                }),
                job_available: Condvar::new(),
                job_finished: Condvar::new(),
            });
            for index in 0..self.num_threads {
                let thread_shared = Arc::clone(&shared);
                if let Err(err) = thread::Builder::new()
                    .name(format!("{prefix}-{index:02}"))
                    .spawn(move || thread_shared.run_thread())
                {
                    // Let the threads already spawned exit, since no handle will ever drop them.
                    shared.shut_down();
                    panic!("failed to spawn {prefix} thread: {err}");
                }
            }
            shared
        })
    }

    /// Runs `f` `num_threads` times on the pool's threads and waits for all runs, waking only
    /// as many threads as there are runs. `num_threads` is capped at the pool's size.
    ///
    /// Each run takes the first free thread, so a thread that finishes its run before another
    /// thread wakes may start a second one. Concurrent broadcasts are served in the order they
    /// were made. If `f` panics, the panic is resumed here once every run has finished.
    ///
    /// Do not call this from one of the pool's own threads: a broadcast that needs the calling
    /// thread waits for itself.
    pub fn broadcast(&self, num_threads: usize, f: impl Fn() + Sync) {
        let num_threads = num_threads.min(self.num_threads);
        if num_threads == 0 {
            return
        }

        let f: &(dyn Fn() + Sync) = &f;
        // SAFETY: only the lifetime is erased. The threads call `f` only before they mark their
        // run finished, and this function returns only after all `num_threads` runs finished.
        let f = unsafe { std::mem::transmute::<&(dyn Fn() + Sync), &'static (dyn Fn() + Sync)>(f) };
        let mut job = Job { f, unclaimed: num_threads, unfinished: num_threads, panic: None };
        let job_ptr = JobPtr(NonNull::from(&mut job));

        let shared = self.shared();
        let mut state = shared.lock();
        state.queue.push_back(job_ptr);
        for _ in 0..num_threads {
            shared.job_available.notify_one();
        }
        // SAFETY: the job's counters are read and written only while the state lock is held.
        while unsafe { job_ptr.0.as_ref() }.unfinished > 0 {
            state = shared.job_finished.wait(state).unwrap_or_else(|err| err.into_inner());
        }
        drop(state);

        if let Some(panic) = job.panic.take() {
            resume_unwind(panic)
        }
    }
}

impl Drop for ParkedThreadPool {
    fn drop(&mut self) {
        if let Some(shared) = self.shared.get() {
            shared.shut_down();
        }
    }
}

/// State shared by the pool handle and its threads.
#[derive(Debug)]
struct Shared {
    state: Mutex<State>,
    /// Signalled once per thread a new broadcast needs, and on shutdown.
    job_available: Condvar,
    /// Signalled when a broadcast's last run finishes.
    job_finished: Condvar,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        // Jobs run outside the lock, so nothing panics while holding it.
        self.state.lock().unwrap_or_else(|err| err.into_inner())
    }

    /// Makes the threads exit once the queue is empty.
    fn shut_down(&self) {
        self.lock().shutdown = true;
        self.job_available.notify_all();
    }

    /// Runs broadcast jobs until the pool is dropped.
    fn run_thread(&self) {
        let mut state = self.lock();
        loop {
            let Some(&job_ptr) = state.queue.front() else {
                if state.shutdown {
                    return
                }
                state = self.job_available.wait(state).unwrap_or_else(|err| err.into_inner());
                continue
            };

            // SAFETY: a queued job stays alive until all its runs are finished, which includes
            // this one, and its counters are touched only under the state lock.
            let job = unsafe { &mut *job_ptr.0.as_ptr() };
            job.unclaimed -= 1;
            if job.unclaimed == 0 {
                state.queue.pop_front();
            }
            let f = job.f;
            drop(state);

            let result = catch_unwind(AssertUnwindSafe(f));

            state = self.lock();
            // SAFETY: as above; the broadcast waits for this run to finish before it returns.
            let job = unsafe { &mut *job_ptr.0.as_ptr() };
            let mut extra_panic = None;
            if let Err(panic) = result {
                if job.panic.is_none() {
                    job.panic = Some(panic);
                } else {
                    extra_panic = Some(panic);
                }
            }
            job.unfinished -= 1;
            if job.unfinished == 0 {
                // The job may be freed once the lock is released, so it is not touched again.
                self.job_finished.notify_all();
            }

            if let Some(panic) = extra_panic {
                // Only the first panic is resumed. Dropping a payload runs arbitrary code that
                // may panic itself, so it happens outside the lock, and a payload whose drop
                // panics is leaked, as std does.
                drop(state);
                if let Err(payload) = catch_unwind(AssertUnwindSafe(|| drop(panic))) {
                    std::mem::forget(payload);
                }
                state = self.lock();
            }
        }
    }
}

/// Broadcasts waiting for threads to claim them, oldest first.
#[derive(Debug)]
struct State {
    queue: VecDeque<JobPtr>,
    shutdown: bool,
}

/// A broadcast in progress, living on the stack of the thread that made it.
struct Job {
    f: &'static (dyn Fn() + Sync),
    /// Runs no thread has started yet.
    unclaimed: usize,
    /// Runs that have not finished yet.
    unfinished: usize,
    /// The first panic of a run.
    panic: Option<Box<dyn Any + Send>>,
}

/// Pointer to a [`Job`] on a broadcasting thread's stack.
#[derive(Clone, Copy, Debug)]
struct JobPtr(NonNull<Job>);

// SAFETY: the job is accessed only under the state lock, and `Job::f` is `Sync`.
unsafe impl Send for JobPtr {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        collections::HashSet,
        sync::{
            atomic::{AtomicUsize, Ordering},
            Barrier,
        },
        time::Duration,
    };

    #[test]
    fn broadcast_runs_on_requested_threads() {
        let pool = ParkedThreadPool::new(4, "parked-test");
        assert!(!pool.is_initialized());

        let names = Mutex::new(HashSet::new());
        let barrier = Barrier::new(3);
        pool.broadcast(3, || {
            // Every run waits for the others, so they must be on three distinct threads.
            barrier.wait();
            names.lock().unwrap().insert(thread::current().name().unwrap().to_string());
        });
        assert!(pool.is_initialized());
        assert_eq!(names.into_inner().unwrap().len(), 3);
    }

    #[test]
    fn broadcast_survives_second_panic() {
        /// A panic payload whose drop panics.
        struct PanicOnDrop;

        impl Drop for PanicOnDrop {
            fn drop(&mut self) {
                panic!("payload drop");
            }
        }

        let pool = ParkedThreadPool::new(2, "parked-double");
        let barrier = Barrier::new(2);
        let result = catch_unwind(AssertUnwindSafe(|| {
            pool.broadcast(2, || {
                // Both runs panic, so one payload is dropped by the pool.
                barrier.wait();
                std::panic::panic_any(PanicOnDrop);
            })
        }));
        std::mem::forget(result.unwrap_err());

        let runs = AtomicUsize::new(0);
        pool.broadcast(2, || {
            runs.fetch_add(1, Ordering::Relaxed);
        });
        assert_eq!(runs.into_inner(), 2);
    }

    #[test]
    fn broadcast_caps_at_pool_size() {
        let pool = ParkedThreadPool::new(2, "parked-cap");
        let runs = AtomicUsize::new(0);
        pool.broadcast(8, || {
            runs.fetch_add(1, Ordering::Relaxed);
        });
        assert_eq!(runs.into_inner(), 2);

        pool.broadcast(0, || panic!("no runs expected"));
    }

    #[test]
    fn concurrent_broadcasts_share_threads() {
        let pool = Arc::new(ParkedThreadPool::new(2, "parked-concurrent"));
        let runs = Arc::new(AtomicUsize::new(0));
        let handles = (0..4)
            .map(|_| {
                let pool = Arc::clone(&pool);
                let runs = Arc::clone(&runs);
                thread::spawn(move || {
                    pool.broadcast(2, || {
                        thread::sleep(Duration::from_millis(5));
                        runs.fetch_add(1, Ordering::Relaxed);
                    })
                })
            })
            .collect::<Vec<_>>();
        for handle in handles {
            handle.join().unwrap();
        }
        assert_eq!(runs.load(Ordering::Relaxed), 8);
    }

    #[test]
    fn broadcast_resumes_panic_after_all_runs() {
        let pool = ParkedThreadPool::new(3, "parked-panic");
        let finished = AtomicUsize::new(0);
        let started = AtomicUsize::new(0);
        let result = catch_unwind(AssertUnwindSafe(|| {
            pool.broadcast(3, || {
                assert_ne!(started.fetch_add(1, Ordering::Relaxed), 0, "worker panic");
                thread::sleep(Duration::from_millis(5));
                finished.fetch_add(1, Ordering::Relaxed);
            })
        }));
        let panic = result.unwrap_err();
        assert!(panic.downcast_ref::<String>().unwrap().contains("worker panic"));
        assert_eq!(finished.load(Ordering::Relaxed), 2);

        // The thread that panicked keeps serving broadcasts: three runs that wait for each other
        // need all three threads.
        let names = Mutex::new(HashSet::new());
        let barrier = Barrier::new(3);
        pool.broadcast(3, || {
            barrier.wait();
            names.lock().unwrap().insert(thread::current().name().unwrap().to_string());
        });
        assert_eq!(names.into_inner().unwrap().len(), 3);
    }
}
