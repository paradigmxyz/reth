//! Additional helpers for executing tracing calls

use crate::metrics::WorkerPoolMetrics;
use std::{
    any::Any,
    cell::RefCell,
    future::Future,
    panic::{catch_unwind, AssertUnwindSafe},
    pin::Pin,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, OnceLock,
    },
    task::{ready, Context, Poll},
    thread,
    time::Instant,
};
use tokio::sync::{oneshot, AcquireError, OwnedSemaphorePermit, Semaphore};

/// RPC Tracing call guard semaphore.
///
/// This is used to restrict the number of concurrent RPC requests to tracing methods like
/// `debug_traceTransaction` as well as `eth_getProof` because they can consume a lot of
/// memory and CPU.
///
/// This types serves as an entry guard for the [`BlockingTaskPool`] and is used to rate limit
/// parallel blocking tasks in the pool.
#[derive(Clone, Debug)]
pub struct BlockingTaskGuard(Arc<Semaphore>);

impl BlockingTaskGuard {
    /// Create a new `BlockingTaskGuard` with the given maximum number of blocking tasks in
    /// parallel.
    pub fn new(max_blocking_tasks: usize) -> Self {
        Self(Arc::new(Semaphore::new(max_blocking_tasks)))
    }

    /// See also [`Semaphore::acquire_owned`]
    pub async fn acquire_owned(self) -> Result<OwnedSemaphorePermit, AcquireError> {
        self.0.acquire_owned().await
    }

    /// See also [`Semaphore::acquire_many_owned`]
    pub async fn acquire_many_owned(self, n: u32) -> Result<OwnedSemaphorePermit, AcquireError> {
        self.0.acquire_many_owned(n).await
    }
}

/// Used to execute blocking tasks on a rayon threadpool from within a tokio runtime.
///
/// This is a dedicated threadpool for blocking tasks which are CPU bound.
/// RPC calls that perform blocking IO (disk lookups) are not executed on this pool but on the tokio
/// runtime's blocking pool, which performs poorly with CPU bound tasks (see
/// <https://ryhl.io/blog/async-what-is-blocking/>). Once the tokio blocking
/// pool is saturated it is converted into a queue, blocking tasks could then interfere with the
/// queue and block other RPC calls.
///
/// See also [tokio-docs] for more information.
///
/// [tokio-docs]: https://docs.rs/tokio/latest/tokio/index.html#cpu-bound-tasks-and-blocking-code
#[derive(Clone, Debug)]
pub struct BlockingTaskPool {
    pool: Arc<rayon::ThreadPool>,
}

impl BlockingTaskPool {
    /// Create a new `BlockingTaskPool` with the given threadpool.
    pub fn new(pool: rayon::ThreadPool) -> Self {
        Self { pool: Arc::new(pool) }
    }

    /// Convenience function to start building a new threadpool.
    pub fn builder() -> rayon::ThreadPoolBuilder {
        rayon::ThreadPoolBuilder::new()
    }

    /// Convenience function to build a new threadpool with the default configuration.
    ///
    /// Uses [`rayon::ThreadPoolBuilder::build`](rayon::ThreadPoolBuilder::build) defaults.
    /// If a different stack size or other parameters are needed, they can be configured via
    /// [`rayon::ThreadPoolBuilder`] returned by [`Self::builder`].
    pub fn build() -> Result<Self, rayon::ThreadPoolBuildError> {
        Self::builder().build().map(Self::new)
    }

    /// Asynchronous wrapper around Rayon's
    /// [`ThreadPool::spawn`](rayon::ThreadPool::spawn).
    ///
    /// Runs a function on the configured threadpool, returning a future that resolves with the
    /// function's return value.
    ///
    /// If the function panics, the future will resolve to an error.
    pub fn spawn<F, R>(&self, func: F) -> BlockingTaskHandle<R>
    where
        F: FnOnce() -> R + Send + 'static,
        R: Send + 'static,
    {
        let (tx, rx) = oneshot::channel();

        self.pool.spawn(move || {
            let _result = tx.send(catch_unwind(AssertUnwindSafe(func)));
        });

        BlockingTaskHandle { rx }
    }

