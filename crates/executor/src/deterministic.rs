//! Private virtual-time driver around Shuttle's fixed scheduler.

use crate::time::{Duration, Instant};
use shuttle::scheduler::{RoundRobinScheduler, Schedule, Scheduler, Task, TaskId};
use std::{
    cell::RefCell,
    rc::Rc,
    sync::{Arc, Mutex},
};

std::thread_local! {
    static CLOCK: RefCell<Option<Rc<RefCell<Clock>>>> = const { RefCell::new(None) };
}

#[derive(Default)]
struct Clock {
    now: Duration,
    timers: Vec<(u64, Instant, shuttle::thread::Thread)>,
    next_timer: u64,
    driver: Option<shuttle::thread::Thread>,
    stopped: bool,
}

struct ClockScheduler {
    scheduler: RoundRobinScheduler,
    clock: Rc<RefCell<Clock>>,
}

impl Scheduler for ClockScheduler {
    fn new_execution(&mut self) -> Option<Schedule> {
        self.scheduler.new_execution()
    }

    fn next_task(
        &mut self,
        tasks: &[&Task],
        current: Option<TaskId>,
        yielding: bool,
    ) -> Option<TaskId> {
        // Shuttle includes spuriously wakeable parked tasks in its candidates.
        // Blocking executor waits wake only through their registered notifications.
        let runnable: Vec<_> = tasks.iter().copied().filter(|task| task.runnable()).collect();
        assert!(!runnable.is_empty(), "simulated workload deadlocked without a pending timer");
        let tasks = &runnable;
        let mut clock = self.clock.borrow_mut();
        if tasks.len() == 1 &&
            clock
                .driver
                .as_ref()
                .is_some_and(|driver| usize::from(driver.id()) == usize::from(tasks[0].id())) &&
            let Some(deadline) = clock.timers.iter().map(|(_, deadline, _)| *deadline).min()
        {
            clock.now = clock.now.max(deadline.0);
        }
        drop(clock);
        self.scheduler.next_task(tasks, current, yielding)
    }

    fn next_u64(&mut self) -> u64 {
        self.scheduler.next_u64()
    }
}

fn with_clock<T>(f: impl FnOnce(&mut Clock) -> T) -> T {
    CLOCK.with(|slot| {
        let clock =
            slot.borrow().as_ref().expect("simulated primitives require Runtime::run").clone();
        f(&mut clock.borrow_mut())
    })
}

pub(crate) fn now() -> Duration {
    with_clock(|clock| clock.now)
}

pub(crate) struct Timer(u64);

impl Timer {
    pub(crate) fn new(deadline: Instant) -> Self {
        let current = shuttle::thread::current();
        let (id, driver) = with_clock(|clock| {
            assert!(
                !clock.stopped,
                "simulated work used a timer after Runtime::run's workload returned; finish or join spawned work inside the workload"
            );
            let id = clock.next_timer;
            clock.next_timer += 1;
            clock.timers.push((id, deadline, current));
            (id, clock.driver.clone())
        });
        if let Some(driver) = driver {
            driver.unpark();
        }
        Self(id)
    }
}

impl Drop for Timer {
    fn drop(&mut self) {
        with_clock(|clock| clock.timers.retain(|(id, _, _)| *id != self.0));
    }
}

fn drive_clock() {
    with_clock(|clock| clock.driver = Some(shuttle::thread::current()));
    loop {
        let (stopped, pending, ready) = with_clock(|clock| {
            let mut ready = Vec::new();
            clock.timers.retain(|(_, deadline, thread)| {
                if deadline.0 <= clock.now {
                    ready.push(thread.clone());
                    false
                } else {
                    true
                }
            });
            (clock.stopped, !clock.timers.is_empty(), ready)
        });
        if stopped {
            return;
        }
        for thread in ready {
            thread.unpark();
        }
        if pending {
            shuttle::thread::yield_now();
        } else {
            shuttle::thread::park();
        }
    }
}

pub(crate) fn run<F, T>(work: F) -> T
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    let clock = Rc::new(RefCell::new(Clock::default()));
    CLOCK.with(|slot| {
        assert!(slot.borrow().is_none(), "Runtime::run cannot be nested");
        slot.replace(Some(clock.clone()));
    });
    let reset = ResetClock;
    let work = Mutex::new(Some(work));
    let result = Arc::new(Mutex::new(None));
    let output = result.clone();
    let scheduler = ClockScheduler { scheduler: RoundRobinScheduler::new(1), clock };
    shuttle::Runner::new(scheduler, shuttle::Config::default()).run(move || {
        let driver = shuttle::thread::spawn(drive_clock);
        let work = work
            .lock()
            .expect("work holder is not poisoned")
            .take()
            .expect("one execution");
        let value = work();
        with_clock(|clock| {
            assert!(
                clock.timers.is_empty(),
                "Runtime::run's workload returned with pending timers; finish or join spawned work inside the workload"
            );
            clock.stopped = true;
        });
        driver.thread().unpark();
        driver.join().expect("virtual clock driver panicked");
        *output.lock().expect("output holder is not poisoned") = Some(value);
    });
    drop(reset);
    result.lock().expect("output holder is not poisoned").take().expect("execution completed")
}

