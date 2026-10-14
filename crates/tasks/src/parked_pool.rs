//! A pool of dedicated OS threads for long-running blocking jobs.
//!
//! Each job owns one thread until it returns. Dispatching a job wakes exactly one parked idle
//! thread, and threads are created lazily up to a maximum, so a pool sized for peak demand only
//! creates as many threads as jobs ever run at once.

use crate::{metrics::WorkerPoolMetrics, pool::RecordWorkerPoolJobDurationOnDrop};
use parking_lot::Mutex;
use std::{
    collections::VecDeque,
    fmt,
    panic::{catch_unwind, AssertUnwindSafe},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, OnceLock,
    },
    thread::{self, JoinHandle, Thread},
    time::Instant,
};
use tracing::error;

/// A pool of dedicated OS threads for long-running blocking jobs.
///
/// Every job owns one thread until it returns, and dispatching a job wakes exactly one parked
/// thread. Threads are created on demand, up to [`max_threads`](Self::max_threads), and park
/// while idle. They never exit on their own, only once the pool is dropped.
///
/// [`spawn`](Self::spawn) never waits for other jobs. If every thread is busy and the pool is at
/// capacity, the job is queued and starts once a thread finishes its current job, so jobs that
/// wait for each other must run on separate pools.
///
/// Dropping the pool discards queued jobs and blocks until running jobs return and every thread
/// has exited.
pub struct ParkedPool {
    /// State shared with the pool's threads.
    shared: Arc<Shared>,
    /// Join handles of the pool's threads, joined when the pool is dropped.
    handles: Mutex<Vec<JoinHandle<()>>>,
    /// Metrics for jobs spawned on this pool, created on first use.
    metrics: OnceLock<WorkerPoolMetrics>,
    /// Maximum number of threads the pool creates.
    max_threads: usize,
    /// Prefix of the pool's thread names.
    thread_name_prefix: &'static str,
}

impl ParkedPool {
    /// Creates a pool that runs jobs on at most `max_threads` threads, named
    /// `"{thread_name_prefix}-{index:02}"`.
    ///
    /// No thread is created until a job needs one. A `max_threads` of zero is raised to one, so
    /// jobs can always run. Linux truncates thread names to 15 bytes, so keep the prefix short.
    pub fn new(max_threads: usize, thread_name_prefix: &'static str) -> Self {
        Self {
            shared: Arc::new(Shared {
                state: Mutex::new(State { idle: Vec::new(), pending: VecDeque::new(), threads: 0 }),
                shutdown: AtomicBool::new(false),
            }),
            handles: Mutex::new(Vec::new()),
            metrics: OnceLock::new(),
            max_threads: max_threads.max(1),
            thread_name_prefix,
        }
    }

    /// Returns the maximum number of threads the pool creates.
    pub const fn max_threads(&self) -> usize {
        self.max_threads
    }

    /// Returns the number of threads the pool has created so far.
    pub fn spawned_threads(&self) -> usize {
        self.shared.state.lock().threads
    }

    /// Spawns a job on the pool without waiting for other jobs to finish.
    ///
    /// The job is handed to the most recently parked idle thread, which is the only thread woken.
    /// If no thread is idle, a new thread runs the job while fewer than
    /// [`max_threads`](Self::max_threads) exist. Otherwise the job is queued and runs, in spawn
    /// order, once a thread finishes its current job.
    ///
    /// A panicking job is logged and does not affect the thread or the pool.
    ///
    /// # Panics
    ///
    /// Panics if a new thread is needed and the OS fails to create it.
    pub fn spawn(&self, job: impl FnOnce() + Send + 'static) {
        let metrics = self.metrics().clone();
        let queued_at = Instant::now();
        let job = Box::new(move || {
            let started_at = Instant::now();
            metrics.record_job_queue_wait(started_at.saturating_duration_since(queued_at));
            let _record_job_duration = RecordWorkerPoolJobDurationOnDrop::new(metrics, started_at);
            job();
        });

        let mut state = self.shared.state.lock();
        if let Some(slot) = state.idle.pop() {
            debug_assert!(state.pending.is_empty(), "jobs are queued while a thread is idle");
            drop(state);
            *slot.job.lock() = Some(job);
            slot.thread.unpark();
        } else if state.threads < self.max_threads {
            let index = state.threads;
            state.threads += 1;
            drop(state);
            self.spawn_thread(index, job);
        } else {
            state.pending.push_back(job);
        }
    }

