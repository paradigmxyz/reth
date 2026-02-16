//! Additional helpers for executing tracing calls

use std::{
    any::Any,
    cell::RefCell,
    future::Future,
    panic::{catch_unwind, AssertUnwindSafe},
    pin::Pin,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    task::{ready, Context, Poll},
    thread,
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
/// [`Worker::init`]. The state is thread-local and accessible during [`install`](Self::install)
/// calls.
///
/// The pool supports multiple init/clear cycles, allowing reuse of the same threads with
/// different state configurations.
///
/// The number of active threads can be dynamically controlled via [`set_num_threads`](Self::set_num_threads)
/// without recreating the pool.
#[derive(Debug)]
pub struct WorkerPool {
    pool: rayon::ThreadPool,
    /// How many threads should participate in [`broadcast`](Self::broadcast) calls.
    /// Threads beyond this count skip the closure.
    num_threads: AtomicUsize,
}

impl WorkerPool {
    /// Creates a new `WorkerPool` with the given number of threads.
    pub fn new(num_threads: usize) -> Result<Self, rayon::ThreadPoolBuildError> {
        Self::from_builder(rayon::ThreadPoolBuilder::new().num_threads(num_threads))
    }

    /// Creates a new `WorkerPool` from a [`rayon::ThreadPoolBuilder`].
    pub fn from_builder(
        builder: rayon::ThreadPoolBuilder,
    ) -> Result<Self, rayon::ThreadPoolBuildError> {
        let pool = builder.build()?;
        let num_threads = pool.current_num_threads();
        Ok(Self { pool, num_threads: AtomicUsize::new(num_threads) })
    }

    /// Returns the total number of threads in the underlying rayon pool.
    pub fn current_num_threads(&self) -> usize {
        self.pool.current_num_threads()
    }

    /// Returns the number of active threads that will participate in
    /// [`broadcast`](Self::broadcast).
    pub fn num_threads(&self) -> usize {
        self.num_threads.load(Ordering::Relaxed)
    }

    /// Sets the number of threads that will participate in [`broadcast`](Self::broadcast).
    ///
    /// Clamped to the pool's total thread count.
    pub fn set_num_threads(&self, n: usize) {
        let clamped = n.min(self.pool.current_num_threads());
        self.num_threads.store(clamped, Ordering::Relaxed);
    }

    /// Runs a closure on up to [`num_threads`](Self::num_threads) threads in the pool, giving
    /// mutable access to each thread's [`Worker`].
    ///
    /// Use this to initialize or re-initialize per-thread state via [`Worker::init`].
    /// Threads beyond the configured `num_threads` limit skip the closure.
    pub fn broadcast(&self, f: impl Fn(&mut Worker) + Sync) {
        let remaining = AtomicUsize::new(self.num_threads.load(Ordering::Relaxed));
        self.pool.broadcast(|_| {
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

    /// Clears the state on every thread in the pool.
    pub fn clear(&self) {
        self.pool.broadcast(|_| {
            WORKER.with_borrow_mut(Worker::clear);
        });
    }

    /// Runs a closure on the pool with access to the calling thread's [`Worker`].
    ///
    /// All rayon parallelism (e.g. `par_iter`) spawned inside the closure executes on this pool.
    /// Each thread can access its own [`Worker`] via the provided reference or through additional
    /// [`WorkerPool::with_worker`] calls.
    pub fn install<R: Send>(&self, f: impl FnOnce(&Worker) -> R + Send) -> R {
        self.pool.install(|| WORKER.with_borrow(|worker| f(worker)))
    }

    /// Runs a closure on the pool without worker state access.
    ///
    /// Like [`install`](Self::install) but for closures that don't need per-thread [`Worker`]
    /// state.
    pub fn install_fn<R: Send>(&self, f: impl FnOnce() -> R + Send) -> R {
        self.pool.install(f)
    }

    /// Spawns a closure on the pool.
    pub fn spawn(&self, f: impl FnOnce() + Send + 'static) {
        self.pool.spawn(f);
    }

    /// Access the current thread's [`Worker`] from within an [`install`](Self::install) closure.
    ///
    /// This is useful for accessing the worker from inside `par_iter` where the initial `&Worker`
    /// reference from `install` belongs to a different thread.
    pub fn with_worker<R>(f: impl FnOnce(&Worker) -> R) -> R {
        WORKER.with_borrow(|worker| f(worker))
    }
}

/// Per-thread state container for a [`WorkerPool`].
///
/// Holds a type-erased `Box<dyn Any>` that can be initialized and accessed with concrete types
/// via [`init`](Self::init) and [`get`](Self::get).
#[derive(Debug, Default)]
pub struct Worker {
    state: Option<Box<dyn Any>>,
}

impl Worker {
    /// Creates a new empty `Worker`.
    const fn new() -> Self {
        Self { state: None }
    }

    /// Initializes the worker state.
    ///
    /// If state of type `T` already exists, passes `Some(&mut T)` to the closure so resources
    /// can be reused. On first init, passes `None`.
    pub fn init<T: 'static>(&mut self, f: impl FnOnce(Option<&mut T>) -> T) {
        let existing =
            self.state.take().and_then(|mut b| b.downcast_mut::<T>().is_some().then_some(b));

        let new_state = match existing {
            Some(mut boxed) => {
                let r = boxed.downcast_mut::<T>().expect("type checked above");
                *r = f(Some(r));
                boxed
            }
            None => Box::new(f(None)),
        };

        self.state = Some(new_state);
    }

    /// Returns a reference to the state, downcasted to `T`.
    ///
    /// # Panics
    ///
    /// Panics if the worker has not been initialized or if the type does not match.
    pub fn get<T: 'static>(&self) -> &T {
        self.state
            .as_ref()
            .expect("worker not initialized")
            .downcast_ref::<T>()
            .expect("worker state type mismatch")
    }

    /// Clears the worker state, dropping the contained value.
    pub fn clear(&mut self) {
        self.state = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let pool = WorkerPool::new(2).unwrap();

        pool.broadcast(|worker| {
            worker.init::<Vec<u8>>(|_| vec![1, 2, 3]);
        });

        let sum: u8 = pool.install(|worker| {
            let v = worker.get::<Vec<u8>>();
            v.iter().sum()
        });
        assert_eq!(sum, 6);

        pool.clear();
    }

    #[test]
    fn worker_pool_reinit_reuses_resources() {
        let pool = WorkerPool::new(1).unwrap();

        pool.broadcast(|worker| {
            worker.init::<Vec<u8>>(|existing| {
                assert!(existing.is_none());
                vec![1, 2, 3]
            });
        });

        pool.broadcast(|worker| {
            worker.init::<Vec<u8>>(|existing| {
                let v = existing.expect("should have existing state");
                assert_eq!(v, &mut vec![1, 2, 3]);
                v.push(4);
                std::mem::take(v)
            });
        });

        let len = pool.install(|worker| worker.get::<Vec<u8>>().len());
        assert_eq!(len, 4);

        pool.clear();
    }

    #[test]
    fn worker_pool_clear_and_reinit() {
        let pool = WorkerPool::new(1).unwrap();

        pool.broadcast(|worker| {
            worker.init::<u64>(|_| 42);
        });
        let val = pool.install(|worker| *worker.get::<u64>());
        assert_eq!(val, 42);

        pool.clear();

        pool.broadcast(|worker| {
            worker.init::<String>(|_| "hello".to_string());
        });
        let val = pool.install(|worker| worker.get::<String>().clone());
        assert_eq!(val, "hello");

        pool.clear();
    }

    #[test]
    fn worker_pool_par_iter_with_worker() {
        use rayon::prelude::*;

        let pool = WorkerPool::new(2).unwrap();

        pool.broadcast(|worker| {
            worker.init::<u64>(|_| 10);
        });

        let results: Vec<u64> = pool.install(|_| {
            (0u64..4)
                .into_par_iter()
                .map(|i| WorkerPool::with_worker(|w| i + *w.get::<u64>()))
                .collect()
        });
        assert_eq!(results, vec![10, 11, 12, 13]);

        pool.clear();
    }
}
