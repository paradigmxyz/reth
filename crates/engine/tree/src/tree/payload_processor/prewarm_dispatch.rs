//! Optional admission for the external transaction-prewarm distributor.
//!
//! Waiting happens before a Rayon job is spawned, outside worker-local state. The existing
//! committed cursor and stop flag are polled; the canonical loop never waits or takes a new lock.

use std::{
    num::NonZeroUsize,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError, SyncSender},
        Arc,
    },
    time::Duration,
};

/// Opt-in bounds for transaction prewarming, independent of completed-result retention.
///
/// This gates all transaction prewarming, including cache/proof hints. A smaller window can
/// reduce useful prefetch lead. It does not bound the already converted input or provider memory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TransactionPrewarmPolicy {
    lookahead: NonZeroUsize,
    max_in_flight: NonZeroUsize,
    poll_interval: Duration,
}

impl TransactionPrewarmPolicy {
    /// Creates a policy. Both counts must be nonzero, and polling must be in `(0, 10ms]`.
    ///
    /// `max_in_flight` counts queued and running jobs, including jobs overtaken by the canonical
    /// cursor. Choose it independently of the number of canonical execution threads.
    pub const fn new(
        lookahead: usize,
        max_in_flight: usize,
        poll_interval: Duration,
    ) -> Option<Self> {
        if let (Some(lookahead), Some(max_in_flight)) =
            (NonZeroUsize::new(lookahead), NonZeroUsize::new(max_in_flight)) &&
            !poll_interval.is_zero() &&
            poll_interval.as_nanos() <= 10_000_000
        {
            Some(Self { lookahead, max_in_flight, poll_interval })
        } else {
            None
        }
    }

    /// Maximum forward distance from the successfully committed transaction cursor.
    pub const fn lookahead(self) -> usize {
        self.lookahead.get()
    }

    /// Maximum total queued and running transaction jobs.
    pub const fn max_in_flight(self) -> usize {
        self.max_in_flight.get()
    }

    /// Maximum idle wait before rechecking canonical progress or cancellation.
    pub const fn poll_interval(self) -> Duration {
        self.poll_interval
    }
}

#[derive(Debug)]
struct InFlight {
    count: AtomicUsize,
    completed: SyncSender<()>,
}

/// A queued/running job owns this until its whole worker call has returned, including unwinding.
#[derive(Debug)]
pub(super) struct PrewarmPermit(Arc<InFlight>);

impl Drop for PrewarmPermit {
    fn drop(&mut self) {
        let previous = self.0.count.fetch_sub(1, Ordering::Relaxed);
        debug_assert!(previous > 0, "a prewarm permit was released more than once");
        // A coalesced wake is only an optimization. Timed polling observes progress/stop even
        // without a wake, and permit destruction must never block a pool worker.
        let _ = self.0.completed.try_send(());
    }
}

#[derive(Debug)]
pub(super) struct BoundedPrewarmReceiver<T> {
    pending: Receiver<(usize, T)>,
    policy: TransactionPrewarmPolicy,
    committed: Arc<AtomicUsize>,
    stopped: Arc<AtomicBool>,
    in_flight: Arc<InFlight>,
    completed: Receiver<()>,
    #[cfg(test)]
    waiting: Option<SyncSender<WaitReason>>,
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WaitReason {
    Input,
    Future,
    Capacity,
}

impl<T> BoundedPrewarmReceiver<T> {
    pub(super) fn new(
        pending: Receiver<(usize, T)>,
        policy: TransactionPrewarmPolicy,
        committed: Arc<AtomicUsize>,
        stopped: Arc<AtomicBool>,
    ) -> Self {
        let (completion_tx, completed) = mpsc::sync_channel(1);
        Self {
            pending,
            policy,
            committed,
            stopped,
            in_flight: Arc::new(InFlight { count: AtomicUsize::new(0), completed: completion_tx }),
            completed,
            #[cfg(test)]
            waiting: None,
        }
    }