    /// Returns metrics for this pool.
    fn metrics(&self) -> &WorkerPoolMetrics {
        self.metrics.get_or_init(|| WorkerPoolMetrics::new(self.thread_name_prefix))
    }

    /// Creates the pool thread with the given index, which runs `job` first.
    fn spawn_thread(&self, index: usize, job: Job) {
        let prefix = self.thread_name_prefix;
        let shared = Arc::clone(&self.shared);
        let handle = thread::Builder::new()
            .name(format!("{prefix}-{index:02}"))
            .spawn(move || shared.run(job))
            .unwrap_or_else(|err| {
                // Release the reserved thread, so that a caught panic does not leave the pool
                // counting a thread that never serves queued jobs.
                self.shared.state.lock().threads -= 1;
                panic!("failed to spawn {prefix} pool thread: {err}")
            });
        self.handles.lock().push(handle);
    }
}

impl Drop for ParkedPool {
    /// Discards queued jobs, then blocks until running jobs return and every thread has exited.
    fn drop(&mut self) {
        let (idle, pending) = {
            let mut state = self.shared.state.lock();
            // Setting the flag under the lock means a thread finishing its job afterwards observes
            // it, while a thread that went idle before is taken from the idle stack and unparked.
            self.shared.shutdown.store(true, Ordering::Release);
            (std::mem::take(&mut state.idle), std::mem::take(&mut state.pending))
        };
        for slot in idle {
            slot.thread.unpark();
        }
        // Queued jobs never start.
        drop(pending);

        // A job can drop the last reference to its own pool. Joining that job's thread from
        // inside the job would deadlock, so it is skipped and exits once the job returns.
        let current = thread::current().id();
        for handle in std::mem::take(self.handles.get_mut()) {
            if handle.thread().id() != current {
                let _ = handle.join();
            }
        }
    }
}

impl fmt::Debug for ParkedPool {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ParkedPool")
            .field("max_threads", &self.max_threads)
            .field("spawned_threads", &self.spawned_threads())
            .field("thread_name_prefix", &self.thread_name_prefix)
            .finish_non_exhaustive()
    }
}

/// A job spawned on a [`ParkedPool`].
type Job = Box<dyn FnOnce() + Send + 'static>;

/// State shared between a [`ParkedPool`] and its threads.
struct Shared {
    /// Idle threads, queued jobs, and the thread count.
    state: Mutex<State>,
    /// Set once the pool is dropped, telling its threads to exit instead of parking.
    shutdown: AtomicBool,
}

impl Shared {
    /// Runs `job`, then every job dispatched to the current thread, until the pool is dropped.
    fn run(&self, mut job: Job) {
        let slot = Arc::new(Slot { job: Mutex::new(None), thread: thread::current() });
        loop {
            // A panicking job must neither kill the thread nor shrink the pool.
            if let Err(panic) = catch_unwind(AssertUnwindSafe(job)) {
                let message = panic
                    .downcast_ref::<&str>()
                    .copied()
                    .or_else(|| panic.downcast_ref::<String>().map(String::as_str));
                error!(thread = slot.thread.name(), panic = message, "Parked pool job panicked");
            }
            let Some(next) = self.next_job(&slot) else { return };
            job = next;
        }
    }

    /// Returns the next job for the thread that owns `slot`, or `None` once the pool is dropped.
    ///
    /// Queued jobs are taken before the thread parks, so no job waits in the queue while a thread
    /// is idle.
    fn next_job(&self, slot: &Arc<Slot>) -> Option<Job> {
        {
            let mut state = self.state.lock();
            if self.shutdown.load(Ordering::Acquire) {
                return None
            }
            if let Some(job) = state.pending.pop_front() {
                return Some(job)
            }
            state.idle.push(Arc::clone(slot));
        }

        // `ParkedPool::spawn` pops the slot from the idle stack and stores the job before it
        // unparks the thread. An unpark that lands before `park` leaves a token that makes `park`
        // return immediately, so the hand-off is never missed, and the loop re-checks the slot
        // after spurious wakeups.
        loop {
            if let Some(job) = slot.job.lock().take() {
                return Some(job)
            }
            if self.shutdown.load(Ordering::Acquire) {
                return None
            }
            thread::park();
        }
    }
}