    /// Asynchronous wrapper around Rayon's
    /// [`ThreadPool::spawn_fifo`](rayon::ThreadPool::spawn_fifo).
    ///
    /// Runs a function on the configured threadpool, returning a future that resolves with the
    /// function's return value.
    ///
    /// If the function panics, the future will resolve to an error.
    pub fn spawn_fifo<F, R>(&self, func: F) -> BlockingTaskHandle<R>
    where
        F: FnOnce() -> R + Send + 'static,
        R: Send + 'static,
    {
        let (tx, rx) = oneshot::channel();

        self.pool.spawn_fifo(move || {
            let _result = tx.send(catch_unwind(AssertUnwindSafe(func)));
        });

        BlockingTaskHandle { rx }
    }
}

/// Async handle for a blocking task running in a Rayon thread pool.
///
/// ## Panics
///
/// If polled from outside a tokio runtime.
#[derive(Debug)]
#[must_use = "futures do nothing unless you `.await` or poll them"]
#[pin_project::pin_project]
pub struct BlockingTaskHandle<T> {
    #[pin]
    pub(crate) rx: oneshot::Receiver<thread::Result<T>>,
}

impl<T> Future for BlockingTaskHandle<T> {
    type Output = thread::Result<T>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match ready!(self.project().rx.poll(cx)) {
            Ok(res) => Poll::Ready(res),
            Err(_) => Poll::Ready(Err(Box::<TokioBlockingTaskError>::default())),
        }
    }
}

/// An error returned when the Tokio channel is dropped while awaiting a result.
///
/// This should only happen
#[derive(Debug, Default, thiserror::Error)]
#[error("tokio channel dropped while awaiting result")]
#[non_exhaustive]
pub struct TokioBlockingTaskError;

thread_local! {
    static WORKER: RefCell<Worker> = const { RefCell::new(Worker::new()) };
}

/// A rayon thread pool with per-thread [`Worker`] state.
///
/// Each thread in the pool has its own [`Worker`] that can hold arbitrary state via
/// [`WorkerState::init`]. Access state through its owner from jobs executing on the pool,
/// for example inside [`install_fn`](Self::install_fn).
///
/// Worker access is backed by a thread-local [`RefCell`]. Keep [`with_worker`](Self::with_worker)
/// and [`with_worker_mut`](Self::with_worker_mut) closures short and non-yielding: if Rayon runs
/// another job on the same thread while a borrow is active, re-entrant worker access can panic.
///
/// Independent [`WorkerState`] owners can reuse the same threads with different state
/// configurations; each owner clears its state when dropped.
///
/// The underlying rayon pool is created lazily on first access.
#[derive(Debug)]
pub struct WorkerPool {
    pool: OnceLock<rayon::ThreadPool>,
    metrics: OnceLock<WorkerPoolMetrics>,
    num_threads: usize,
    thread_name_prefix: &'static str,
}

impl WorkerPool {
    /// Creates a new lazy `WorkerPool` with the given number of threads and a thread name prefix.
    ///
    /// The underlying rayon pool is not created until the first method that requires it is called.
    /// Thread names follow the pattern `"{prefix}-{index:02}"`.
    pub const fn new(num_threads: usize, thread_name_prefix: &'static str) -> Self {
        Self { pool: OnceLock::new(), metrics: OnceLock::new(), num_threads, thread_name_prefix }
    }

    /// Returns a reference to the underlying rayon pool, creating it on first access.
    fn pool(&self) -> &rayon::ThreadPool {
        self.pool.get_or_init(|| {
            let prefix = self.thread_name_prefix;
            build_pool_with_panic_handler(
                rayon::ThreadPoolBuilder::new()
                    .num_threads(self.num_threads)
                    .thread_name(move |i| format!("{prefix}-{i:02}")),
            )
            .unwrap_or_else(|err| panic!("failed to build {prefix} worker pool: {err}"))
        })
    }

