//! A pool of persistent OS threads for long-running blocking jobs.

use parking_lot::{Condvar, Mutex};
use std::{
    collections::VecDeque,
    panic::{catch_unwind, AssertUnwindSafe},
    sync::Arc,
    thread::{self, JoinHandle},
};

type Job = Box<dyn FnOnce() + Send + 'static>;

/// A pool of persistent OS threads that run long-running blocking jobs, such as proof workers
/// that block on a channel for the lifetime of a block.
///
/// Threads are spawned on demand, up to [`num_threads`](Self::num_threads), and kept for reuse.
/// An idle thread parks on a condition variable.
///
/// Unlike a rayon pool, its threads are not work-stealing workers. An idle rayon worker spins
/// through steal attempts on every other worker of its pool before it sleeps, and each attempt
/// pins the global `crossbeam-epoch` collector that rayon's deques share. Every 128 pins a
/// thread walks the collector's list of every thread that ever pinned, so large rayon pools
/// make every rayon thread in the process slower. Threads in this pool never pin the collector
/// and never spin.
///
/// Dropping the pool waits for every queued and running job to finish.
#[derive(Debug)]
pub struct DedicatedThreadPool {
    shared: Arc<Shared>,
    handles: Mutex<Vec<JoinHandle<()>>>,
    num_threads: usize,
    thread_name_prefix: &'static str,
}

#[derive(Debug, Default)]
struct Shared {
    state: Mutex<State>,
    job_available: Condvar,
}

#[derive(Default)]
struct State {
    /// Jobs waiting for a thread.
    queue: VecDeque<Job>,
    /// Threads parked waiting for a job.
    idle: usize,
    /// Threads spawned so far.
    spawned: usize,
    /// Set when the pool is dropped: threads exit once the queue is empty.
    shutdown: bool,
}

impl std::fmt::Debug for State {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("State")
            .field("queued", &self.queue.len())
            .field("idle", &self.idle)
            .field("spawned", &self.spawned)
            .field("shutdown", &self.shutdown)
            .finish()
    }
}

impl DedicatedThreadPool {
    /// Creates a pool of at most `num_threads` threads (at least one), named
    /// `"{prefix}-{index:02}"`. No thread is spawned until the first job.
    pub fn new(num_threads: usize, thread_name_prefix: &'static str) -> Self {
        Self {
            shared: Arc::default(),
            handles: Mutex::default(),
            num_threads: num_threads.max(1),
            thread_name_prefix,
        }
    }

    /// Returns the maximum number of threads of the pool.
    pub const fn num_threads(&self) -> usize {
        self.num_threads
    }

    /// Returns the number of threads spawned so far.
    pub fn spawned_threads(&self) -> usize {
        self.shared.state.lock().spawned
    }

    /// Runs `f` on a thread of the pool, spawning a new thread when none is idle and the pool
    /// has fewer than [`num_threads`](Self::num_threads) threads. Otherwise `f` waits for the
    /// next thread to finish its job.
    ///
    /// A panic in `f` is caught: the thread survives and runs the next job.
    pub fn spawn(&self, f: impl FnOnce() + Send + 'static) {
        let mut state = self.shared.state.lock();
        state.queue.push_back(Box::new(f));

        // Every parked thread takes one queued job, so spawn when jobs outnumber them.
        if state.queue.len() > state.idle && state.spawned < self.num_threads {
            let index = state.spawned;
            state.spawned += 1;
            drop(state);
            self.spawn_thread(index);
        } else {
            drop(state);
            self.shared.job_available.notify_one();
        }
    }

    fn spawn_thread(&self, index: usize) {
        let shared = Arc::clone(&self.shared);
        let handle = thread::Builder::new()
            .name(format!("{}-{index:02}", self.thread_name_prefix))
            .spawn(move || {
                crate::utils::decrease_thread_priority();
                shared.run()
            })
            .unwrap_or_else(|err| {
                panic!("failed to spawn {} thread: {err}", self.thread_name_prefix)
            });
        self.handles.lock().push(handle);
    }
}

