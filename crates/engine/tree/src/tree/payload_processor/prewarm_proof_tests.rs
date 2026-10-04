//! Deterministic coordinator tests; hint callbacks never execute an EVM job.

use super::*;

type ProofReceiverFixture<T, U> =
    (SyncSender<(usize, T)>, ProofKeyPrewarmReceiver<T, U>, Receiver<WaitReason>);
use std::{panic::AssertUnwindSafe, thread};

const DEADLINE: Duration = Duration::from_secs(5);

fn receiver<T, U>(
    capacity: usize,
    near: usize,
    max_in_flight: usize,
    far: usize,
) -> ProofReceiverFixture<T, U> {
    let (input, pending) = mpsc::sync_channel(capacity);
    let (waiting, waits) = mpsc::sync_channel(1);
    let near =
        TransactionPrewarmPolicy::new(near, max_in_flight, Duration::from_millis(1)).unwrap();
    let mut receiver = ProofKeyPrewarmReceiver::new(
        pending,
        near,
        ProofKeyPrewarmPolicy::new(near, far).unwrap(),
        Arc::new(AtomicUsize::new(0)),
        Arc::new(AtomicBool::new(false)),
    );
    receiver.near.waiting = Some(waiting);
    (input, receiver, waits)
}

fn wait_for(waits: &Receiver<WaitReason>, expected: WaitReason) {
    let deadline = std::time::Instant::now() + DEADLINE;
    loop {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        assert!(!remaining.is_zero(), "coordinator never reached {expected:?}");
        if waits.recv_timeout(remaining).unwrap() == expected {
            return;
        }
    }
}

#[derive(Debug)]
struct Tracked {
    index: usize,
    drops: Arc<AtomicUsize>,
}

impl Drop for Tracked {
    fn drop(&mut self) {
        self.drops.fetch_add(1, Ordering::Relaxed);
    }
}

#[test]
fn proof_policy_requires_a_bounded_extension_of_the_same_near_window() {
    let near = TransactionPrewarmPolicy::new(4, 2, Duration::from_millis(1)).unwrap();
    for far in [0, 1, 4, 9, usize::MAX] {
        assert!(ProofKeyPrewarmPolicy::new(near, far).is_none());
    }
    for far in [5, 8] {
        let policy = ProofKeyPrewarmPolicy::new(near, far).unwrap();
        assert_eq!(policy.lookahead(), far);
        assert!(policy.matches(near));
        let other = TransactionPrewarmPolicy::new(3, 2, Duration::from_millis(1)).unwrap();
        assert!(!policy.matches(other));
    }
    let near = TransactionPrewarmPolicy::new(512, 2, Duration::from_millis(1)).unwrap();
    assert!(ProofKeyPrewarmPolicy::new(near, 1024).is_some());
    assert!(ProofKeyPrewarmPolicy::new(near, 1025).is_none());
    let near = TransactionPrewarmPolicy::new(usize::MAX, 1, Duration::from_millis(1)).unwrap();
    assert!(ProofKeyPrewarmPolicy::new(near, usize::MAX).is_none());
}

#[test]
fn oversized_inline_handles_decline_opt_in_before_allocating_the_queue() {
    let near = TransactionPrewarmPolicy::new(1, 1, Duration::from_millis(1)).unwrap();
    let policy = ProofKeyPrewarmPolicy::new(near, 2).unwrap();
    assert!(ProofKeyPrewarmReceiver::<(), usize>::fits(policy));
    assert!(!ProofKeyPrewarmReceiver::<(), [u8; MAX_PROOF_QUEUE_INLINE_BYTES]>::fits(policy));
}

#[test]
fn ready_near_work_is_returned_before_preparing_or_hinting_far_work() {
    let (input, mut receiver, _) = receiver::<usize, usize>(8, 4, 2, 8);
    for index in 0..8 {
        input.send((index, index)).unwrap();
    }
    let mut prepared = Vec::new();
    let (index, value, permit) = receiver
        .next(
            |value| {
                prepared.push(value);
                value
            },
            |_| panic!("ready near work was delayed by proof hints"),
        )
        .unwrap();
    assert_eq!((index, value), (0, 0));
    assert_eq!(prepared, [0]);
    assert!(receiver.buffered.is_empty());
    assert_eq!(receiver.stats.near_jobs, 1);
    assert_eq!(receiver.stats.far_transactions, 0);
    assert!(receiver.stats.queue_inline_bytes <= MAX_PROOF_QUEUE_INLINE_BYTES);
    drop(permit);
}

