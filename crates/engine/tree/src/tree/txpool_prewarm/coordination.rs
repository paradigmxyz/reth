use crate::tree::TxPoolPrewarmCacheSnapshot as Snapshot;
use alloy_primitives::B256;
use parking_lot::{Condvar, Mutex};
use std::{
    fmt::Debug,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Weak,
    },
    time::Duration,
};

/// Shared synchronization state for the txpool prewarming worker.
pub(super) struct Coordinator<J> {
    state: Mutex<State<J>>,
    /// Condition variable paired with the `state` mutex.
    ///
    /// Waiters evaluate their predicates while holding `state` and pass that mutex guard
    /// here while sleeping.
    ///
    /// Notifications wake the worker when a new job, pause transition, or shutdown
    /// changes its run condition, and wake pause callers when the active worker finishes.
    ///
    /// Timed retry waits also use this condition variable so state changes preempt the polling
    /// delay.
    changed: Condvar,
    /// This is used to interrupt the ongoing worker activity and make it check the changes to
    /// the state. For example, when there is a new job to be done, or when the worker needs to
    /// be paused or shut down.
    ///
    /// This flag could be set by any thread holding the state lock, but is typically set by
    /// the thread holding the handle.
    activity_interrupted: AtomicBool,
}

impl<J> Debug for Coordinator<J> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let state = self.state.lock();
        f.debug_struct("Coordinator")
            .field("shutdown", &state.shutdown)
            .field("pause_count", &state.pause_count)
            .field("active", &state.active)
            .field("published", &state.snapshot.as_ref().map(|snapshot| snapshot.parent_hash()))
            .finish_non_exhaustive()
    }
}

impl<J> Coordinator<J> {
    pub(super) const fn new() -> Self {
        Self {
            state: Mutex::new(State {
                latest_job: None,
                shutdown: false,
                pause_count: 0,
                active: false,
                snapshot: None,
            }),
            changed: Condvar::new(),
            activity_interrupted: AtomicBool::new(false),
        }
    }

    pub(super) fn set_job(&self, job: J) {
        let replaced = {
            let mut state = self.state.lock();
            if state.shutdown {
                return
            }
            self.activity_interrupted.store(true, Ordering::Release);
            state.latest_job.replace(job)
        };
        drop(replaced);
        self.changed.notify_all();
    }

    /// Waits until a job may run and marks the worker active before releasing the state lock.
    ///
    /// has_current_job indicates whether there is already a current job running. If there is none
    /// the method will wait for a job to become available.
    pub(super) fn begin_activity(
        self: &Arc<Self>,
        has_current_job: bool,
    ) -> Result<(Option<J>, ActivityGuard<J>), Shutdown> {
        let mut state = self.state.lock();
        loop {
            if state.shutdown {
                return Err(Shutdown)
            }
            let has_runnable_job = has_current_job || state.latest_job.is_some();
            if state.active || state.pause_count != 0 || !has_runnable_job {
                self.changed.wait(&mut state);
                continue
            }

            let replacement = state.latest_job.take();
            // Every interruption writer holds this same lock, so resetting here cannot lose an
            // interruption request for the activity being started.
            self.activity_interrupted.store(false, Ordering::Release);
            state.active = true;
            return Ok((
                replacement,
                ActivityGuard { coordinator: Arc::clone(self), disarmed: false },
            ))
        }
    }

    /// Waits for a control-plane change or until polling should be retried.
    pub(super) fn wait_for_change(&self, timeout: Duration) {
        let mut state = self.state.lock();
        if !state.has_pending_interruption() {
            self.changed.wait_for(&mut state, timeout);
        }
    }

    pub(super) fn pause(self: &Arc<Self>) -> PauseGuard<J> {
        let mut state = self.state.lock();
        state.pause_count =
            state.pause_count.checked_add(1).expect("txpool prewarm pause count overflow");
        self.activity_interrupted.store(true, Ordering::Release);
        self.changed.notify_all();
        while state.active {
            self.changed.wait(&mut state);
        }
        PauseGuard { coordinator: Arc::downgrade(self) }
    }

    pub(super) fn snapshot(&self, parent_hash: B256) -> Option<Snapshot> {
        self.state
            .lock()
            .snapshot
            .as_ref()
            .filter(|snapshot| snapshot.parent_hash() == parent_hash)
            .cloned()
    }

    pub(super) fn shutdown(&self) {
        let latest = {
            let mut state = self.state.lock();
            state.shutdown = true;
            self.activity_interrupted.store(true, Ordering::Release);
            state.latest_job.take()
        };
        drop(latest);
        self.changed.notify_all();
    }

    fn resume(&self) {
        let mut state = self.state.lock();
        state.pause_count = state
            .pause_count
            .checked_sub(1)
            .expect("txpool prewarm resumed without a matching pause");
        let resumed = state.pause_count == 0;
        drop(state);
        if resumed {
            self.changed.notify_all();
        }
    }

    fn finish_activity(&self, snapshot: Option<Snapshot>) -> bool {
        let mut state = self.state.lock();
        debug_assert!(state.active);
        let published = if !state.has_pending_interruption() &&
            let Some(snapshot) = snapshot
        {
            state.snapshot = Some(snapshot);
            true
        } else {
            false
        };
        state.active = false;
        self.changed.notify_all();
        published
    }
}

struct State<J> {
    latest_job: Option<J>,
    /// Whether the prewarming worker should shut down.
    shutdown: bool,
    /// How many pause guards are currently active.
    ///
    /// When it is non-zero, the worker will pause. When it reaches zero, the worker will resume.
    pause_count: u64,
    /// Whether the worker is in the active section right now.
    active: bool,
    /// The current snapshot of the prewarm cache based at some parent state.
    snapshot: Option<Snapshot>,
}