    /// Returns metrics for this worker pool.
    fn metrics(&self) -> &WorkerPoolMetrics {
        self.metrics.get_or_init(|| WorkerPoolMetrics::new(self.thread_name_prefix))
    }

    /// Returns `true` if the underlying rayon pool has been initialized.
    pub fn is_initialized(&self) -> bool {
        self.pool.get().is_some()
    }

    /// Returns the number of threads the pool is configured with, without creating it.
    pub const fn num_threads(&self) -> usize {
        self.num_threads
    }

    /// Returns the total number of threads in the underlying rayon pool.
    pub fn current_num_threads(&self) -> usize {
        self.pool().current_num_threads()
    }

    /// Runs a closure on `num_threads` threads in the pool, giving mutable access to each
    /// thread's [`Worker`].
    ///
    /// The closure runs with exclusive access to each selected thread's worker.
    /// Only `num_threads` threads execute the closure; the rest skip it.
    pub fn broadcast(&self, num_threads: usize, f: impl Fn(&mut Worker) + Sync) {
        if num_threads >= self.pool().current_num_threads() {
            // Fast path: run on every thread, no atomic coordination needed.
            self.pool().broadcast(|_| {
                WORKER.with_borrow_mut(|worker| f(worker));
            });
        } else {
            let remaining = AtomicUsize::new(num_threads);
            self.pool().broadcast(|_| {
                // Atomically claim a slot; threads that can't decrement skip the closure.
                let mut current = remaining.load(Ordering::Relaxed);
                loop {
                    if current == 0 {
                        return;
                    }
                    match remaining.compare_exchange_weak(
                        current,
                        current - 1,
                        Ordering::Relaxed,
                        Ordering::Relaxed,
                    ) {
                        Ok(_) => break,
                        Err(actual) => current = actual,
                    }
                }
                WORKER.with_borrow_mut(|worker| f(worker));
            });
        }
    }

    /// Runs a closure on the pool with access to the calling thread's [`Worker`].
    ///
    /// All rayon parallelism (e.g. `par_iter`) spawned inside the closure executes on this pool.
    /// Each thread can access its own [`Worker`] via the provided reference or through additional
    /// [`WorkerPool::with_worker`] calls.
    pub fn install<R: Send>(&self, f: impl FnOnce(&Worker) -> R + Send) -> R {
        let pool = self.pool();
        let metrics = self.metrics().clone();
        let queued_at = Instant::now();

        pool.install(move || {
            let started_at = Instant::now();
            metrics.record_job_queue_wait(started_at.saturating_duration_since(queued_at));
            let _record_job_duration = RecordWorkerPoolJobDurationOnDrop::new(metrics, started_at);
            WORKER.with_borrow(|worker| f(worker))
        })
    }

    /// Runs a closure on the pool without worker state access.
    ///
    /// Like [`install`](Self::install) but for closures that don't need per-thread [`Worker`]
    /// state.
    pub fn install_fn<R: Send>(&self, f: impl FnOnce() -> R + Send) -> R {
        let pool = self.pool();
        let metrics = self.metrics().clone();
        let queued_at = Instant::now();

        pool.install(move || {
            let started_at = Instant::now();
            metrics.record_job_queue_wait(started_at.saturating_duration_since(queued_at));
            let _record_job_duration = RecordWorkerPoolJobDurationOnDrop::new(metrics, started_at);
            f()
        })
    }

