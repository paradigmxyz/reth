//! Private virtual-time driver around Shuttle's fixed scheduler.

use crate::executor::time::{Duration, Instant};
use shuttle::{
    scheduler::{RoundRobinScheduler, Schedule, Scheduler, Task, TaskId},
    thread, Config, MaxSteps, Runner,
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

pub(crate) struct Timer(u64);

impl Timer {
    pub(crate) fn new(deadline: Instant) -> Self {
        let current = thread::current();
        let (id, driver) = with_clock(|clock| {
            assert!(
                !clock.stopped,
                "simulated work used a timer after Runtime::run's workload returned; finish or join spawned work inside the workload"
            );
            let id = clock.next_timer;
            clock.next_timer += 1;
            clock.timers.push(TimerEntry { id, deadline, thread: current });
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
    now: Duration,
    timers: Vec<TimerEntry>,
    next_timer: u64,
    driver: Option<thread::Thread>,
    stopped: bool,
    pools: HashMap<usize, Slots>,
}

// Slot metadata is shared by every logical task in this execution, not by retained pool handles.
// Borrows never span a scheduler operation or user callback.
pub(super) struct Slots {
    pub(super) occupied: Vec<bool>,
    pub(super) waiters: Vec<thread::Thread>,
}

struct TimerEntry {
    id: u64,
    deadline: Instant,
    thread: thread::Thread,
}

struct DriverState {
    stopped: bool,
    pending: bool,
    ready: Vec<thread::Thread>,
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

pub(crate) fn now() -> Duration {
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
                    ready.push(timer.thread.clone());
                    false
                } else {
                    true
                }
            });
            DriverState { stopped: clock.stopped, pending: !clock.timers.is_empty(), ready }
        });
        if state.stopped {
            return;
        }
        for thread in state.ready {
            thread.unpark();
        }
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