impl Shared {
    fn run(&self) {
        loop {
            let job = {
                let mut state = self.state.lock();
                loop {
                    if let Some(job) = state.queue.pop_front() {
                        break job
                    }
                    if state.shutdown {
                        return
                    }
                    state.idle += 1;
                    self.job_available.wait(&mut state);
                    state.idle -= 1;
                }
            };
            // The default panic hook already reports the panic.
            let _ = catch_unwind(AssertUnwindSafe(job));
        }
    }
}

impl Drop for DedicatedThreadPool {
    fn drop(&mut self) {
        self.shared.state.lock().shutdown = true;
        self.shared.job_available.notify_all();
        for handle in self.handles.get_mut().drain(..) {
            // A job can own the last reference to the pool and drop it on a pool thread, which
            // cannot join itself. That thread exits on its own once the job returns.
            if handle.thread().id() != thread::current().id() {
                let _ = handle.join();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        sync::{mpsc, Barrier},
        time::Duration,
    };

    #[test]
    fn runs_concurrent_long_running_jobs_on_distinct_threads() {
        let pool = DedicatedThreadPool::new(4, "dedicated");
        assert_eq!(pool.spawned_threads(), 0);

        // Every job waits for all others, so this only finishes if each runs on its own thread.
        let barrier = Arc::new(Barrier::new(4));
        let (tx, rx) = mpsc::channel();
        for _ in 0..4 {
            let (barrier, tx) = (barrier.clone(), tx.clone());
            pool.spawn(move || {
                barrier.wait();
                tx.send(std::thread::current().name().unwrap().to_string()).unwrap();
            });
        }
        let mut names: Vec<_> =
            (0..4).map(|_| rx.recv_timeout(Duration::from_secs(10)).unwrap()).collect();
        names.sort();
        assert_eq!(names, ["dedicated-00", "dedicated-01", "dedicated-02", "dedicated-03"]);
        assert_eq!(pool.spawned_threads(), 4);
    }

    #[test]
    fn reuses_idle_threads() {
        let pool = DedicatedThreadPool::new(8, "reuse");
        let (tx, rx) = mpsc::channel();
        for _ in 0..16 {
            let tx = tx.clone();
            pool.spawn(move || tx.send(()).unwrap());
            rx.recv_timeout(Duration::from_secs(10)).unwrap();
            // Wait for the thread to park again before the next job.
            while pool.shared.state.lock().idle == 0 {
                std::thread::yield_now();
            }
        }
        assert_eq!(pool.spawned_threads(), 1);
    }

    #[test]
    fn queues_jobs_beyond_max_threads() {
        let pool = DedicatedThreadPool::new(2, "capped");
        let (tx, rx) = mpsc::channel();
        for i in 0..10 {
            let tx = tx.clone();
            pool.spawn(move || {
                std::thread::sleep(Duration::from_millis(1));
                tx.send(i).unwrap();
            });
        }
        let mut done: Vec<_> =
            (0..10).map(|_| rx.recv_timeout(Duration::from_secs(10)).unwrap()).collect();
        done.sort_unstable();
        assert_eq!(done, (0..10).collect::<Vec<_>>());
        assert!(pool.spawned_threads() <= 2);
    }

    #[test]
    fn drop_waits_for_queued_and_running_jobs() {
        let pool = DedicatedThreadPool::new(1, "drain");
        let done = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        for _ in 0..3 {
            let done = done.clone();
            pool.spawn(move || {
                std::thread::sleep(Duration::from_millis(5));
                done.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            });
        }
        drop(pool);
        assert_eq!(done.load(std::sync::atomic::Ordering::Relaxed), 3);
    }

    #[test]
    fn survives_panicking_job() {
        let pool = DedicatedThreadPool::new(1, "panicky");
        pool.spawn(|| panic!("job panicked"));
        let (tx, rx) = mpsc::channel();
        pool.spawn(move || tx.send(()).unwrap());
        rx.recv_timeout(Duration::from_secs(10)).unwrap();
        assert_eq!(pool.spawned_threads(), 1);
    }
}