impl<J> State<J> {
    /// If true the worker should consult with the control-plane to determine if it should change
    /// its course.
    const fn has_pending_interruption(&self) -> bool {
        self.shutdown || self.pause_count != 0 || self.latest_job.is_some()
    }
}

#[derive(Debug)]
pub(super) struct Shutdown;

/// Keeps txpool prewarming paused until the cache-sensitive work is complete.
pub(super) struct PauseGuard<J> {
    coordinator: Weak<Coordinator<J>>,
}

impl<J> Drop for PauseGuard<J> {
    fn drop(&mut self) {
        if let Some(coordinator) = self.coordinator.upgrade() {
            coordinator.resume();
        }
    }
}

pub(super) struct ActivityGuard<J> {
    coordinator: Arc<Coordinator<J>>,
    disarmed: bool,
}

impl<J> ActivityGuard<J> {
    pub(super) fn is_interrupted(&self) -> bool {
        self.coordinator.activity_interrupted.load(Ordering::Acquire)
    }

    pub(super) fn publish(mut self, snapshot: Snapshot) -> bool {
        let published = self.coordinator.finish_activity(Some(snapshot));
        self.disarmed = true;
        published
    }
}

impl<J> Drop for ActivityGuard<J> {
    fn drop(&mut self) {
        if !self.disarmed {
            self.coordinator.finish_activity(None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{sync::mpsc, thread, time::Instant};

    type TestCoordinator = Coordinator<u64>;

    fn coordinator() -> Arc<TestCoordinator> {
        Arc::new(TestCoordinator::new())
    }

    fn snapshot(parent_hash: B256) -> Snapshot {
        Snapshot::from_parts(
            parent_hash,
            Default::default(),
            Default::default(),
            Default::default(),
        )
    }

    #[test]
    fn snapshot_is_published_for_matching_parent() {
        let coordinator = coordinator();
        let (_, activity_guard) =
            coordinator.begin_activity(true).expect("prewarming should become active");
        let parent_hash = B256::repeat_byte(0x01);
        assert!(activity_guard.publish(snapshot(parent_hash)));

        assert_eq!(
            coordinator.snapshot(parent_hash).map(|snapshot| snapshot.parent_hash()),
            Some(parent_hash)
        );
        assert!(coordinator.snapshot(B256::ZERO).is_none());
    }

    #[test]
    fn newest_job_interrupts_activity() {
        let coordinator = coordinator();
        let (_, activity_guard) =
            coordinator.begin_activity(true).expect("prewarming should become active");

        coordinator.set_job(1);
        coordinator.set_job(2);
        assert!(activity_guard.is_interrupted());
        let parent_hash = B256::repeat_byte(0x01);
        assert!(!activity_guard.publish(snapshot(parent_hash)));
        assert!(coordinator.snapshot(parent_hash).is_none());

        let (replacement, next_activity_guard) =
            coordinator.begin_activity(false).expect("latest job should become active");
        assert_eq!(replacement, Some(2));
        assert!(!next_activity_guard.is_interrupted());
    }

    #[test]
    fn pause_waits_for_activity_and_rejects_publication() {
        let coordinator = coordinator();
        let (_, activity_guard) =
            coordinator.begin_activity(true).expect("prewarming should become active");
        let waiter_coordinator = Arc::clone(&coordinator);
        let (guard_tx, guard_rx) = mpsc::channel();
        let waiter = thread::spawn(move || {
            let guard = waiter_coordinator.pause();
            guard_tx.send(guard).expect("pause guard receiver dropped");
        });

        let deadline = Instant::now() + Duration::from_secs(1);
        while coordinator.state.lock().pause_count == 0 {
            assert!(Instant::now() < deadline, "pause request was not observed");
            thread::yield_now();
        }
        assert!(guard_rx.try_recv().is_err());
        assert!(activity_guard.is_interrupted());

        let parent_hash = B256::repeat_byte(0x01);
        assert!(!activity_guard.publish(snapshot(parent_hash)));
        let guard = guard_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("pause did not finish after activity stopped");
        waiter.join().expect("pause waiter panicked");
        assert!(coordinator.snapshot(parent_hash).is_none());

        drop(guard);
        assert_eq!(coordinator.state.lock().pause_count, 0);
    }

    #[test]
    fn pause_guard_does_not_retain_coordinator() {
        let coordinator = coordinator();
        let weak_coordinator = Arc::downgrade(&coordinator);
        let guard = coordinator.pause();

        drop(coordinator);
        assert!(weak_coordinator.upgrade().is_none());
        drop(guard);
    }

    #[test]
    fn overlapping_pause_guards_resume_after_last_drop() {
        let coordinator = coordinator();

        let first = coordinator.pause();
        let second = coordinator.pause();
        drop(first);
        assert_eq!(coordinator.state.lock().pause_count, 1);

        drop(second);
        assert_eq!(coordinator.state.lock().pause_count, 0);
        assert!(coordinator.begin_activity(true).is_ok());
    }

    #[test]
    fn shutdown_wakes_waiting_worker() {
        let coordinator = coordinator();
        let waiter_coordinator = Arc::clone(&coordinator);
        let waiter = thread::spawn(move || waiter_coordinator.begin_activity(false).is_err());

        coordinator.shutdown();
        assert!(waiter.join().expect("worker waiter panicked"));
    }
}