    /// Runs a closure on this pool, waiting for its result.
    ///
    /// Unlike [`install_fn`](Self::install_fn), this always queues the closure onto this pool
    /// when called from another rayon pool. This avoids Rayon running the closure on the caller's
    /// worker through its cross-pool install path.
    pub fn spawn_and_wait<R: Send + 'static>(&self, f: impl FnOnce() -> R + Send + 'static) -> R {
        let pool = self.pool();
        if pool.current_thread_index().is_some() {
            return f()
        }

        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        self.spawn(move || {
            let _ = tx.send(catch_unwind(AssertUnwindSafe(f)));
        });

        match rx.recv().expect("worker pool exited before completing task") {
            Ok(result) => result,
            Err(panic) => std::panic::resume_unwind(panic),
        }
    }

    /// Spawns a closure on the pool.
    pub fn spawn(&self, f: impl FnOnce() + Send + 'static) {
        let pool = self.pool();
        let metrics = self.metrics().clone();
        let queued_at = Instant::now();

        pool.spawn(move || {
            let started_at = Instant::now();
            metrics.record_job_queue_wait(started_at.saturating_duration_since(queued_at));
            let _record_job_duration = RecordWorkerPoolJobDurationOnDrop::new(metrics, started_at);
            f();
        });
    }

    /// Executes `f` on this pool using [`rayon::in_place_scope`], which converts the calling
    /// thread into a worker for the duration — tasks spawned inside the scope run on the pool
    /// and the call blocks until all of them complete.
    pub fn in_place_scope<'scope, R>(&self, f: impl FnOnce(&rayon::Scope<'scope>) -> R) -> R {
        self.pool().in_place_scope(f)
    }

    /// Accesses the current thread's [`Worker`] from within a pool closure.
    ///
    /// This is useful for accessing the worker from inside `par_iter` where the initial `&Worker`
    /// reference from `install` belongs to a different thread.
    ///
    /// This borrows the thread-local worker for the entire duration of `f`. Do not yield to Rayon
    /// from inside `f` if another job could call [`with_worker_mut`](Self::with_worker_mut) on the
    /// same thread. Yield points include parallel iterators, `rayon::join`, scopes, and waiting in
    /// `ThreadPool::install` on a different Rayon pool. A cross-pool `install` may run another job
    /// from the caller's pool on the same thread while it waits.
    ///
    /// Prefer copying or cloning the required worker state in `f`, returning from this method to
    /// release the borrow, and only then performing work that may yield.
    ///
    /// # Panics
    ///
    /// Panics if the current thread's worker is already mutably borrowed, including by a
    /// re-entrant [`with_worker_mut`](Self::with_worker_mut) call.
    pub fn with_worker<R>(f: impl FnOnce(&Worker) -> R) -> R {
        WORKER.with_borrow(|worker| f(worker))
    }

    /// Mutably accesses the current thread's [`Worker`] from within a pool closure.
    ///
    /// This exclusively borrows the thread-local worker for the entire duration of `f`. The borrow
    /// is not re-entrant: if Rayon runs another job on the same thread before `f` returns, any call
    /// to [`with_worker`](Self::with_worker) or `with_worker_mut` from that job will panic.
    ///
    /// Do not call operations that can yield to Rayon from inside `f` when re-entrant worker access
    /// is possible. This includes parallel iterators, `rayon::join`, scopes, and waiting in
    /// `ThreadPool::install` on a different Rayon pool. In particular, a cross-pool `install`
    /// cooperatively runs jobs from the caller's pool while waiting for the target pool.
    ///
    /// Prefer computing updates before entering `with_worker_mut`, then use this closure only to
    /// apply the update to the worker state.
    ///
    /// # Panics
    ///
    /// Panics if the current thread's worker is already borrowed, including by a re-entrant
    /// `with_worker` or `with_worker_mut` call.
    pub fn with_worker_mut<R>(f: impl FnOnce(&mut Worker) -> R) -> R {
        WORKER.with_borrow_mut(|worker| f(worker))
    }
}

/// Records a worker pool job's run time when the job finishes or unwinds.
struct RecordWorkerPoolJobDurationOnDrop {
    metrics: WorkerPoolMetrics,
    started_at: Instant,
}

impl RecordWorkerPoolJobDurationOnDrop {
    const fn new(metrics: WorkerPoolMetrics, started_at: Instant) -> Self {
        Self { metrics, started_at }
    }
}