    /// Returns the next admitted transaction and its already acquired job permit.
    ///
    /// Call only from the external coordinator. Source ordering is unchanged. The caller moves
    /// the permit into the spawned closure; queued stale work keeps its charge until it returns.
    pub(super) fn next(&self) -> Option<(usize, T, PrewarmPermit)> {
        'input: loop {
            if self.stopped.load(Ordering::Relaxed) {
                return None;
            }
            #[cfg(test)]
            self.observe_wait(WaitReason::Input);
            let (index, tx) = match self.pending.recv_timeout(self.policy.poll_interval) {
                Ok(item) => item,
                Err(RecvTimeoutError::Timeout) => continue,
                Err(RecvTimeoutError::Disconnected) => return None,
            };
            loop {
                if self.stopped.load(Ordering::Relaxed) {
                    return None;
                }
                let committed = self.committed.load(Ordering::Relaxed);
                if index < committed {
                    continue 'input;
                }
                // Subtract after the stale check to avoid overflow near usize::MAX.
                let future = index - committed >= self.policy.lookahead.get();
                if !future &&
                    self.in_flight.count.load(Ordering::Relaxed) <
                        self.policy.max_in_flight.get()
                {
                    // Only this coordinator increments; workers only decrement. No competing
                    // producer can invalidate the capacity check before this reservation.
                    self.in_flight.count.fetch_add(1, Ordering::Relaxed);
                    return Some((index, tx, PrewarmPermit(Arc::clone(&self.in_flight))));
                }
                #[cfg(test)]
                self.observe_wait(if future { WaitReason::Future } else { WaitReason::Capacity });
                let _ = self.completed.recv_timeout(self.policy.poll_interval);
            }
        }
    }