#[test]
fn hinted_handles_are_prepared_once_and_keep_their_ordered_near_opportunity() {
    let (input, mut receiver, _) = receiver::<usize, usize>(4, 2, 1, 4);
    for index in 0..4 {
        input.send((index, index)).unwrap();
    }
    drop(input);
    let mut prepared = Vec::new();
    let mut hints = Vec::new();
    let (index, value, permit) = receiver
        .next(
            |value| {
                prepared.push(value);
                value
            },
            |_| panic!("first near job must be immediate"),
        )
        .unwrap();
    assert_eq!((index, value), (0, 0));
    let mut held = Some(permit);
    let (index, value, permit) = receiver
        .next(
            |value| {
                prepared.push(value);
                value
            },
            |batch| {
                hints.extend(batch.iter().map(|value| **value));
                drop(held.take());
            },
        )
        .unwrap();
    assert_eq!((index, value), (1, 1));
    drop(permit);
    for expected in 2..4 {
        receiver.near.committed.store(expected, Ordering::Relaxed);
        let (index, value, permit) = receiver
            .next(
                |_| panic!("buffered handle was prepared twice"),
                |_| panic!("buffered handle was hinted twice"),
            )
            .unwrap();
        assert_eq!((index, value), (expected, expected));
        drop(permit);
    }
    assert!(receiver.next(|value| value, |_| panic!("unexpected hint")).is_none());
    assert_eq!(prepared, [0, 1, 2, 3]);
    assert_eq!(hints, [2, 3]);
    assert_eq!(receiver.stats.near_jobs, 4);
    assert_eq!(receiver.stats.far_transactions, 2);
    assert!(receiver.stats.input_disconnected);
}

#[test]
fn near_progress_is_rechecked_after_one_sixteen_transaction_hint_batch() {
    let (input, mut receiver, _) = receiver::<usize, usize>(64, 32, 1, 64);
    for index in 0..64 {
        input.send((index, index)).unwrap();
    }
    drop(input);
    let (_, _, permit) = receiver.next(|value| value, |_| panic!("unexpected hint")).unwrap();
    let mut held = Some(permit);
    let committed = receiver.near.committed.clone();
    let mut batches = Vec::new();
    let (index, value, permit) = receiver
        .next(
            |value| value,
            |batch| {
                batches.push(batch.iter().map(|value| **value).collect::<Vec<_>>());
                committed.store(32, Ordering::Relaxed);
                drop(held.take());
            },
        )
        .unwrap();
    assert_eq!((index, value), (32, 32));
    assert_eq!(batches, vec![(32..48).collect::<Vec<_>>()]);
    assert_eq!(receiver.stats.far_batches, 1);
    assert_eq!(receiver.stats.far_transactions, PROOF_HINT_BATCH_TRANSACTIONS);
    assert_eq!(receiver.stats.stale_handles, 31);
    assert!(receiver.stats.peak_buffered_handles <= 64);
    drop(permit);
}

#[test]
fn stopping_after_one_hint_batch_drops_buffered_owners_without_extra_jobs() {
    let (input, mut receiver, _) = receiver::<Tracked, Tracked>(64, 32, 1, 64);
    let drops = Arc::new(AtomicUsize::new(0));
    for index in 0..64 {
        input.send((index, Tracked { index, drops: drops.clone() })).unwrap();
    }
    drop(input);
    let (_, first, permit) = receiver.next(|value| value, |_| panic!("unexpected hint")).unwrap();
    let stopped = receiver.near.stopped.clone();
    let mut batches = 0;
    assert!(receiver
        .next(
            |value| value,
            |batch| {
                batches += 1;
                assert_eq!(batch.len(), PROOF_HINT_BATCH_TRANSACTIONS);
                assert_eq!(batch[0].index, 32);
                stopped.store(true, Ordering::Relaxed);
            }
        )
        .is_none());
    assert_eq!(batches, 1);
    assert_eq!(receiver.stats.near_jobs, 1);
    assert_eq!(receiver.stats.stopped_handles, 63);
    assert!(receiver.stats.stopped);
    assert!(receiver.buffered.is_empty());
    assert_eq!(drops.load(Ordering::Relaxed), 63);
    assert_eq!(receiver.near.in_flight.count.load(Ordering::Relaxed), 1);
    drop(first);
    drop(permit);
    assert_eq!(drops.load(Ordering::Relaxed), 64);
    assert_eq!(receiver.near.in_flight.count.load(Ordering::Relaxed), 0);
}

#[test]
fn an_idle_source_stops_without_requiring_the_sender_to_disconnect() {
    let (_input, mut receiver, waits) = receiver::<(), ()>(1, 1, 1, 2);
    let stopped = receiver.near.stopped.clone();
    let (done, completion) = mpsc::sync_channel(1);
    let worker = thread::spawn(move || {
        let result = receiver.next(|value| value, |_| panic!("unexpected hint"));
        done.send((result.is_none(), receiver.stats.stopped)).unwrap();
    });
    wait_for(&waits, WaitReason::Input);
    stopped.store(true, Ordering::Relaxed);
    assert_eq!(completion.recv_timeout(DEADLINE).unwrap(), (true, true));
    worker.join().unwrap();
}

