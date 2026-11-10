//! Compute pools backed by Rayon normally and Shuttle logical threads under `fuzz`.

#[cfg(feature = "fuzz")]
pub use deterministic::*;
#[cfg(not(feature = "fuzz"))]
pub use rayon::{
    current_num_threads, current_thread_index, in_place_scope, join, scope, spawn, spawn_fifo,
    BroadcastContext, Scope, ThreadPool, ThreadPoolBuildError, ThreadPoolBuilder,
};

#[cfg(feature = "fuzz")]
mod deterministic {
    use std::{
        any::Any,
        cell::RefCell,
        fmt,
        panic::{catch_unwind, resume_unwind, AssertUnwindSafe},
        sync::{
            atomic::{AtomicUsize, Ordering},
            Arc, Mutex, OnceLock,
        },
    };

    static NEXT_POOL: AtomicUsize = AtomicUsize::new(1);
    static GLOBAL_POOL: OnceLock<ThreadPool> = OnceLock::new();

    shuttle::thread_local! {
        static CONTEXT: RefCell<Option<WorkerContext>> = RefCell::new(None);
    }

    type Panic = Box<dyn Any + Send + 'static>;
    type PanicHandler = Arc<dyn Fn(Panic) + Send + Sync>;
    type Job<'env> = Box<dyn FnOnce(&Scope<'env>) + Send + 'env>;

    /// A compute pool whose jobs are managed by Shuttle's scheduler.
    #[derive(Clone)]
    pub struct ThreadPool {
        id: usize,
        names: Arc<Vec<Option<String>>>,
        slots: Arc<Mutex<Slots>>,
        panic_handler: Option<PanicHandler>,
    }

    impl ThreadPool {
        /// Returns the number of logical workers configured for this pool.
        pub fn current_num_threads(&self) -> usize {
            self.names.len()
        }

        /// Returns this job's logical index when it belongs to this pool.
        pub fn current_thread_index(&self) -> Option<usize> {
            CONTEXT.with(|context| {
                context
                    .borrow()
                    .as_ref()
                    .filter(|c| c.pool.id == self.id && c.owns_slot)
                    .map(|c| c.index)
            })
        }

        /// Executes inline, waiting for a free logical worker unless already on this pool.
        pub fn install<T: Send>(&self, f: impl FnOnce() -> T + Send) -> T {
            if self.current_thread_index().is_some() {
                f()
            } else {
                without_current_slot(|| self.execute(None, f))
            }
        }

        /// Starts a scheduler-managed job, waiting for pool capacity before executing it.
        pub fn spawn(&self, f: impl FnOnce() + Send + 'static) {
            let pool = self.clone();
            let handle = shuttle::thread::Builder::new()
                .name(self.names[0].clone().unwrap_or_else(|| "compute".into()))
                .spawn(move || {
                    pool.execute(None, || {
                        if let Err(panic) = catch_unwind(AssertUnwindSafe(f)) {
                            if let Some(handler) = &pool.panic_handler {
                                handler(panic);
                            } else {
                                resume_unwind(panic);
                            }
                        }
                    });
                })
                .expect("Shuttle logical thread creation is infallible");
            drop(handle);
        }

        /// Starts a scheduler-managed job; logical scheduling determines execution order.
        pub fn spawn_fifo(&self, f: impl FnOnce() + Send + 'static) {
            self.spawn(f);
        }

        /// Runs one callback per logical worker, exclusively owning each slot until it completes.
        pub fn broadcast<T: Send>(&self, f: impl Fn(BroadcastContext<'_>) -> T + Sync) -> Vec<T> {
            without_current_slot(|| {
                let results = scope_inner(None, |scope| {
                    let (sender, receiver) = shuttle::sync::mpsc::channel();
                    let f = &f;
                    for index in 0..self.current_num_threads() {
                        let sender = sender.clone();
                        scope.spawn(move |_| {
                            let result = self
                                .execute(Some(index), || f(BroadcastContext { pool: self, index }));
                            sender.send((index, result)).unwrap_or_else(|_| {
                                panic!("broadcast receiver outlives callbacks")
                            });
                        });
                    }
                    drop(sender);
                    receiver.into_iter().collect::<Vec<_>>()
                });
                let mut results = results;
                results.sort_unstable_by_key(|(index, _)| *index);
                results.into_iter().map(|(_, result)| result).collect()
            })
        }

        /// Runs the borrowing callback inline; its children exclusively acquire this pool's slots.
        ///
        /// An external caller does not consume pool capacity. A caller already on this pool keeps
        /// its slot while the callback runs, releasing it while draining completed scope work.
        pub fn in_place_scope<'env, T>(&self, f: impl FnOnce(&Scope<'env>) -> T) -> T {
            scope_inner(Some(self.clone()), f)
        }

        /// Executes the scope callback on this pool, draining descendants before unwinding.
        pub fn scope<'env, T: Send>(&self, f: impl FnOnce(&Scope<'env>) -> T + Send) -> T {
            self.install(|| self.in_place_scope(f))
        }

        /// Returns a stable identity for logical worker-local state owned by this pool.
        pub const fn id(&self) -> usize {
            self.id
        }

        fn execute<T>(&self, index: Option<usize>, f: impl FnOnce() -> T) -> T {
            let index = self.acquire(index);
            let _lease = Lease { pool: self.clone(), index };
            with_context(WorkerContext { pool: self.clone(), index, owns_slot: true }, f)
        }

        fn acquire(&self, index: Option<usize>) -> usize {
            loop {
                let current = shuttle::thread::current();
                let mut slots = self.slots.lock().expect("logical slot metadata is not poisoned");
                let available = match index {
                    Some(index) => (!slots.occupied[index]).then_some(index),
                    None => slots.occupied.iter().position(|occupied| !occupied),
                };
                if let Some(index) = available {
                    slots.occupied[index] = true;
                    return index;
                }
                if !slots.waiters.iter().any(|waiter| waiter.id() == current.id()) {
                    slots.waiters.push(current);
                }
                drop(slots);
                shuttle::thread::park();
            }
        }

        fn release(&self, index: usize) {
            let waiters = {
                let mut slots = self.slots.lock().expect("logical slot metadata is not poisoned");
                slots.occupied[index] = false;
                std::mem::take(&mut slots.waiters)
            };
            for waiter in waiters {
                waiter.unpark();
            }
        }
    }

    impl fmt::Debug for ThreadPool {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.debug_struct("ThreadPool")
                .field("num_threads", &self.current_num_threads())
                .finish_non_exhaustive()
        }
    }

    /// Configures a deterministic compute pool without starting native worker threads.
    #[derive(Default)]
    pub struct ThreadPoolBuilder {
        num_threads: usize,
        thread_name: Option<Box<dyn FnMut(usize) -> String + Send>>,
        panic_handler: Option<PanicHandler>,
    }

    impl ThreadPoolBuilder {
        /// Creates a builder with one logical worker by default.
        pub fn new() -> Self {
            Self::default()
        }
        /// Sets the logical worker count; zero selects the deterministic default of one.
        pub const fn num_threads(mut self, threads: usize) -> Self {
            self.num_threads = threads;
            self
        }
        /// Sets the names attached to logical worker jobs.
        pub fn thread_name(mut self, name: impl FnMut(usize) -> String + Send + 'static) -> Self {
            self.thread_name = Some(Box::new(name));
            self
        }
        /// Sets the handler for panics from unscoped jobs.
        pub fn panic_handler(mut self, handler: impl Fn(Panic) + Send + Sync + 'static) -> Self {
            self.panic_handler = Some(Arc::new(handler));
            self
        }
        /// Configures the process-wide logical pool, returning an error if it is already
        /// configured.
        pub fn build_global(self) -> Result<(), ThreadPoolBuildError> {
            GLOBAL_POOL
                .set(self.build()?)
                .map_err(|_| ThreadPoolBuildError::GlobalPoolAlreadyInitialized)
        }
        /// Builds a pool; constructing logical workers cannot fail.
        pub fn build(mut self) -> Result<ThreadPool, ThreadPoolBuildError> {
            let names = (0..self.num_threads.max(1))
                .map(|index| self.thread_name.as_mut().map(|name| name(index)))
                .collect();
            Ok(ThreadPool {
                id: NEXT_POOL.fetch_add(1, Ordering::Relaxed),
                names: Arc::new(names),
                slots: Arc::new(Mutex::new(Slots {
                    occupied: vec![false; self.num_threads.max(1)],
                    waiters: Vec::new(),
                })),
                panic_handler: self.panic_handler,
            })
        }
    }

    impl fmt::Debug for ThreadPoolBuilder {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.debug_struct("ThreadPoolBuilder")
                .field("num_threads", &self.num_threads)
                .finish_non_exhaustive()
        }
    }

    /// A compute pool could not be installed as the process-wide pool.
    #[derive(Debug)]
    pub enum ThreadPoolBuildError {
        /// The global pool was already configured or used.
        GlobalPoolAlreadyInitialized,
    }
    impl fmt::Display for ThreadPoolBuildError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("the global compute pool is already initialized")
        }
    }
    impl std::error::Error for ThreadPoolBuildError {}

    fn global_pool() -> ThreadPool {
        GLOBAL_POOL
            .get_or_init(|| {
                ThreadPoolBuilder::new().build().expect("logical pool construction is infallible")
            })
            .clone()
    }

    /// Identifies the logical worker executing a broadcast callback.
    #[derive(Debug)]
    pub struct BroadcastContext<'pool> {
        pool: &'pool ThreadPool,
        index: usize,
    }
    impl BroadcastContext<'_> {
        /// Returns the callback's distinct zero-based worker index.
        pub const fn index(&self) -> usize {
            self.index
        }
        /// Returns the total number of callbacks in the broadcast.
        pub fn num_threads(&self) -> usize {
            self.pool.current_num_threads()
        }
    }

    /// A borrowing scope that keeps all submitted jobs alive until they have finished.
    pub struct Scope<'env> {
        sender: shuttle::sync::mpsc::Sender<Message<'env>>,
    }
    impl<'env> Scope<'env> {
        /// Submits a borrowing job; nested submissions are included in the scope's drain.
        pub fn spawn(&self, f: impl FnOnce(&Self) + Send + 'env) {
            self.sender
                .send(Message::Job(Box::new(f)))
                .unwrap_or_else(|_| panic!("scope dispatcher outlives its jobs"));
        }
    }
    impl fmt::Debug for Scope<'_> {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.debug_struct("Scope").finish_non_exhaustive()
        }
    }

    enum Message<'env> {
        Job(Job<'env>),
        Done(Result<(), Panic>),
    }
    // Metadata guards never span a scheduler operation; contended jobs park only after unlocking.
    struct Slots {
        occupied: Vec<bool>,
        waiters: Vec<shuttle::thread::Thread>,
    }
    struct Lease {
        pool: ThreadPool,
        index: usize,
    }
    impl Drop for Lease {
        fn drop(&mut self) {
            self.pool.release(self.index);
        }
    }
    #[derive(Clone)]
    struct WorkerContext {
        pool: ThreadPool,
        index: usize,
        owns_slot: bool,
    }
    struct RestoreContext(Option<WorkerContext>);
    impl Drop for RestoreContext {
        fn drop(&mut self) {
            CONTEXT.with(|context| context.replace(self.0.take()));
        }
    }
    fn with_context<T>(context: WorkerContext, f: impl FnOnce() -> T) -> T {
        let previous = CONTEXT.with(|slot| slot.replace(Some(context)));
        let _restore = RestoreContext(previous);
        f()
    }
    struct ResumeSlot(WorkerContext);
    impl Drop for ResumeSlot {
        fn drop(&mut self) {
            self.0.pool.acquire(Some(self.0.index));
            CONTEXT.with(|context| {
                context
                    .borrow_mut()
                    .as_mut()
                    .expect("suspended context remains installed")
                    .owns_slot = true;
            });
        }
    }
    fn without_current_slot<T>(f: impl FnOnce() -> T) -> T {
        let context = CONTEXT.with(|slot| {
            let mut slot = slot.borrow_mut();
            slot.as_mut().filter(|context| context.owns_slot).map(|context| {
                context.owns_slot = false;
                context.clone()
            })
        });
        let _resume = context.map(|context| {
            context.pool.release(context.index);
            ResumeSlot(context)
        });
        f()
    }
    fn current_pool() -> Option<ThreadPool> {
        CONTEXT.with(|context| context.borrow().as_ref().map(|context| context.pool.clone()))
    }

    /// Runs a scope on the caller and drains all spawned jobs, including when the caller panics.
    pub fn in_place_scope<'env, T>(f: impl FnOnce(&Scope<'env>) -> T) -> T {
        scope_inner(Some(current_pool().unwrap_or_else(global_pool)), f)
    }

    fn scope_inner<'env, T>(pool: Option<ThreadPool>, f: impl FnOnce(&Scope<'env>) -> T) -> T {
        let (sender, receiver) = shuttle::sync::mpsc::channel();
        let scope = Scope { sender };
        let (result, child_panic) = shuttle::thread::scope(|threads| {
            let scope = &scope;
            let dispatcher = threads.spawn(move || {
                let mut running = 1usize;
                let mut first_panic = None;
                while running != 0 {
                    match receiver.recv().expect("scope sender outlives dispatcher") {
                        Message::Job(job) => {
                            running += 1;
                            let pool = pool.clone();
                            threads.spawn(move || {
                                let result = catch_unwind(AssertUnwindSafe(|| {
                                    if let Some(pool) = pool {
                                        pool.execute(None, || job(scope));
                                    } else {
                                        job(scope);
                                    }
                                }));
                                scope
                                    .sender
                                    .send(Message::Done(result))
                                    .unwrap_or_else(|_| panic!("scope dispatcher drains all jobs"));
                            });
                        }
                        Message::Done(result) => {
                            running -= 1;
                            if first_panic.is_none() {
                                first_panic = result.err();
                            }
                        }
                    }
                }
                first_panic
            });
            let result = catch_unwind(AssertUnwindSafe(|| f(scope)));
            scope
                .sender
                .send(Message::Done(Ok(())))
                .unwrap_or_else(|_| panic!("scope dispatcher drains root completion"));
            let panic = without_current_slot(|| {
                dispatcher.join().unwrap_or_else(|panic| resume_unwind(panic))
            });
            (result, panic)
        });
        match result {
            Err(panic) => resume_unwind(panic),
            Ok(value) => {
                if let Some(panic) = child_panic {
                    resume_unwind(panic);
                }
                value
            }
        }
    }

    /// Runs the callback on the current or global pool and waits for all descendants.
    pub fn scope<'env, T: Send>(f: impl FnOnce(&Scope<'env>) -> T + Send) -> T {
        current_pool().unwrap_or_else(global_pool).scope(f)
    }
    /// Returns the current pool size, or the configured global pool size outside a pool.
    pub fn current_num_threads() -> usize {
        current_pool().unwrap_or_else(global_pool).current_num_threads()
    }
    /// Returns the current pool identity for indexing logical worker-local state.
    pub fn current_pool_id() -> Option<usize> {
        CONTEXT.with(|context| context.borrow().as_ref().filter(|c| c.owns_slot).map(|c| c.pool.id))
    }
    /// Returns this managed job's logical worker index, or `None` outside a pool.
    pub fn current_thread_index() -> Option<usize> {
        CONTEXT.with(|context| context.borrow().as_ref().filter(|c| c.owns_slot).map(|c| c.index))
    }
    /// Starts a Shuttle job, acquiring current-pool capacity when called from a pool.
    pub fn spawn(f: impl FnOnce() + Send + 'static) {
        current_pool().unwrap_or_else(global_pool).spawn(f);
    }
    /// Starts an unscoped job through Shuttle; its scheduler selects execution order.
    pub fn spawn_fifo(f: impl FnOnce() + Send + 'static) {
        spawn(f);
    }
    /// Evaluates two borrowing computations and waits for both before propagating a panic.
    pub fn join<A: Send, B: Send>(
        a: impl FnOnce() -> A + Send,
        b: impl FnOnce() -> B + Send,
    ) -> (A, B) {
        let (sender, receiver) = shuttle::sync::mpsc::channel();
        let b = in_place_scope(|scope| {
            scope.spawn(move |_| {
                sender.send(a()).unwrap_or_else(|_| panic!("join receiver outlives producer"));
            });
            b()
        });
        (receiver.recv().expect("join producer completed before scope returned"), b)
    }
}
