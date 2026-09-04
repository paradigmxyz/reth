//! Execution and time for tasks shared by production and deterministic simulation.
//!
//! Futures must yield while waiting for other work. Synchronous jobs must be bounded: simulation
//! executes each closure atomically and cannot suspend a blocking channel receive, lock, or FFI
//! call.

use crate::Runtime;
use futures_util::{
    future::{AbortHandle, AbortRegistration, Abortable},
    FutureExt,
};
use std::{
    future::Future,
    panic::{catch_unwind, AssertUnwindSafe},
    pin::Pin,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    task::{ready, Context, Poll},
    time::{Duration, SystemTime},
};
use tokio::sync::{mpsc, oneshot};

#[cfg(feature = "deterministic")]
use {
    commonware_runtime::{Clock, Spawner, Supervisor},
    parking_lot::Mutex,
    std::collections::BTreeMap,
};

/// Task execution and time without exposing an underlying executor.
///
/// Production uses [`Runtime`]'s Tokio, Rayon, and OS workers. The deterministic backend polls
/// futures through Commonware and executes synchronous jobs atomically. Channel synchronization
/// must use asynchronous operations in simulated actors; a blocking wait stalls the whole
/// simulator.
///
/// Async primitives also need deterministic wake order. Tokio `mpsc`, `oneshot`, and a single
/// `Notify` queue work with this executor. Tokio `watch` shards its waiters using ambient
/// randomness, and unbiased `tokio::select!` randomizes polling order; avoid both in simulated
/// actors.
#[derive(Clone)]
pub struct TaskRuntime(Arc<Backend>);

impl TaskRuntime {
    /// Creates a runtime driven entirely by the supplied Commonware context.
    ///
    /// Keep this handle inside the context's runner. All associated tasks are stopped when that
    /// runner exits. No production executor is constructed or accessible through this handle.
    #[cfg(feature = "deterministic")]
    pub fn deterministic(context: commonware_runtime::deterministic::Context) -> Self {
        Self(Arc::new(Backend::Deterministic(Simulation {
            context,
            workers: Mutex::new(BTreeMap::new()),
        })))
    }