#[test]
fn disconnected_input_preserves_a_buffered_future_near_opportunity() {
    let (input, mut receiver, waits) = receiver::<usize, usize>(1, 2, 1, 4);
    input.send((2, 22)).unwrap();
    drop(input);
    let committed = receiver.near.committed.clone();
    let (done, completion) = mpsc::sync_channel(1);
    let worker = thread::spawn(move || {
        let mut hints = 0;
        let (index, value, permit) = receiver
            .next(
                |value| value,
                |batch| {
                    assert_eq!(batch, &[&22]);
                    hints += 1;
                },
            )
            .unwrap();
        drop(permit);
        done.send((index, value, hints, receiver.stats.input_disconnected)).unwrap();
    });
    wait_for(&waits, WaitReason::Future);
    assert!(matches!(completion.try_recv(), Err(mpsc::TryRecvError::Empty)));
    committed.store(1, Ordering::Relaxed);
    assert_eq!(completion.recv_timeout(DEADLINE).unwrap(), (2, 22, 1, true));
    worker.join().unwrap();
}

#[test]
fn the_first_out_of_range_handle_blocks_further_reads_and_receives_no_hint() {
    let (input, mut receiver, waits) = receiver::<usize, usize>(2, 2, 1, 4);
    input.send((4, 44)).unwrap();
    input.send((5, 55)).unwrap();
    drop(input);
    let committed = receiver.near.committed.clone();
    let prepared = Arc::new(AtomicUsize::new(0));
    let observed = prepared.clone();
    let (done, completion) = mpsc::sync_channel(1);
    let worker = thread::spawn(move || {
        let (index, value, permit) = receiver
            .next(
                |value| {
                    prepared.fetch_add(1, Ordering::Relaxed);
                    value
                },
                |_| panic!("out-of-range item was hinted"),
            )
            .unwrap();
        drop(permit);
        done.send((index, value, receiver.stats.peak_buffered_handles)).unwrap();
    });
    wait_for(&waits, WaitReason::Future);
    assert_eq!(observed.load(Ordering::Relaxed), 1);
    committed.store(3, Ordering::Relaxed);
    assert_eq!(completion.recv_timeout(DEADLINE).unwrap(), (4, 44, 1));
    assert_eq!(observed.load(Ordering::Relaxed), 1);
    worker.join().unwrap();
}

#[test]
fn stale_buffered_owners_are_dropped_when_canonical_progress_overtakes_them() {
    let (input, mut receiver, _) = receiver::<Tracked, Tracked>(6, 2, 1, 4);
    let drops = Arc::new(AtomicUsize::new(0));
    for index in 0..6 {
        input.send((index, Tracked { index, drops: drops.clone() })).unwrap();
    }
    drop(input);
    let (_, first, permit) = receiver.next(|value| value, |_| panic!("unexpected hint")).unwrap();
    let mut held = Some(permit);
    let committed = receiver.near.committed.clone();
    let (index, last, permit) = receiver
        .next(
            |value| value,
            |batch| {
                assert_eq!(batch.iter().map(|value| value.index).collect::<Vec<_>>(), [2, 3]);
                committed.store(5, Ordering::Relaxed);
                drop(held.take());
            },
        )
        .unwrap();
    assert_eq!((index, last.index), (5, 5));
    assert_eq!(receiver.stats.stale_handles, 4);
    assert_eq!(receiver.stats.peak_buffered_handles, 4);
    assert_eq!(drops.load(Ordering::Relaxed), 4);
    drop((first, last, permit));
    assert_eq!(drops.load(Ordering::Relaxed), 6);
}

#[test]
fn indices_near_usize_max_can_be_hinted_then_admitted_without_overflow() {
    let (input, mut receiver, _) = receiver::<usize, usize>(1, 2, 1, 4);
    receiver.near.committed.store(usize::MAX - 3, Ordering::Relaxed);
    input.send((usize::MAX, usize::MAX)).unwrap();
    drop(input);
    let committed = receiver.near.committed.clone();
    let (index, value, permit) = receiver
        .next(
            |value| value,
            |batch| {
                assert_eq!(batch, &[&usize::MAX]);
                committed.store(usize::MAX - 1, Ordering::Relaxed);
            },
        )
        .unwrap();
    assert_eq!((index, value), (usize::MAX, usize::MAX));
    assert_eq!(receiver.stats.far_transactions, 1);
    assert_eq!(receiver.stats.near_jobs, 1);
    drop(permit);
}

#[test]
fn a_proof_coordinator_permit_is_released_during_worker_unwind() {
    let (input, mut receiver, _) = receiver::<(), ()>(2, 2, 1, 4);
    input.send((0, ())).unwrap();
    input.send((1, ())).unwrap();
    drop(input);
    let (_, (), permit) = receiver.next(|value| value, |_| panic!("unexpected hint")).unwrap();
    assert_eq!(receiver.near.in_flight.count.load(Ordering::Relaxed), 1);
    assert!(std::panic::catch_unwind(AssertUnwindSafe(move || {
        let _permit = permit;
        panic!("worker unwound");
    }))
    .is_err());
    assert_eq!(receiver.near.in_flight.count.load(Ordering::Relaxed), 0);
    let (index, (), permit) = receiver.next(|value| value, |_| panic!("unexpected hint")).unwrap();
    assert_eq!(index, 1);
    drop(permit);
}
