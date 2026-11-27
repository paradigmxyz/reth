//! Condition-variable notifications and deadlines on the simulated clock.

use super::{
    deterministic::{Timer, Waiter},
    time::{Duration, Instant},
    LockResult, MutexGuard, PoisonError,
};
use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};

/// A simulated condition variable whose timed waits use the runtime's virtual clock.
#[derive(Debug, Default)]
pub struct Condvar {
    // No scheduler operation or user callback runs while this native bookkeeping lock is held.
    waiters: Mutex<VecDeque<Arc<CondvarWaiter>>>,
}

#[derive(Debug)]
struct CondvarWaiter {
    wake: Waiter,
    notified: AtomicBool,
}

struct Registration<'a> {
    condvar: &'a Condvar,
    waiter: Arc<CondvarWaiter>,
}

impl Condvar {
    /// Creates a condition variable with no waiters.
    pub const fn new() -> Self {
        Self { waiters: Mutex::new(VecDeque::new()) }
    }

    /// Releases the guard and waits for notification, then reacquires the mutex.
    ///
    /// A poisoned mutex is returned in the error, with its guard held.
    pub fn wait<'a, T>(&self, guard: MutexGuard<'a, T>) -> LockResult<MutexGuard<'a, T>> {
        self.wait_until(guard, None)
            .map(|(guard, _)| guard)
            .map_err(|error| PoisonError::new(error.into_inner().0))
    }

    /// Waits and reacquires the mutex until `condition` returns false, propagating poison.
    pub fn wait_while<'a, T, F>(
        &self,
        mut guard: MutexGuard<'a, T>,
        mut condition: F,
    ) -> LockResult<MutexGuard<'a, T>>
    where
        F: FnMut(&mut T) -> bool,
    {
        while condition(&mut guard) {
            guard = self.wait(guard)?;
        }
        Ok(guard)
    }

    /// Waits for notification or `duration` on the virtual clock and reacquires the mutex.
    ///
    /// The result reports whether the deadline elapsed without notification. Poison errors
    /// contain the same guard and timeout result that success would return.
    pub fn wait_timeout<'a, T>(
        &self,
        guard: MutexGuard<'a, T>,
        duration: Duration,
    ) -> LockResult<(MutexGuard<'a, T>, WaitTimeoutResult)> {
        self.wait_until(guard, Instant::now().checked_add(duration))
    }

    /// Waits until `condition` is false or the original virtual deadline expires.
    ///
    /// Notifications do not restart the timeout. A false condition returns a non-timeout result,
    /// including when it becomes false while reacquiring the mutex after the deadline.
    pub fn wait_timeout_while<'a, T, F>(
        &self,
        mut guard: MutexGuard<'a, T>,
        duration: Duration,
        mut condition: F,
    ) -> LockResult<(MutexGuard<'a, T>, WaitTimeoutResult)>
    where
        F: FnMut(&mut T) -> bool,
    {
        let deadline = Instant::now().checked_add(duration);
        loop {
            if !condition(&mut guard) {
                return Ok((guard, WaitTimeoutResult(false)));
            }
            if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
                return Ok((guard, WaitTimeoutResult(true)));
            }
            guard = self.wait_until(guard, deadline)?.0;
        }
    }

    /// Wakes one currently registered waiter; notifications are not buffered.
    pub fn notify_one(&self) {
        let waiter = self.waiters.lock().expect("condvar waiters are not poisoned").pop_front();
        if let Some(waiter) = waiter {
            waiter.notified.store(true, Ordering::Relaxed);
            waiter.wake.notify();
        }
    }

    /// Wakes all currently registered waiters; notifications are not buffered.
    pub fn notify_all(&self) {
        let waiters =
            std::mem::take(&mut *self.waiters.lock().expect("condvar waiters are not poisoned"));
        // Mark all waiters before notification can switch to a newly arriving waiter.
        for waiter in &waiters {
            waiter.notified.store(true, Ordering::Relaxed);
        }
        for waiter in waiters {
            waiter.wake.notify();
        }
    }

    fn wait_until<'a, T>(
        &self,
        guard: MutexGuard<'a, T>,
        deadline: Option<Instant>,
    ) -> LockResult<(MutexGuard<'a, T>, WaitTimeoutResult)> {
        let waiter =
            Arc::new(CondvarWaiter { wake: Waiter::new(), notified: AtomicBool::new(false) });
        self.waiters.lock().expect("condvar waiters are not poisoned").push_back(waiter.clone());
        let registration = Registration { condvar: self, waiter };
        let timer = deadline.map(|deadline| Timer::new(deadline, registration.waiter.wake.clone()));
        // Register before unlocking: releasing a simulated mutex is a scheduling point.
        let mutex = guard.unlock();
        let timed_out = loop {
            if registration.waiter.notified.load(Ordering::Relaxed) {
                break false;
            }
            if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
                break true;
            }
            registration.waiter.wake.wait();
        };
        drop(registration);
        drop(timer);
        mutex
            .lock()
            .map(|guard| (guard, WaitTimeoutResult(timed_out)))
            .map_err(|error| PoisonError::new((error.into_inner(), WaitTimeoutResult(timed_out))))
    }
}

impl Drop for Registration<'_> {
    fn drop(&mut self) {
        self.condvar
            .waiters
            .lock()
            .expect("condvar waiters are not poisoned")
            .retain(|waiter| !Arc::ptr_eq(waiter, &self.waiter));
    }
}

/// Indicates whether a timed condition-variable wait expired without notification.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WaitTimeoutResult(bool);

impl WaitTimeoutResult {
    /// Returns true only when this wait's deadline expired without notification.
    pub const fn timed_out(&self) -> bool {
        self.0
    }
}