impl Drop for RecordWorkerPoolJobDurationOnDrop {
    fn drop(&mut self) {
        self.metrics.record_job_duration(self.started_at.elapsed());
    }
}

/// Builds a rayon thread pool with a panic handler that prevents aborting the process.
///
/// Rust's default panic hook already logs the panic message and backtrace to stderr, so the handler
/// itself is intentionally a no-op.
pub fn build_pool_with_panic_handler(
    builder: rayon::ThreadPoolBuilder,
) -> Result<rayon::ThreadPool, rayon::ThreadPoolBuildError> {
    builder.panic_handler(|_| {}).build()
}

/// Per-thread state container for a [`WorkerPool`].
///
/// Holds a type-erased `Box<dyn Any>` that can be initialized and accessed with concrete types
/// through [`WorkerState`].
#[derive(Debug, Default)]
pub struct Worker {
    state: Option<(Arc<()>, Box<dyn Any>)>,
}

impl Worker {
    /// Creates a new empty `Worker`.
    const fn new() -> Self {
        Self { state: None }
    }

    /// Initializes the worker state.
    ///
    /// If state of type `T` already exists for `run_id`, passes `Some(&mut T)` to the closure
    /// so resources can be reused. Otherwise, drops the previous state and passes `None`.
    fn init<T: 'static>(&mut self, run_id: &Arc<()>, f: impl FnOnce(Option<&mut T>) -> T) {
        let existing = self.state.take().and_then(|(owner, boxed)| {
            (Arc::ptr_eq(&owner, run_id) && boxed.is::<T>()).then_some(boxed)
        });

        let new_state = match existing {
            Some(mut boxed) => {
                let r = boxed.downcast_mut::<T>().expect("type checked above");
                *r = f(Some(r));
                boxed
            }
            None => Box::new(f(None)),
        };

        self.state = Some((run_id.clone(), new_state));
    }

    /// Returns a reference to the state for `run_id`, downcasted to `T`.
    ///
    /// # Panics
    ///
    /// Panics if the worker has not been initialized or if the run or type does not match.
    fn get<T: 'static>(&self, run_id: &Arc<()>) -> &T {
        let (owner, state) = self.state.as_ref().expect("worker not initialized");
        assert!(Arc::ptr_eq(owner, run_id), "worker state run mismatch");
        state.downcast_ref::<T>().expect("worker state type mismatch")
    }

    /// Returns a mutable reference to the state for `run_id`, downcasted to `T`.
    ///
    /// # Panics
    ///
    /// Panics if the worker has not been initialized or if the run or type does not match.
    fn get_mut<T: 'static>(&mut self, run_id: &Arc<()>) -> &mut T {
        let (owner, state) = self.state.as_mut().expect("worker not initialized");
        assert!(Arc::ptr_eq(owner, run_id), "worker state run mismatch");
        state.downcast_mut::<T>().expect("worker state type mismatch")
    }

    /// Returns mutable state for `run_id`, creating it with `f` if absent.
    ///
    /// Reuses existing state without calling `f` when both its run and type match.
    /// Otherwise, drops the previous state before calling `f` to create its replacement.
    /// If `f` panics, the worker remains uninitialized.
    fn get_or_init<T: 'static>(&mut self, run_id: &Arc<()>, f: impl FnOnce() -> T) -> &mut T {
        if !self
            .state
            .as_ref()
            .is_some_and(|(owner, state)| Arc::ptr_eq(owner, run_id) && state.is::<T>())
        {
            self.state = None;
            self.state = Some((run_id.clone(), Box::new(f())));
        }
        self.get_mut(run_id)
    }

    /// Drops the worker state only if it belongs to `run_id`.
    fn clear(&mut self, run_id: &Arc<()>) {
        if self.state.as_ref().is_some_and(|(owner, _)| Arc::ptr_eq(owner, run_id)) {
            self.state = None;
        }
    }
}