    /// Spawns a cooperative future. Dropping its handle leaves the task running.
    pub fn spawn<F, T>(&self, name: &'static str, future: F) -> TaskHandle<T>
    where
        F: Future<Output = T> + Send + 'static,
        T: Send + 'static,
    {
        self.spawn_async(name, AsyncKind::Shared, future)
    }

    /// Runs a cooperative service future on its own OS thread in production.
    ///
    /// Simulation polls the same future through Commonware. The service must await its input;
    /// wrapping an ordinary blocking receive loop in an async block does not make it cooperative.
    pub fn spawn_dedicated_task<F, T>(&self, name: &'static str, future: F) -> TaskHandle<T>
    where
        F: Future<Output = T> + Send + 'static,
        T: Send + 'static,
    {
        self.spawn_async(name, AsyncKind::Dedicated, future)
    }

    /// Runs a cooperative future on the persistent OS worker identified by `name`.
    ///
    /// Synchronous and asynchronous named tasks share submission order. The future owns its lane
    /// until it completes, including while awaiting input. It may await work on another lane but
    /// must not await a later job on the same lane. Simulation preserves the same queue semantics.
    pub fn spawn_named_task<F, T>(&self, name: &'static str, future: F) -> TaskHandle<T>
    where
        F: Future<Output = T> + Send + 'static,
        T: Send + 'static,
    {
        self.spawn_async(name, AsyncKind::Named, future)
    }

    fn spawn_async<F, T>(&self, name: &'static str, kind: AsyncKind, future: F) -> TaskHandle<T>
    where
        F: Future<Output = T> + Send + 'static,
        T: Send + 'static,
    {
        let (handle, completion, registration) = TaskHandle::new();
        let future = async move {
            let outcome =
                AssertUnwindSafe(Abortable::new(future, registration)).catch_unwind().await;
            let result = match outcome {
                Ok(Ok(value)) => Ok(value),
                Ok(Err(_)) => Err(TaskError::Canceled),
                Err(_) => Err(TaskError::Panicked),
            };
            let _ = completion.send(result);
        };
        match self.0.as_ref() {
            Backend::Production(runtime) => {
                if matches!(kind, AsyncKind::Shared) {
                    drop(runtime.spawn_task(future));
                } else {
                    let handle = runtime.handle().clone();
                    let shutdown = runtime.on_shutdown_signal().clone();
                    let task = move || {
                        handle.block_on(async move {
                            let _ =
                                futures_util::future::select(Box::pin(shutdown), Box::pin(future))
                                    .await;
                        });
                    };
                    if matches!(kind, AsyncKind::Named) {
                        drop(runtime.spawn_blocking_named(name, task));
                    } else {
                        drop(
                            std::thread::Builder::new()
                                .name(name.to_owned())
                                .spawn(task)
                                .unwrap_or_else(|error| {
                                    panic!("failed to spawn cooperative worker {name:?}: {error}")
                                }),
                        );
                    }
                }
            }
            #[cfg(feature = "deterministic")]
            Backend::Deterministic(simulation) => {
                if matches!(kind, AsyncKind::Named) {
                    simulation.spawn_named(name, Box::pin(future));
                } else {
                    drop(
                        simulation
                            .context
                            .child("task")
                            .with_attribute("name", name)
                            .spawn(|_| future),
                    );
                }
            }
        }
        handle
    }

    /// Runs a bounded CPU job on Rayon when available, otherwise on Tokio's blocking pool.
    ///
    /// In simulation the job is one schedulable, atomic operation. Nested jobs may be submitted,
    /// but must be awaited by a cooperative caller after the outer closure returns.
    pub fn spawn_cpu<F, T>(&self, name: &'static str, job: F) -> TaskHandle<T>
    where
        F: FnOnce() -> T + Send + 'static,
        T: Send + 'static,
    {
        self.spawn_job(name, JobKind::Cpu, job)
    }

    /// Runs a bounded closure on Tokio's blocking pool, or atomically in simulation.
    pub fn spawn_blocking<F, T>(&self, name: &'static str, job: F) -> TaskHandle<T>
    where
        F: FnOnce() -> T + Send + 'static,
        T: Send + 'static,
    {
        self.spawn_job(name, JobKind::Blocking, job)
    }

    /// Runs bounded jobs in submission order on a persistent worker identified by `name`.
    ///
    /// The deterministic backend preserves that FIFO lane. A job must not wait synchronously for
    /// another job, including one on its own lane. Return the nested handle and await it instead.
    /// Simulation does not reproduce separate OS thread-local state for each lane.
    pub fn spawn_named<F, T>(&self, name: &'static str, job: F) -> TaskHandle<T>
    where
        F: FnOnce() -> T + Send + 'static,
        T: Send + 'static,
    {
        self.spawn_job(name, JobKind::Named, job)
    }

    /// Uses an idle named OS worker, or the blocking pool when that worker is occupied.
    ///
    /// These jobs have no FIFO guarantee. Simulation makes each bounded closure independently
    /// schedulable, so callers must not depend on the named worker being available.
    pub fn spawn_named_or_blocking<F, T>(&self, name: &'static str, job: F) -> TaskHandle<T>
    where
        F: FnOnce() -> T + Send + 'static,
        T: Send + 'static,
    {
        self.spawn_job(name, JobKind::NamedOrBlocking, job)
    }

    /// Runs a bounded closure on a new OS thread, or as an atomic simulated job.
    pub fn spawn_dedicated<F, T>(&self, name: &'static str, job: F) -> TaskHandle<T>
    where
        F: FnOnce() -> T + Send + 'static,
        T: Send + 'static,
    {
        self.spawn_job(name, JobKind::Dedicated, job)
    }

    /// Returns the runtime's wall clock. Simulated time comes only from Commonware.
    pub fn now(&self) -> SystemTime {
        match self.0.as_ref() {
            Backend::Production(_) => SystemTime::now(),
            #[cfg(feature = "deterministic")]
            Backend::Deterministic(simulation) => simulation.context.current(),
        }
    }

    /// Waits using the runtime's clock.
    pub async fn sleep(&self, duration: Duration) {
        match self.0.as_ref() {
            Backend::Production(runtime) => {
                let sleep = {
                    let _guard = runtime.handle().enter();
                    tokio::time::sleep(duration)
                };
                sleep.await;
            }
            #[cfg(feature = "deterministic")]
            Backend::Deterministic(simulation) => simulation.context.sleep(duration).await,
        }
    }

    /// Yields once to the executor, even if no other operation is waiting.
    pub async fn yield_now(&self) {
        yield_once().await;
    }

    /// Creates a bounded channel usable with both executors.
    ///
    /// Use `send().await` and `recv().await` in cooperative actors. Production OS workers may
    /// use Tokio's blocking channel methods; those methods must not run inside simulated jobs.
    pub fn bounded_channel<T>(&self, capacity: usize) -> (mpsc::Sender<T>, mpsc::Receiver<T>) {
        mpsc::channel(capacity)
    }

    fn spawn_job<F, T>(&self, name: &'static str, kind: JobKind, job: F) -> TaskHandle<T>
    where
        F: FnOnce() -> T + Send + 'static,
        T: Send + 'static,
    {
        let (handle, completion, _registration) = TaskHandle::new();
        let canceled = Arc::clone(&handle.canceled);
        let job = move || {
            let result = if canceled.load(Ordering::Acquire) {
                Err(TaskError::Canceled)
            } else {
                catch_unwind(AssertUnwindSafe(job)).map_err(|_| TaskError::Panicked)
            };
            let _ = completion.send(result);
        };
        match self.0.as_ref() {
            Backend::Production(runtime) => match kind {
                JobKind::Cpu => {
                    #[cfg(feature = "rayon")]
                    runtime.cpu_pool().spawn(job);
                    #[cfg(not(feature = "rayon"))]
                    drop(runtime.spawn_blocking(job));
                }
                JobKind::Blocking => {
                    drop(runtime.spawn_blocking(job));
                }
                JobKind::Named => {
                    drop(runtime.spawn_blocking_named(name, job));
                }
                JobKind::NamedOrBlocking => {
                    runtime.spawn_blocking_named_or_tokio(name, job);
                }
                JobKind::Dedicated => {
                    drop(crate::spawn_os_thread(name, job));
                }
            },
            #[cfg(feature = "deterministic")]
            Backend::Deterministic(simulation) => {
                if matches!(kind, JobKind::Named) {
                    simulation.spawn_named(name, Box::pin(async move { job() }));
                } else {
                    drop(
                        simulation
                            .context
                            .child("job")
                            .with_attribute("name", name)
                            .spawn(|_| async move { job() }),
                    );
                }
            }
        }
        handle
    }
}

impl From<Runtime> for TaskRuntime {
    fn from(runtime: Runtime) -> Self {
        Self(Arc::new(Backend::Production(runtime)))
    }
}

impl std::fmt::Debug for TaskRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.0.as_ref() {
            Backend::Production(_) => f.write_str("TaskRuntime::Production"),
            #[cfg(feature = "deterministic")]
            Backend::Deterministic(_) => f.write_str("TaskRuntime::Deterministic"),
        }
    }
}

/// Completion of a cooperative task or bounded job.
///
/// Dropping a handle detaches its work unless [`Self::abort_on_drop`] is enabled. [`Self::abort`]
/// stops a cooperative future at a poll boundary and skips a job if cancellation is observed before
/// execution. Already-running synchronous jobs continue; their side effects cannot be rolled back.
#[must_use = "await the handle for completion, or drop it to detach the work"]
pub struct TaskHandle<T> {
    receiver: Abortable<oneshot::Receiver<Result<T, TaskError>>>,
    completion_abort: AbortHandle,
    task_abort: AbortHandle,
    canceled: Arc<AtomicBool>,
    abort_on_drop: bool,
}

impl<T> TaskHandle<T> {
    fn new() -> (Self, oneshot::Sender<Result<T, TaskError>>, AbortRegistration) {
        let (completion, receiver) = oneshot::channel();
        let (completion_abort, completion_registration) = AbortHandle::new_pair();
        let (task_abort, task_registration) = AbortHandle::new_pair();
        (
            Self {
                receiver: Abortable::new(receiver, completion_registration),
                completion_abort,
                task_abort,
                canceled: Arc::new(AtomicBool::new(false)),
                abort_on_drop: false,
            },
            completion,
            task_registration,
        )
    }

    /// Requests cancellation and makes this handle resolve to [`TaskError::Canceled`].
    pub fn abort(&self) {
        self.canceled.store(true, Ordering::Release);
        self.task_abort.abort();
        self.completion_abort.abort();
    }

    /// Cancels unfinished work when this handle is dropped instead of detaching it.
    ///
    /// Use this for child jobs owned by a cooperative driver so dropping the driver also cancels
    /// queued work. An already-running synchronous job still finishes.
    pub const fn abort_on_drop(mut self) -> Self {
        self.abort_on_drop = true;
        self
    }
}

impl<T> Future for TaskHandle<T> {
    type Output = Result<T, TaskError>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let result = match ready!(Pin::new(&mut self.receiver).poll(cx)) {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(TaskError::Stopped),
            Err(_) => Err(TaskError::Canceled),
        };
        self.abort_on_drop = false;
        Poll::Ready(result)
    }
}

impl<T> Drop for TaskHandle<T> {
    fn drop(&mut self) {
        if self.abort_on_drop {
            self.abort();
        }
    }
}

impl<T> std::fmt::Debug for TaskHandle<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TaskHandle")
            .field("canceled", &self.canceled.load(Ordering::Acquire))
            .field("abort_on_drop", &self.abort_on_drop)
            .finish_non_exhaustive()
    }
}

/// Failure to complete a task or job.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum TaskError {
    /// Cancellation was requested through the task's handle.
    #[error("task canceled")]
    Canceled,
    /// The task or job panicked.
    #[error("task panicked")]
    Panicked,
    /// The executor stopped or discarded the work before it produced a result.
    #[error("task executor stopped")]
    Stopped,
}

enum Backend {
    Production(Runtime),
    #[cfg(feature = "deterministic")]
    Deterministic(Simulation),
}

#[derive(Clone, Copy)]
enum AsyncKind {
    Shared,
    Dedicated,
    Named,
}

#[derive(Clone, Copy)]
enum JobKind {
    Cpu,
    Blocking,
    Named,
    NamedOrBlocking,
    Dedicated,
}

#[cfg(feature = "deterministic")]
type NamedTask = Pin<Box<dyn Future<Output = ()> + Send + 'static>>;

#[cfg(feature = "deterministic")]
struct Simulation {
    context: commonware_runtime::deterministic::Context,
    // Stable drop order also preserves the order in which idle lane receivers are woken.
    workers: Mutex<BTreeMap<&'static str, mpsc::UnboundedSender<NamedTask>>>,
}

#[cfg(feature = "deterministic")]
impl Simulation {
    fn spawn_named(&self, name: &'static str, task: NamedTask) {
        let mut workers = self.workers.lock();
        let sender = workers.entry(name).or_insert_with(|| {
            let (sender, mut receiver) = mpsc::unbounded_channel::<NamedTask>();
            drop(self.context.child("worker").with_attribute("name", name).spawn(|_| async move {
                while let Some(task) = receiver.recv().await {
                    task.await;
                    yield_once().await;
                }
            }));
            sender
        });
        let _ = sender.send(task);
    }
}

async fn yield_once() {
    let mut yielded = false;
    futures_util::future::poll_fn(move |cx| {
        if std::mem::replace(&mut yielded, true) {
            Poll::Ready(())
        } else {
            cx.waker().wake_by_ref();
            Poll::Pending
        }
    })
    .await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    async fn exercise(runtime: TaskRuntime) -> Vec<usize> {
        let (sender, mut receiver) = runtime.bounded_channel(1);
        sender.try_send(0).unwrap();
        assert!(matches!(sender.try_send(99), Err(mpsc::error::TrySendError::Full(99))));

        let producer_runtime = runtime.clone();
        let producer = runtime.spawn("producer", async move {
            for index in 1..5 {
                let nested_runtime = producer_runtime.clone();
                let nested = producer_runtime.spawn_cpu("outer", move || {
                    nested_runtime.spawn_cpu("inner", move || index * 2)
                });
                let value = nested.await.unwrap().await.unwrap();
                sender.send(value).await.unwrap();
            }
        });
        let consumer_runtime = runtime.clone();
        let consumer = runtime.spawn_dedicated_task("consumer", async move {
            let start = consumer_runtime.now();
            consumer_runtime.sleep(Duration::from_millis(3)).await;
            assert!(
                consumer_runtime.now().duration_since(start).unwrap() >= Duration::from_millis(3)
            );
            let mut values = Vec::new();
            while let Some(value) = receiver.recv().await {
                values.push(value);
                consumer_runtime.yield_now().await;
            }
            values
        });
        producer.await.unwrap();
        let values = consumer.await.unwrap();
        assert_eq!(values, [0, 2, 4, 6, 8]);

        let order = Arc::new(Mutex::new(Vec::new()));
        let mut jobs = Vec::new();
        for index in 0..4 {
            let order = Arc::clone(&order);
            jobs.push(runtime.spawn_named("serial", move || {
                order.lock().unwrap().push(index);
                index
            }));
        }
        for (index, job) in jobs.into_iter().enumerate() {
            assert_eq!(job.await.unwrap(), index);
        }
        assert_eq!(*order.lock().unwrap(), [0, 1, 2, 3]);

        let mixed_order = Arc::new(Mutex::new(Vec::new()));
        let first_order = Arc::clone(&mixed_order);
        let nested_runtime = runtime.clone();
        let (started, started_rx) = oneshot::channel();
        let (release, release_rx) = oneshot::channel();
        let first = runtime.spawn_named_task("mixed", async move {
            let thread = std::thread::current().id();
            first_order.lock().unwrap().push(0);
            let value = nested_runtime.spawn_named_task("mixed_other", async { 42 }).await.unwrap();
            started.send(value).unwrap();
            release_rx.await.unwrap();
            first_order.lock().unwrap().push(1);
            assert_eq!(thread, std::thread::current().id());
            thread
        });
        let second_order = Arc::clone(&mixed_order);
        let second = runtime.spawn_named("mixed", move || {
            second_order.lock().unwrap().push(2);
            std::thread::current().id()
        });
        let third_order = Arc::clone(&mixed_order);
        let third = runtime.spawn_named_task("mixed", async move {
            third_order.lock().unwrap().push(3);
        });
        assert_eq!(started_rx.await.unwrap(), 42);
        assert_eq!(*mixed_order.lock().unwrap(), [0]);
        // The fallback must progress while the named lane is occupied; placing it in that lane's
        // FIFO would deadlock behind the release that follows this await.
        assert_eq!(runtime.spawn_named_or_blocking("mixed", || 43).await.unwrap(), 43);
        assert_eq!(*mixed_order.lock().unwrap(), [0]);
        release.send(()).unwrap();
        assert_eq!(first.await.unwrap(), second.await.unwrap());
        third.await.unwrap();
        assert_eq!(*mixed_order.lock().unwrap(), [0, 1, 2, 3]);

        let nested_runtime = runtime.clone();
        assert_eq!(
            runtime
                .spawn_named("serial", move || nested_runtime.spawn_named("serial", || 42))
                .await
                .unwrap()
                .await
                .unwrap(),
            42
        );
        assert_eq!(runtime.spawn_blocking("blocking", || 17).await.unwrap(), 17);
        assert_eq!(runtime.spawn_dedicated("dedicated", || 23).await.unwrap(), 23);
        assert_eq!(
            runtime.spawn_cpu::<_, ()>("panic", || panic!("bounded job panic")).await,
            Err(TaskError::Panicked)
        );
        assert_eq!(
            runtime.spawn::<_, ()>("panic", async { panic!("cooperative task panic") }).await,
            Err(TaskError::Panicked)
        );
        assert_eq!(
            runtime
                .spawn_named_task::<_, ()>("panic_lane", async { panic!("named task panic") })
                .await,
            Err(TaskError::Panicked)
        );
        // A failed future must not terminate the persistent worker or strand later jobs.
        assert_eq!(runtime.spawn_named("panic_lane", || 29).await.unwrap(), 29);
        assert_eq!(
            runtime
                .spawn_dedicated_task::<_, ()>("panic_service", async {
                    panic!("dedicated task panic")
                })
                .await,
            Err(TaskError::Panicked)
        );

        let (dropped, dropped_rx) = oneshot::channel();
        let guard = NotifyDrop(Some(dropped));
        let canceled = runtime.spawn("cancel", async move {
            let _guard = guard;
            std::future::pending::<()>().await;
        });
        canceled.abort();
        assert_eq!(canceled.await, Err(TaskError::Canceled));
        dropped_rx.await.unwrap();

        let (dropped, dropped_rx) = oneshot::channel();
        let guard = NotifyDrop(Some(dropped));
        drop(
            runtime
                .spawn_named_task("owned_named_service", async move {
                    let _guard = guard;
                    std::future::pending::<()>().await;
                })
                .abort_on_drop(),
        );
        dropped_rx.await.unwrap();
        runtime.spawn_named("owned_named_service", || ()).await.unwrap();

        let (dropped, dropped_rx) = oneshot::channel();
        let guard = NotifyDrop(Some(dropped));
        drop(
            runtime
                .spawn_dedicated_task("owned_service", async move {
                    let _guard = guard;
                    std::future::pending::<()>().await;
                })
                .abort_on_drop(),
        );
        dropped_rx.await.unwrap();

        let (detached_tx, detached_rx) = oneshot::channel();
        drop(runtime.spawn("detached", async move { detached_tx.send(31).unwrap() }));
        assert_eq!(detached_rx.await.unwrap(), 31);

        let (sender, receiver) = runtime.bounded_channel::<usize>(1);
        drop(receiver);
        assert!(sender.send(1).await.is_err());
        values
    }

    struct NotifyDrop(Option<oneshot::Sender<()>>);

    impl Drop for NotifyDrop {
        fn drop(&mut self) {
            let _ = self.0.take().unwrap().send(());
        }
    }

    #[tokio::test]
    async fn production_tasks() {
        let runtime = TaskRuntime::from(Runtime::test());
        let caller = std::thread::current().id();
        let worker = runtime
            .spawn_dedicated_task("dedicated_service", async { std::thread::current().id() })
            .await
            .unwrap();
        assert_ne!(caller, worker);
        exercise(runtime).await;
    }

    #[tokio::test]
    async fn running_job_finishes_after_handle_is_aborted() {
        let runtime = TaskRuntime::from(Runtime::test());
        let (started, started_rx) = oneshot::channel();
        let (release, release_rx) = oneshot::channel();
        let (finished, finished_rx) = oneshot::channel();
        let job = runtime.spawn_blocking("running", move || {
            started.send(()).unwrap();
            release_rx.blocking_recv().unwrap();
            finished.send(()).unwrap();
        });
        started_rx.await.unwrap();
        job.abort();
        assert_eq!(job.await, Err(TaskError::Canceled));
        release.send(()).unwrap();
        finished_rx.await.unwrap();
    }

    #[cfg(feature = "deterministic")]
    #[test]
    fn deterministic_tasks() {
        use commonware_runtime::{deterministic, Runner};

        fn run(seed: u64) -> (String, Vec<usize>) {
            let config = deterministic::Config::default()
                .with_seed(seed)
                .with_timeout(Some(Duration::from_secs(5)));
            deterministic::Runner::new(config).start(|context| async move {
                let runtime = TaskRuntime::deterministic(context.child("execution"));
                let canceled_executed = Arc::new(AtomicBool::new(false));
                let executed = Arc::clone(&canceled_executed);
                let canceled = runtime.spawn_named("canceled_lane", move || {
                    executed.store(true, Ordering::Release);
                });
                canceled.abort();
                assert_eq!(canceled.await, Err(TaskError::Canceled));
                // Completion of the following job proves the canceled queue entry was visited.
                runtime.spawn_named("canceled_lane", || ()).await.unwrap();
                assert!(!canceled_executed.load(Ordering::Acquire));

                let executed = Arc::clone(&canceled_executed);
                drop(
                    runtime
                        .spawn_named("canceled_lane", move || {
                            executed.store(true, Ordering::Release);
                        })
                        .abort_on_drop(),
                );
                runtime.spawn_named("canceled_lane", || ()).await.unwrap();
                assert!(!canceled_executed.load(Ordering::Acquire));

                let values = exercise(runtime).await;
                (context.auditor().state(), values)
            })
        }

        let seeds: Vec<u64> = match std::env::var("RETH_DST_SEED") {
            Ok(seed) => vec![seed.parse().expect("RETH_DST_SEED must be a u64")],
            Err(std::env::VarError::NotPresent) => (0..16).collect(),
            Err(error) => panic!("invalid RETH_DST_SEED: {error}"),
        };
        let mut audits = std::collections::BTreeSet::new();
        for &seed in &seeds {
            eprintln!("runtime DST: seed={seed}");
            let outcome = run(seed);
            assert_eq!(outcome, run(seed), "runtime replay diverged for seed {seed}");
            audits.insert(outcome.0);
        }
        if seeds.len() > 1 {
            assert!(audits.len() > 1, "seeds did not vary task scheduling");
        }

        #[expect(
            clippy::async_yields_async,
            reason = "the handle must outlive the runner to verify executor shutdown"
        )]
        let stopped = deterministic::Runner::default().start(|context| async move {
            TaskRuntime::deterministic(context).spawn("pending", std::future::pending::<()>())
        });
        assert_eq!(stopped.now_or_never(), Some(Err(TaskError::Stopped)));
    }
}
