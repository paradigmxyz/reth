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