/// Owns one run's thread-local state on a shared [`WorkerPool`].
///
/// Share this value with jobs through an [`Arc`]. Dropping the last handle clears this run's
/// state on every pool thread, leaving other runs' state untouched. Worker slots retain only
/// a private identity token and do not keep this owner alive.
///
/// Access closures borrow the current thread's worker and must not yield to Rayon or recursively
/// access worker state. Use them only in jobs executing on this pool. Cleanup broadcasts to the
/// pool and blocks until complete; the last handle must be dropped outside worker access closures.
#[derive(Debug)]
pub struct WorkerState {
    pool: Arc<WorkerPool>,
    run_id: Arc<()>,
}

impl WorkerState {
    /// Creates a fresh state owner without initializing the pool or its workers.
    pub fn new(pool: Arc<WorkerPool>) -> Self {
        Self { pool, run_id: Arc::new(()) }
    }

    /// Initializes this run's state on every pool thread.
    ///
    /// Offers existing state to `f` only when its run and type match. Otherwise, drops the previous
    /// state before calling `f` with `None`.
    pub fn init<T: 'static>(&self, f: impl Fn(Option<&mut T>) -> T + Sync) {
        self.pool.broadcast(self.pool.current_num_threads(), |worker| {
            worker.init(&self.run_id, &f);
        });
    }

    /// Reads this run's state on the current worker thread.
    ///
    /// # Panics
    ///
    /// Panics if the worker is uninitialized, its run or type differs, or it is mutably borrowed.
    pub fn get<T: 'static, R>(&self, f: impl FnOnce(&T) -> R) -> R {
        WorkerPool::with_worker(|worker| f(worker.get(&self.run_id)))
    }

    /// Mutably accesses this run's state, initializing it on the current worker if needed.
    ///
    /// Reuses state without calling `init` when both its run and type match. Otherwise, drops the
    /// previous state before calling `init`. If `init` panics, the worker remains uninitialized.
    ///
    /// # Panics
    ///
    /// Panics if the current worker is already borrowed.
    pub fn get_or_init<T: 'static, R>(
        &self,
        init: impl FnOnce() -> T,
        f: impl FnOnce(&mut T) -> R,
    ) -> R {
        WorkerPool::with_worker_mut(|worker| f(worker.get_or_init(&self.run_id, init)))
    }
}

