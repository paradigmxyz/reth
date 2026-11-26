//! Private virtual-time driver around Shuttle's fixed scheduler.

#![allow(
    clippy::disallowed_types,
    clippy::disallowed_methods,
    reason = "Native TLS shares scheduler state across logical tasks; native locks only hand work and results across the runner boundary."
)]

use crate::sync::time::Instant;
use shuttle::{
    scheduler::{RoundRobinScheduler, Schedule, Scheduler, Task, TaskId},
    sync::{self, Condvar},
    thread, Config, FailurePersistence, MaxSteps, Runner,
};
use std::{
    cell::RefCell,
    collections::HashMap,
    rc::Rc,
    sync::{Arc, Mutex},
    thread_local,
};

// Provider and EVM workloads need more than Shuttle's 60 KiB default coroutine stack.
const DEFAULT_STACK_SIZE: usize = 8 * 1024 * 1024;

// Native TLS is shared by Shuttle's logical tasks but isolated between host threads. ResetClock
// drops the execution's timers and pool waiters after the runner finishes, including on unwind.
thread_local! {
    static CLOCK: RefCell<Option<Rc<RefCell<Clock>>>> = const { RefCell::new(None) };
}

/// A private notification token, independent of the caller's public thread park token.
#[derive(Clone, Debug)]
pub(super) struct Waiter {
    id: thread::ThreadId,
    // No user callbacks touch this flag; unrelated logical-task unwinding can poison Shuttle's
    // native mutex, so private notification operations recover its still-valid boolean.
    state: Arc<(sync::Mutex<bool>, Condvar)>,
}

impl Waiter {
    pub(super) fn new() -> Self {
        Self {
            id: thread::current().id(),
            state: Arc::new((sync::Mutex::new(false), Condvar::new())),
        }
    }

    pub(super) const fn id(&self) -> thread::ThreadId {
        self.id
    }

    pub(super) fn wait(&self) {
        let (notified, ready) = &*self.state;
        let mut notified = notified.lock().unwrap_or_else(|error| error.into_inner());
        while !*notified {
            notified = ready.wait(notified).unwrap_or_else(|error| error.into_inner());
        }
        *notified = false;
    }

    pub(super) fn notify(&self) {
        let (notified, ready) = &*self.state;
        *notified.lock().unwrap_or_else(|error| error.into_inner()) = true;
        ready.notify_one();
    }
}

pub(crate) struct Timer(u64);

impl Timer {
    pub(crate) fn new(deadline: Instant, waiter: Waiter) -> Self {
        let (id, driver) = with_clock(|clock| {
            assert!(
                !clock.stopped,
                "simulated work used a timer after Runtime::run's workload returned; finish or join spawned work inside the workload"
            );
            let id = clock.next_timer;
            clock.next_timer += 1;
            clock.timers.push(TimerEntry { id, deadline, waiter });
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
        with_clock(|clock| clock.timers.retain(|timer| timer.id != self.0));
    }
}

#[derive(Default)]
struct Clock {
    now: i128,
    timers: Vec<TimerEntry>,
    next_timer: u64,
    driver: Option<thread::Thread>,
    stopped: bool,
    waking_timers: bool,
    pools: HashMap<usize, Slots>,
    // Construct before user tasks: Shuttle's lazy-static Once can spuriously poison on unwind.
    rendezvous_selection: Rc<super::Mutex<()>>,
}

// Slot metadata is shared by every logical task in this execution, not by retained pool handles.
// Borrows never span a scheduler operation or user callback.
pub(super) struct Slots {
    pub(super) occupied: Vec<bool>,
    pub(super) waiters: Vec<Waiter>,
}

struct TimerEntry {
    id: u64,
    deadline: Instant,
    waiter: Waiter,
}

struct DriverState {
    stopped: bool,
    pending: bool,
    ready: Vec<Waiter>,
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
        if !clock.waking_timers &&
            tasks.len() == 1 &&
            clock
                .driver
                .as_ref()
                .is_some_and(|driver| usize::from(driver.id()) == usize::from(tasks[0].id())) &&
            let Some(deadline) = clock.timers.iter().map(|timer| timer.deadline).min()
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

struct ResetClock;

impl Drop for ResetClock {
    fn drop(&mut self) {
        CLOCK.with(|slot| slot.replace(None));
    }
}

pub(crate) fn now() -> i128 {
    with_clock(|clock| clock.now)
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
    let mut config = Config::default();
    config.stack_size = DEFAULT_STACK_SIZE;
    // Fixed round-robin executions can be replayed by rerunning the workload; do not clone and
    // print Shuttle's entire retained schedule on failure. Shuttle still records every step.
    config.failure_persistence = FailurePersistence::None;
    // Runtime workloads are not bounded-size Shuttle tests.
    config.max_steps = MaxSteps::None;
    Runner::new(scheduler, config).run(move || {
        let driver = thread::spawn(drive_clock);
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

fn with_clock<T>(f: impl FnOnce(&mut Clock) -> T) -> T {
    CLOCK.with(|slot| {
        let clock =
            slot.borrow().as_ref().expect("simulated primitives require Runtime::run").clone();
        f(&mut clock.borrow_mut())
    })
}

fn drive_clock() {
    with_clock(|clock| clock.driver = Some(thread::current()));
    loop {
        let state = with_clock(|clock| {
            let mut ready = Vec::new();
            clock.timers.retain(|timer| {
                if timer.deadline.0 <= clock.now {
                    ready.push(timer.waiter.clone());
                    false
                } else {
                    true
                }
            });
            // A notification schedules before making its waiter runnable. Keep the clock frozen
            // until every due waiter has actually been notified.
            clock.waking_timers = !ready.is_empty();
            DriverState { stopped: clock.stopped, pending: !clock.timers.is_empty(), ready }
        });
        if state.stopped {
            return;
        }
        for waiter in state.ready {
            waiter.notify();
        }
        with_clock(|clock| clock.waking_timers = false);
        if state.pending {
            thread::yield_now();
        } else {
            thread::park();
        }
    }
}

/// Borrows this execution's capacity and waiters for a process-reusable pool identity.
pub(super) fn with_pool_slots<T>(id: usize, threads: usize, f: impl FnOnce(&mut Slots) -> T) -> T {
    with_clock(|clock| {
        f(clock
            .pools
            .entry(id)
            .or_insert_with(|| Slots { occupied: vec![false; threads], waiters: Vec::new() }))
    })
}

/// Clones the execution's arbiter before locking so no clock borrow spans a scheduling point.
pub(super) fn rendezvous_selection() -> Rc<super::Mutex<()>> {
    with_clock(|clock| clock.rendezvous_selection.clone())
}