struct ResetClock;

impl Drop for ResetClock {
    fn drop(&mut self) {
        CLOCK.with(|slot| slot.replace(None));
    }
}

/// A one-time initialized value whose waiting callers remain scheduler-managed.
#[derive(Debug)]
pub struct OnceLock<T> {
    value: std::sync::OnceLock<T>,
    initializing: shuttle::sync::Mutex<()>,
    ready: shuttle::sync::Condvar,
}

impl<T> OnceLock<T> {
    /// Creates an uninitialized cell.
    pub const fn new() -> Self {
        Self {
            value: std::sync::OnceLock::new(),
            initializing: shuttle::sync::Mutex::new(()),
            ready: shuttle::sync::Condvar::new(),
        }
    }
    /// Returns the value if initialization has completed without blocking.
    pub fn get(&self) -> Option<&T> {
        self.value.get()
    }
    /// Blocks the logical thread until initialization completes.
    pub fn wait(&self) -> &T {
        let mut guard = self.initializing.lock().expect("OnceLock initializer panicked");
        while self.value.get().is_none() {
            guard = self.ready.wait(guard).expect("OnceLock initializer panicked");
        }
        drop(guard);
        self.value.get().expect("initialized cell")
    }
    /// Initializes the cell once, or returns `value` when already initialized.
    pub fn set(&self, value: T) -> Result<(), T> {
        let guard = self.initializing.lock().expect("OnceLock initializer panicked");
        let result = self.value.set(value);
        self.ready.notify_all();
        drop(guard);
        result
    }
    /// Returns the value, initializing it with `initialize` if necessary.
    pub fn get_or_init(&self, initialize: impl FnOnce() -> T) -> &T {
        if let Some(value) = self.value.get() {
            return value;
        }
        let guard = self.initializing.lock().expect("OnceLock initializer panicked");
        let value = self.value.get_or_init(initialize);
        self.ready.notify_all();
        drop(guard);
        value
    }
    /// Returns a mutable value when initialized, without blocking.
    pub fn get_mut(&mut self) -> Option<&mut T> {
        self.value.get_mut()
    }
    /// Removes the initialized value, leaving the cell uninitialized.
    pub fn take(&mut self) -> Option<T> {
        self.value.take()
    }
    /// Consumes the cell and returns its initialized value.
    pub fn into_inner(self) -> Option<T> {
        self.value.into_inner()
    }
}
impl<T> Default for OnceLock<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> From<T> for OnceLock<T> {
    fn from(value: T) -> Self {
        Self {
            value: std::sync::OnceLock::from(value),
            initializing: shuttle::sync::Mutex::new(()),
            ready: shuttle::sync::Condvar::new(),
        }
    }
}

/// Non-poisoning locks with parking_lot-compatible locking methods.
pub mod parking_lot {
    pub use shuttle::sync::{MutexGuard, RwLockReadGuard, RwLockWriteGuard};

    /// A non-poisoning mutex managed by the simulation scheduler.
    #[derive(Debug)]
    pub struct Mutex<T>(shuttle::sync::Mutex<T>);
    impl<T> Mutex<T> {
        /// Creates a mutex containing `value`.
        pub const fn new(value: T) -> Self {
            Self(shuttle::sync::Mutex::new(value))
        }
        /// Acquires the mutex, blocking only the current logical thread.
        pub fn lock(&self) -> MutexGuard<'_, T> {
            self.0.lock().unwrap_or_else(|error| error.into_inner())
        }
        /// Attempts to acquire the mutex without waiting.
        pub fn try_lock(&self) -> Option<MutexGuard<'_, T>> {
            match self.0.try_lock() {
                Ok(guard) => Some(guard),
                Err(shuttle::sync::TryLockError::Poisoned(error)) => Some(error.into_inner()),
                Err(shuttle::sync::TryLockError::WouldBlock) => None,
            }
        }
        /// Consumes the mutex, returning its value.
        pub fn into_inner(self) -> T {
            self.0.into_inner().unwrap_or_else(|error| error.into_inner())
        }
        /// Returns exclusive access without locking.
        pub fn get_mut(&mut self) -> &mut T {
            self.0.get_mut().unwrap_or_else(|error| error.into_inner())
        }
    }
    impl<T: Default> Default for Mutex<T> {
        fn default() -> Self {
            Self::new(T::default())
        }
    }
    impl<T> From<T> for Mutex<T> {
        fn from(value: T) -> Self {
            Self::new(value)
        }
    }

    /// A non-poisoning reader-writer lock managed by the simulation scheduler.
    #[derive(Debug)]
    pub struct RwLock<T>(shuttle::sync::RwLock<T>);
    impl<T> RwLock<T> {
        /// Creates a reader-writer lock containing `value`.
        pub const fn new(value: T) -> Self {
            Self(shuttle::sync::RwLock::new(value))
        }
        /// Acquires shared access, blocking only the current logical thread.
        pub fn read(&self) -> RwLockReadGuard<'_, T> {
            self.0.read().unwrap_or_else(|error| error.into_inner())
        }
        /// Acquires exclusive access, blocking only the current logical thread.
        pub fn write(&self) -> RwLockWriteGuard<'_, T> {
            self.0.write().unwrap_or_else(|error| error.into_inner())
        }
        /// Consumes the lock, returning its value.
        pub fn into_inner(self) -> T {
            self.0.into_inner().unwrap_or_else(|error| error.into_inner())
        }
        /// Returns exclusive access without locking.
        pub fn get_mut(&mut self) -> &mut T {
            self.0.get_mut().unwrap_or_else(|error| error.into_inner())
        }
    }
    impl<T: Default> Default for RwLock<T> {
        fn default() -> Self {
            Self::new(T::default())
        }
    }
    impl<T> From<T> for RwLock<T> {
        fn from(value: T) -> Self {
            Self::new(value)
        }
    }
}