/// Bookkeeping of a [`ParkedPool`], guarded by [`Shared::state`].
struct State {
    /// Threads waiting for a job, the most recently parked last.
    idle: Vec<Arc<Slot>>,
    /// Jobs waiting for a thread, oldest first. Only non-empty while no thread is idle.
    pending: VecDeque<Job>,
    /// Number of threads created so far.
    threads: usize,
}

/// Hand-off point between [`ParkedPool::spawn`] and one pool thread.
struct Slot {
    /// Job handed to the thread while it is idle.
    job: Mutex<Option<Job>>,
    /// The thread, unparked once a job is handed to it.
    thread: Thread,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        collections::{HashMap, HashSet},
        sync::mpsc,
        time::Duration,
    };

    /// Upper bound for every wait in these tests.
    const TIMEOUT: Duration = Duration::from_secs(10);

    /// Spawns a job that reports `(id, thread)` once it starts, then blocks until the returned
    /// sender is dropped.
    fn spawn_blocking_job(
        pool: &ParkedPool,
        id: usize,
        started: &mpsc::Sender<(usize, Thread)>,
    ) -> mpsc::Sender<()> {
        let (release_tx, release_rx) = mpsc::channel();
        let started = started.clone();
        pool.spawn(move || {
            let _ = started.send((id, thread::current()));
            let _ = release_rx.recv();
        });
        release_tx
    }

    /// Receives the next `count` start reports, keyed by job id.
    fn recv_started(
        started: &mpsc::Receiver<(usize, Thread)>,
        count: usize,
    ) -> HashMap<usize, Thread> {
        (0..count).map(|_| started.recv_timeout(TIMEOUT).unwrap()).collect()
    }

    /// Waits until `count` threads of `pool` are on its idle stack.
    fn wait_for_idle_threads(pool: &ParkedPool, count: usize) {
        let deadline = Instant::now() + TIMEOUT;
        while pool.shared.state.lock().idle.len() < count {
            assert!(Instant::now() < deadline, "threads did not become idle");
            thread::yield_now();
        }
    }

    #[test]
    fn creates_threads_lazily() {
        let pool = ParkedPool::new(8, "lazy");
        assert_eq!(pool.spawned_threads(), 0);

        let (started_tx, started_rx) = mpsc::channel();
        let _release =
            [spawn_blocking_job(&pool, 0, &started_tx), spawn_blocking_job(&pool, 1, &started_tx)];

        let started = recv_started(&started_rx, 2);
        let names = started.values().map(|thread| thread.name().unwrap()).collect::<HashSet<_>>();
        assert_eq!(names, HashSet::from(["lazy-00", "lazy-01"]));
        assert_eq!(pool.spawned_threads(), 2);
    }

    #[test]
    fn reuses_idle_threads() {
        let pool = ParkedPool::new(8, "reuse");
        let (started_tx, started_rx) = mpsc::channel();

        let release =
            [spawn_blocking_job(&pool, 0, &started_tx), spawn_blocking_job(&pool, 1, &started_tx)];
        let first = recv_started(&started_rx, 2).values().map(Thread::id).collect::<HashSet<_>>();
        drop(release);
        wait_for_idle_threads(&pool, 2);

        let _release =
            [spawn_blocking_job(&pool, 2, &started_tx), spawn_blocking_job(&pool, 3, &started_tx)];
        let second = recv_started(&started_rx, 2).values().map(Thread::id).collect::<HashSet<_>>();
        assert_eq!(second, first);
        assert_eq!(pool.spawned_threads(), 2);
    }

    #[test]
    fn queues_jobs_beyond_max_threads() {
        let pool = ParkedPool::new(2, "queue");
        let (started_tx, started_rx) = mpsc::channel();

        let release_first = spawn_blocking_job(&pool, 0, &started_tx);
        let _release_second = spawn_blocking_job(&pool, 1, &started_tx);
        let _release_third = spawn_blocking_job(&pool, 2, &started_tx);
        // Both threads are busy until a job is released, so the third job waits in the queue.
        assert_eq!(pool.shared.state.lock().pending.len(), 1);

        let started = recv_started(&started_rx, 2);
        assert!(started.contains_key(&0) && started.contains_key(&1));

        drop(release_first);
        let third = recv_started(&started_rx, 1);
        assert_eq!(third[&2].id(), started[&0].id());
        assert_eq!(pool.spawned_threads(), 2);
    }

    #[test]
    fn panicking_job_keeps_capacity() {
        let pool = ParkedPool::new(1, "panic");
        let (thread_tx, thread_rx) = mpsc::channel();

        let panicking_tx = thread_tx.clone();
        pool.spawn(move || {
            let _ = panicking_tx.send(thread::current().id());
            panic!("parked pool test panic");
        });
        pool.spawn(move || {
            let _ = thread_tx.send(thread::current().id());
        });

        let panicked_on = thread_rx.recv_timeout(TIMEOUT).unwrap();
        assert_eq!(thread_rx.recv_timeout(TIMEOUT).unwrap(), panicked_on);
        assert_eq!(pool.spawned_threads(), 1);
    }

    #[test]
    fn zero_max_threads_runs_jobs() {
        let pool = ParkedPool::new(0, "zero");
        assert_eq!(pool.max_threads(), 1);

        let (done_tx, done_rx) = mpsc::channel();
        pool.spawn(move || {
            let _ = done_tx.send(());
        });
        done_rx.recv_timeout(TIMEOUT).unwrap();
    }

    #[test]
    fn drop_with_idle_threads_does_not_hang() {
        let pool = ParkedPool::new(4, "idle");
        let (started_tx, _started_rx) = mpsc::channel();

        // Both jobs block until released, so each gets its own thread.
        let release =
            [spawn_blocking_job(&pool, 0, &started_tx), spawn_blocking_job(&pool, 1, &started_tx)];
        drop(release);
        wait_for_idle_threads(&pool, 2);

        let shared = Arc::downgrade(&pool.shared);
        drop(pool);
        // Dropping joins the threads, which release the shared state when they exit.
        assert_eq!(shared.strong_count(), 0);
    }

    #[test]
    fn drop_discards_queued_jobs_and_waits_for_busy_threads() {
        let pool = ParkedPool::new(1, "busy");
        let (events_tx, events_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel::<()>();
        let job_events = events_tx.clone();
        pool.spawn(move || {
            let _ = job_events.send("job started");
            let _ = release_rx.recv();
            let _ = job_events.send("job returned");
        });
        assert_eq!(events_rx.recv_timeout(TIMEOUT), Ok("job started"));

        let (ran_tx, ran_rx) = mpsc::channel();
        pool.spawn(move || {
            let _ = ran_tx.send(());
        });
        assert_eq!(pool.shared.state.lock().pending.len(), 1);

        // Dropping blocks until the busy job returns, so drop the pool on another thread.
        let shared = Arc::downgrade(&pool.shared);
        let dropper = thread::spawn(move || {
            drop(pool);
            let _ = events_tx.send("pool dropped");
        });
        // The queued job cannot run while the only thread is busy, so its channel closes once the
        // drop discards it. Only then is the busy job released.
        assert_eq!(ran_rx.recv_timeout(TIMEOUT), Err(mpsc::RecvTimeoutError::Disconnected));
        drop(release_tx);
        assert_eq!(events_rx.recv_timeout(TIMEOUT), Ok("job returned"));
        assert_eq!(events_rx.recv_timeout(TIMEOUT), Ok("pool dropped"));
        dropper.join().unwrap();
        assert_eq!(shared.strong_count(), 0);
    }

    #[test]
    fn job_can_drop_its_own_pool() {
        let pool = Arc::new(ParkedPool::new(2, "self"));
        let (dropped_tx, dropped_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel::<()>();
        let job_pool = Arc::clone(&pool);
        pool.spawn(move || {
            let _ = release_rx.recv();
            // This is the last reference, so the pool is dropped on one of its own threads.
            drop(job_pool);
            let _ = dropped_tx.send(());
        });
        // A second thread is idle when the pool is dropped, and gets joined.
        pool.spawn(|| {});
        wait_for_idle_threads(&pool, 1);

        drop(pool);
        drop(release_tx);
        dropped_rx.recv_timeout(TIMEOUT).unwrap();
    }
}