impl Drop for WorkerState {
    fn drop(&mut self) {
        if let Some(pool) = self.pool.pool.get() {
            pool.broadcast(|_| {
                WORKER.with_borrow_mut(|worker| worker.clear(&self.run_id));
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rayon::prelude::*;
    use std::time::Duration;

    #[tokio::test]
    async fn blocking_pool() {
        let pool = BlockingTaskPool::build().unwrap();
        let res = pool.spawn(move || 5);
        let res = res.await.unwrap();
        assert_eq!(res, 5);
    }

    #[tokio::test]
    async fn blocking_pool_panic() {
        let pool = BlockingTaskPool::build().unwrap();
        let res = pool.spawn(move || -> i32 {
            panic!();
        });
        let res = res.await;
        assert!(res.is_err());
    }

    #[test]
    fn worker_pool_init_and_access() {
        let pool = Arc::new(WorkerPool::new(2, "test"));
        let state = WorkerState::new(Arc::clone(&pool));
        state.init(|_| vec![1u8, 2, 3]);
        let sum = pool.install_fn(|| state.get(|v: &Vec<u8>| v.iter().sum::<u8>()));
        assert_eq!(sum, 6);
        drop(state);
        pool.broadcast(2, |worker| assert!(worker.state.is_none()));
    }

    #[test]
    fn worker_pool_reinit_reuses_resources() {
        let pool = Arc::new(WorkerPool::new(1, "test"));
        let state = WorkerState::new(Arc::clone(&pool));
        state.init(|existing: Option<&mut Vec<u8>>| {
            assert!(existing.is_none());
            vec![1, 2, 3]
        });
        state.init(|existing: Option<&mut Vec<u8>>| {
            let v = existing.expect("should have existing state");
            assert_eq!(v, &mut vec![1, 2, 3]);
            v.push(4);
            std::mem::take(v)
        });
        let len = pool.install_fn(|| state.get(Vec::<u8>::len));
        assert_eq!(len, 4);
    }

    #[test]
    fn worker_pool_clear_and_reinit() {
        let pool = Arc::new(WorkerPool::new(1, "test"));
        let state = WorkerState::new(Arc::clone(&pool));
        state.init(|_| 42u64);
        assert_eq!(pool.install_fn(|| state.get(|v: &u64| *v)), 42);
        drop(state);
        let state = WorkerState::new(Arc::clone(&pool));
        state.init(|_| String::from("hello"));
        assert_eq!(pool.install_fn(|| state.get(String::clone)), "hello");
    }

    #[test]
    fn worker_pool_par_iter_with_worker() {
        let pool = Arc::new(WorkerPool::new(2, "test"));
        let state = WorkerState::new(Arc::clone(&pool));
        state.init(|_| 10u64);
        let results = pool.install_fn(|| {
            (0u64..4).into_par_iter().map(|i| state.get(|v: &u64| i + *v)).collect::<Vec<_>>()
        });
        assert_eq!(results, vec![10, 11, 12, 13]);
    }

    #[test]
    fn worker_pool_spawn_and_wait_runs_on_target_pool() {
        let caller_pool = WorkerPool::new(1, "caller");
        let target_pool = Arc::new(WorkerPool::new(1, "target"));

        let thread_name = caller_pool.install_fn({
            let target_pool = Arc::clone(&target_pool);
            move || {
                WorkerPool::with_worker_mut(|_| {
                    target_pool.spawn_and_wait(|| thread::current().name().unwrap().to_owned())
                })
            }
        });

        assert_eq!(thread_name, "target-00");
    }

    #[test]
    fn worker_get_or_init_reuses_only_matching_run_and_type() {
        let mut worker = Worker::new();
        let first = Arc::new(());
        let second = Arc::new(());

        worker.get_or_init(&first, || vec![1u8]).push(2);
        assert_eq!(worker.get_or_init::<Vec<u8>>(&first.clone(), || panic!("must reuse")), &[1, 2]);
        assert_eq!(worker.get_or_init(&second, || vec![3u8]), &[3]);
        assert_eq!(worker.get_or_init(&second, || String::from("new type")), "new type");
        assert_eq!(*worker.get_or_init(&first, || 42u64), 42);
    }

    #[test]
    fn worker_init_reuses_only_matching_run_and_type() {
        let mut worker = Worker::new();
        let first = Arc::new(());
        let second = Arc::new(());
        worker.init(&first, |_| 1u64);
        worker.init(&second, |existing: Option<&mut u64>| {
            assert!(existing.is_none());
            2
        });
        worker.init(&second, |existing: Option<&mut String>| {
            assert!(existing.is_none());
            String::from("new type")
        });
        assert_eq!(worker.get::<String>(&second), "new type");
    }

    #[test]
    fn worker_pool_late_cleanup_preserves_other_runs() {
        let pool = Arc::new(WorkerPool::new(2, "run-cleanup"));
        let first = WorkerState::new(Arc::clone(&pool));
        let second = WorkerState::new(Arc::clone(&pool));
        first.init(|_| 1u64);
        pool.broadcast(1, |worker| {
            assert_eq!(*worker.get_or_init(&second.run_id, || 2u64), 2);
        });
        drop(first);
        let retained = AtomicUsize::new(0);
        pool.broadcast(2, |worker| {
            if let Some((owner, state)) = &worker.state {
                assert!(Arc::ptr_eq(owner, &second.run_id));
                assert_eq!(state.downcast_ref::<u64>(), Some(&2));
                retained.fetch_add(1, Ordering::Relaxed);
            }
        });
        assert_eq!(retained.load(Ordering::Relaxed), 1);
        drop(second);
        pool.broadcast(2, |worker| assert!(worker.state.is_none()));
    }

    #[test]
    #[should_panic(expected = "worker state run mismatch")]
    fn worker_get_rejects_another_run() {
        let mut worker = Worker::new();
        worker.init(&Arc::new(()), |_| 1u64);
        worker.get::<u64>(&Arc::new(()));
    }

    #[test]
    #[should_panic(expected = "worker state run mismatch")]
    fn worker_get_mut_rejects_another_run() {
        let mut worker = Worker::new();
        worker.init(&Arc::new(()), |_| 1u64);
        worker.get_mut::<u64>(&Arc::new(()));
    }

    #[test]
    fn worker_replacement_drops_previous_state_before_initialization() {
        let mut worker = Worker::new();
        let previous_run = Arc::new(());
        let next_run = Arc::new(());
        let resource = Arc::new(());

        for run_id in [&previous_run, &next_run] {
            worker.init(&previous_run, |_| Arc::clone(&resource));
            assert_eq!(Arc::strong_count(&resource), 2);
            assert_eq!(
                *worker.get_or_init(run_id, || {
                    assert_eq!(Arc::strong_count(&resource), 1);
                    42u64
                }),
                42,
            );
        }
    }

    #[test]
    fn worker_failed_replacement_leaves_state_uninitialized() {
        let mut worker = Worker::new();
        let previous_run = Arc::new(());
        let next_run = Arc::new(());
        worker.init(&previous_run, |_| 1u64);

        let panic = catch_unwind(AssertUnwindSafe(|| {
            worker.get_or_init::<u64>(&next_run, || panic!("initialization failed"));
        }))
        .unwrap_err();
        assert_eq!(panic.downcast_ref::<&str>(), Some(&"initialization failed"));
        assert!(worker.state.is_none());
        assert_eq!(*worker.get_or_init(&next_run, || 2u64), 2);
    }

    #[test]
    fn worker_state_drop_does_not_initialize_unused_pool() {
        let pool = Arc::new(WorkerPool::new(1, "unused-state"));
        drop(WorkerState::new(Arc::clone(&pool)));
        assert!(!pool.is_initialized());
    }

    #[test]
    fn worker_state_last_job_handle_clears_state() {
        let pool = Arc::new(WorkerPool::new(2, "owned-state"));
        let state = Arc::new(WorkerState::new(Arc::clone(&pool)));
        let owner = Arc::downgrade(&state);
        let resource = Arc::new(());
        state.init(|_| Arc::clone(&resource));
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let job_state = Arc::clone(&state);
        pool.spawn(move || {
            ready_tx.send(()).unwrap();
            release_rx.recv_timeout(Duration::from_secs(5)).unwrap();
            job_state.get(|value: &Arc<()>| assert_eq!(Arc::strong_count(value), 3));
            drop(job_state);
            done_tx.send(()).unwrap();
        });
        ready_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        drop(state);
        assert!(owner.upgrade().is_some());
        assert_eq!(Arc::strong_count(&resource), 3);
        release_tx.send(()).unwrap();
        done_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(owner.upgrade().is_none());
        assert_eq!(Arc::strong_count(&resource), 1);
        pool.broadcast(2, |worker| assert!(worker.state.is_none()));
    }

    #[test]
    fn worker_state_clears_after_job_panics() {
        let pool = Arc::new(WorkerPool::new(1, "unwind-state"));
        let state = Arc::new(WorkerState::new(Arc::clone(&pool)));
        let resource = Arc::new(());
        state.init(|_| Arc::clone(&resource));
        let panic = catch_unwind(AssertUnwindSafe(|| {
            pool.install_fn(move || {
                state.get_or_init(|| Arc::new(()), |_| panic!("job failed"));
            });
        }))
        .unwrap_err();
        assert_eq!(panic.downcast_ref::<&str>(), Some(&"job failed"));
        assert_eq!(Arc::strong_count(&resource), 1);
        pool.broadcast(1, |worker| assert!(worker.state.is_none()));
    }
}