    #[cfg(test)]
    fn observe_wait(&self, reason: WaitReason) {
        if let Some(waiting) = &self.waiting {
            let _ = waiting.try_send(reason);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{panic::AssertUnwindSafe, thread};

    const DEADLINE: Duration = Duration::from_secs(5);

    fn receiver<T>(
        capacity: usize,
        lookahead: usize,
        max_in_flight: usize,
    ) -> (SyncSender<(usize, T)>, BoundedPrewarmReceiver<T>, Receiver<WaitReason>) {
        let (input, pending) = mpsc::sync_channel(capacity);
        let (waiting, waits) = mpsc::sync_channel(1);
        let mut receiver = BoundedPrewarmReceiver::new(
            pending,
            TransactionPrewarmPolicy::new(lookahead, max_in_flight, Duration::from_millis(1))
                .unwrap(),
            Arc::new(AtomicUsize::new(0)),
            Arc::new(AtomicBool::new(false)),
        );
        receiver.waiting = Some(waiting);
        (input, receiver, waits)
    }

    fn wait_for(waits: &Receiver<WaitReason>, expected: WaitReason) {
        let deadline = std::time::Instant::now() + DEADLINE;
        loop {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            assert!(!remaining.is_zero(), "coordinator never reached {expected:?}");
            if waits.recv_timeout(remaining).unwrap() == expected {
                break;
            }
        }
    }

    #[test]
    fn policy_rejects_zero_counts_and_unbounded_poll_intervals() {
        let interval = Duration::from_millis(1);
        assert!(TransactionPrewarmPolicy::new(0, 1, interval).is_none());
        assert!(TransactionPrewarmPolicy::new(1, 0, interval).is_none());
        assert!(TransactionPrewarmPolicy::new(1, 1, Duration::ZERO).is_none());
        assert!(TransactionPrewarmPolicy::new(1, 1, Duration::from_millis(11)).is_none());
        assert!(TransactionPrewarmPolicy::new(1, 1, Duration::from_millis(10)).is_some());
    }

    #[test]
    fn stop_while_source_is_idle_does_not_require_sender_drop() {
        let (_input, receiver, waits) = receiver::<()>(1, 1, 1);
        let stopped = receiver.stopped.clone();
        let (done, completion) = mpsc::channel();
        let worker = thread::spawn(move || done.send(receiver.next().is_none()).unwrap());
        wait_for(&waits, WaitReason::Input);
        stopped.store(true, Ordering::Relaxed);
        assert!(completion.recv_timeout(DEADLINE).unwrap());
        worker.join().unwrap();
    }

    #[test]
    fn early_error_stop_releases_future_input_with_stalled_cursor() {
        let (input, receiver, waits) = receiver(2, 1, 1);
        input.send((1, ())).unwrap();
        let stopped = receiver.stopped.clone();
        let (done, completion) = mpsc::channel();
        let worker = thread::spawn(move || done.send(receiver.next().is_none()).unwrap());
        wait_for(&waits, WaitReason::Future);
        stopped.store(true, Ordering::Relaxed);
        assert!(completion.recv_timeout(DEADLINE).unwrap());
        worker.join().unwrap();
        drop(input);
    }

    #[test]
    fn raw_cursor_advance_admits_future_work_without_a_notification() {
        let (input, receiver, waits) = receiver(2, 2, 1);
        input.send((2, 17)).unwrap();
        let committed = receiver.committed.clone();
        let (done, completion) = mpsc::channel();
        let worker = thread::spawn(move || {
            let (index, value, _permit) = receiver.next().unwrap();
            done.send((index, value)).unwrap();
        });
        wait_for(&waits, WaitReason::Future);
        committed.store(1, Ordering::Relaxed);
        assert_eq!(completion.recv_timeout(DEADLINE).unwrap(), (2, 17));
        worker.join().unwrap();
    }

    #[test]
    fn in_flight_cap_survives_cursor_advance_and_wakes_on_completion() {
        let (input, receiver, waits) = receiver(2, 2, 1);
        input.send((0, ())).unwrap();
        input.send((1, ())).unwrap();
        let (_, (), permit) = receiver.next().unwrap();
        receiver.committed.store(1, Ordering::Relaxed);
        let count = receiver.in_flight.clone();
        let (done, completion) = mpsc::channel();
        let worker = thread::spawn(move || {
            let (index, (), _permit) = receiver.next().unwrap();
            done.send(index).unwrap();
        });
        wait_for(&waits, WaitReason::Capacity);
        assert_eq!(count.count.load(Ordering::Relaxed), 1);
        assert!(matches!(completion.try_recv(), Err(mpsc::TryRecvError::Empty)));
        drop(permit);
        assert_eq!(completion.recv_timeout(DEADLINE).unwrap(), 1);
        worker.join().unwrap();
        assert_eq!(count.count.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn overtaken_inputs_are_skipped_without_releasing_running_jobs() {
        let (input, receiver, _) = receiver(3, 2, 1);
        input.send((0, ())).unwrap();
        input.send((1, ())).unwrap();
        input.send((2, ())).unwrap();
        drop(input);
        let (_, (), permit) = receiver.next().unwrap();
        receiver.committed.store(3, Ordering::Relaxed);
        assert!(receiver.next().is_none());
        assert_eq!(receiver.in_flight.count.load(Ordering::Relaxed), 1);
        drop(permit);
        assert_eq!(receiver.in_flight.count.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn permits_release_after_worker_error_or_unwind() {
        let (input, receiver, _) = receiver(3, 3, 1);
        for index in 0..3 {
            input.send((index, ())).unwrap();
        }
        fn fails(_permit: PrewarmPermit) -> Result<(), ()> {
            Err(())?;
            Ok(())
        }
        let (_, (), permit) = receiver.next().unwrap();
        assert!(fails(permit).is_err());
        assert_eq!(receiver.in_flight.count.load(Ordering::Relaxed), 0);
        let (_, (), permit) = receiver.next().unwrap();
        assert!(std::panic::catch_unwind(AssertUnwindSafe(move || {
            let _permit = permit;
            panic!("worker unwound");
        }))
        .is_err());
        assert_eq!(receiver.in_flight.count.load(Ordering::Relaxed), 0);
        let (index, (), permit) = receiver.next().unwrap();
        assert_eq!(index, 2);
        drop(permit);
    }

    #[test]
    fn full_block_input_does_not_need_dispatch_or_canonical_progress() {
        let (input, receiver, _) = receiver(128, 4, 2);
        for index in 0..128 {
            input.try_send((index, index)).unwrap();
        }
        drop(input);
        for index in 0..128 {
            let (actual, value, permit) = receiver.next().unwrap();
            assert_eq!((actual, value), (index, index));
            assert!(receiver.in_flight.count.load(Ordering::Relaxed) <= 2);
            receiver.committed.store(index + 1, Ordering::Relaxed);
            drop(permit);
        }
        assert!(receiver.next().is_none());
    }

    #[test]
    fn near_maximum_cursor_does_not_overflow_lookahead() {
        let (input, receiver, _) = receiver(1, usize::MAX, 1);
        receiver.committed.store(usize::MAX - 1, Ordering::Relaxed);
        input.send((usize::MAX, ())).unwrap();
        let (index, (), _permit) = receiver.next().unwrap();
        assert_eq!(index, usize::MAX);
    }
}