/// A condition variable whose timeout waits use the virtual clock.
#[derive(Debug)]
pub struct Condvar(shuttle::sync::Condvar);

/// Whether a virtual condition-variable wait reached its deadline.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WaitTimeoutResult(bool);
impl WaitTimeoutResult {
    /// Returns `true` when the timeout notification fired.
    pub const fn timed_out(&self) -> bool {
        self.0
    }
}

impl Condvar {
    /// Creates a condition variable.
    pub const fn new() -> Self {
        Self(shuttle::sync::Condvar::new())
    }
    /// Wakes one waiting logical thread.
    pub fn notify_one(&self) {
        self.0.notify_one();
    }
    /// Wakes every waiting logical thread.
    pub fn notify_all(&self) {
        self.0.notify_all();
    }
    /// Releases the mutex and blocks until notified, then reacquires it.
    pub fn wait<'a, T>(
        &self,
        guard: shuttle::sync::MutexGuard<'a, T>,
    ) -> shuttle::sync::LockResult<shuttle::sync::MutexGuard<'a, T>> {
        self.0.wait(guard)
    }
    /// Waits while `condition` returns true with the mutex held.
    pub fn wait_while<'a, T, F>(
        &self,
        guard: shuttle::sync::MutexGuard<'a, T>,
        condition: F,
    ) -> shuttle::sync::LockResult<shuttle::sync::MutexGuard<'a, T>>
    where
        F: FnMut(&mut T) -> bool,
    {
        self.0.wait_while(guard, condition)
    }
    /// Waits for a notification or a virtual timeout, then reacquires the mutex.
    ///
    /// Other waiters may wake spuriously when one waiter's timeout fires.
    pub fn wait_timeout<'a, T>(
        &self,
        guard: shuttle::sync::MutexGuard<'a, T>,
        duration: Duration,
    ) -> shuttle::sync::LockResult<(shuttle::sync::MutexGuard<'a, T>, WaitTimeoutResult)> {
        if duration.is_zero() {
            return Ok((guard, WaitTimeoutResult(true)));
        }
        let timed_out = std::sync::atomic::AtomicBool::new(false);
        shuttle::thread::scope(|scope| {
            let (cancel, cancelled) = crate::channel::bounded::<()>(1);
            let timed_out = &timed_out;
            let timer = scope.spawn(move || match cancelled.recv_timeout(duration) {
                Err(crate::channel::RecvTimeoutError::Timeout) => {
                    timed_out.store(true, std::sync::atomic::Ordering::Relaxed);
                    self.0.notify_all();
                }
                Ok(()) | Err(crate::channel::RecvTimeoutError::Disconnected) => {}
            });
            let result = self.0.wait(guard);
            match cancel.try_send(()) {
                Ok(()) | Err(crate::channel::TrySendError::Disconnected(())) => {}
                Err(crate::channel::TrySendError::Full(())) => {
                    unreachable!("timeout cancellation is sent once")
                }
            }
            timer.join().expect("condition-variable timeout notifier panicked");
            let timeout = WaitTimeoutResult(timed_out.load(std::sync::atomic::Ordering::Relaxed));
            result
                .map(|guard| (guard, timeout))
                .map_err(|error| shuttle::sync::PoisonError::new((error.into_inner(), timeout)))
        })
    }
    /// Waits until `condition` becomes false or the total virtual timeout expires.
    pub fn wait_timeout_while<'a, T, F>(
        &self,
        mut guard: shuttle::sync::MutexGuard<'a, T>,
        duration: Duration,
        mut condition: F,
    ) -> shuttle::sync::LockResult<(shuttle::sync::MutexGuard<'a, T>, WaitTimeoutResult)>
    where
        F: FnMut(&mut T) -> bool,
    {
        let deadline = Instant::now() + duration;
        while condition(&mut guard) {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let (next, timeout) = self.wait_timeout(guard, remaining)?;
            guard = next;
            if timeout.timed_out() {
                return Ok((guard, timeout));
            }
        }
        Ok((guard, WaitTimeoutResult(false)))
    }
}
impl Default for Condvar {
    fn default() -> Self {
        Self::new()
    }
}
