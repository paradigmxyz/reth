//! Run with `cargo run -p reth-tasks --example executor
//! --features rayon`. Add `fuzz` to run the same blocking workload with deterministic scheduling
//! and virtual time.
#![forbid(unsafe_code)]

use reth_tasks::{
    executor::{
        channel, pool, thread,
        time::{self, Duration, Instant},
        Runtime,
    },
    runtime::{DEFAULT_MAX_BLOCKING_TASKS, DEFAULT_RESERVED_CPU_CORES},
    RayonConfig, RuntimeBuilder, RuntimeConfig, TaskExecutor, TokioConfig, WorkerStatefulPool,
};
use tokio::{runtime::Builder, task};

use std::{
    cell::Cell,
    rc::Rc,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    thread::{current, ThreadId},
};

const WAIT: Duration = Duration::from_secs(1);

struct WorkerState {
    value: Rc<Cell<usize>>,
    creator: ThreadId,
    drops: Arc<AtomicUsize>,
    wrong_thread_drops: Arc<AtomicUsize>,
}

impl Drop for WorkerState {
    fn drop(&mut self) {
        if self.creator != current().id() {
            self.wrong_thread_drops.fetch_add(1, Ordering::SeqCst);
        }
        self.drops.fetch_add(1, Ordering::SeqCst);
    }
}

fn main() {
    // Tokio remains native: only blocking work belongs to the selected executor runtime.
    let tokio = Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .expect("native Tokio runtime builds");
    let handle = tokio.handle().clone();
    let asynchronous = Runtime::new().run(move || {
        let tasks: TaskExecutor = RuntimeBuilder::new(
            RuntimeConfig::default().with_tokio(TokioConfig::existing_handle(handle)).with_rayon(
                RayonConfig {
                    cpu_threads: Some(2),
                    rpc_threads: Some(2),
                    storage_threads: Some(2),
                    proof_storage_worker_threads: Some(2),
                    proof_account_worker_threads: Some(2),
                    prewarming_threads: Some(2),
                    bal_streaming_threads: Some(2),
                    state_trie_overlay_worker_threads: Some(2),
                    reserved_cpu_cores: DEFAULT_RESERVED_CPU_CORES,
                    max_blocking_tasks: DEFAULT_MAX_BLOCKING_TASKS,
                },
            ),
        )
        .build()
        .expect("Reth runtime attaches to native Tokio");
        let asynchronous = tasks.handle().spawn(async {
            task::yield_now().await;
            42
        });
        println!("runtime: native Tokio attached; blocking executor entered");

        let (sender, receiver) = channel::bounded(1);
        let (filled, ready) = channel::bounded(1);
        let second_sender = sender.clone();
        let producer = tasks.spawn_blocking_named("example-producer", move || {
            sender.send(1).expect("consumers remain connected");
            let second = match sender.try_send(2) {
                Err(channel::TrySendError::Full(value)) => value,
                other => panic!("capacity-one queue must apply backpressure: {other:?}"),
            };
            filled.send(()).expect("driver waits for backpressure");
            sender.send(second).expect("consumer releases backpressure");
            sender.send(3).expect("consumers remain connected");
            thread::current().id()
        });
        ready.recv_timeout(WAIT).expect("producer fills bounded queue");
        assert!(tasks.try_spawn_blocking_named("example-producer", || 0).is_none());
        println!("producer: bounded queue full; busy worker rejected");

        // Cloned receivers compete on one bounded MPMC queue, not independent copies.
        let second_receiver = receiver.clone();
        let consumer = tasks.spawn_blocking_named("example-consumer-a", move || {
            time::sleep(Duration::from_millis(2));
            let sum: u64 = (0..3)
                .map(|_| receiver.recv_timeout(WAIT).expect("producer supplies three values"))
                .sum();
            (thread::current().id(), sum)
        });
        let second_consumer = tasks.spawn_blocking_named("example-consumer-b", move || {
            time::sleep(Duration::from_millis(2));
            let sum: u64 = (0..3)
                .map(|_| {
                    second_receiver.recv_timeout(WAIT).expect("producer supplies three values")
                })
                .sum();
            (thread::current().id(), sum)
        });
        let second_producer = tasks.spawn_blocking_named("example-producer-b", move || {
            for value in 4..=6 {
                second_sender.send(value).expect("consumers remain connected");
            }
            thread::current().id()
        });
        let (first_id, first_sum) = consumer.get();
        let (second_id, second_sum) = second_consumer.get();
        assert_ne!(producer.get(), first_id);
        assert_ne!(producer.get(), second_id);
        assert_ne!(first_id, second_id);
        assert_ne!(second_producer.get(), first_id);
        assert_ne!(second_producer.get(), second_id);
        assert_ne!(producer.get(), second_producer.get());
        assert_eq!(first_sum + second_sum, 21);
        println!("mpmc: two producers and two consumers on distinct workers; six values; sum=21");
        let reused = tasks
            .try_spawn_blocking_named("example-producer", || thread::current().id())
            .expect("completed named worker accepts another job");
        assert_eq!(reused.get(), producer.get());
        println!("producer: idle named worker reused");

        let (keep_connected, empty) = channel::bounded::<u64>(1);
        let started = Instant::now();
        assert_eq!(
            empty.recv_timeout(Duration::from_millis(3)),
            Err(channel::RecvTimeoutError::Timeout)
        );
        let elapsed = started.elapsed();
        assert!(elapsed >= Duration::from_millis(3));
        if cfg!(feature = "fuzz") {
            assert_eq!(elapsed, Duration::from_millis(3));
        }
        drop(keep_connected);
        println!("clock: empty connected queue timed out after three milliseconds");

        // Rc<Cell<_>> proves initialization, access, replacement, and drop accept non-Send state.
        let stateful = WorkerStatefulPool::new(tasks.state_trie_overlay_worker_pool());
        let drops = Arc::new(AtomicUsize::new(0));
        let wrong_thread_drops = Arc::new(AtomicUsize::new(0));
        stateful.init::<WorkerState>(|previous| {
            assert!(previous.is_none());
            WorkerState {
                value: Rc::new(Cell::new(40 + pool::current_thread_index().expect("pool worker"))),
                creator: current().id(),
                drops: Arc::clone(&drops),
                wrong_thread_drops: Arc::clone(&wrong_thread_drops),
            }
        });
        stateful.init::<WorkerState>(|previous| {
            let index = pool::current_thread_index().expect("initialization runs on a pool worker");
            let previous = previous.expect("worker state survives successive broadcasts");
            assert_eq!(previous.creator, current().id());
            assert_eq!(previous.value.get(), 40 + index);
            previous.value.set(previous.value.get() + 2);
            WorkerState {
                value: Rc::clone(&previous.value),
                creator: current().id(),
                drops: Arc::clone(&drops),
                wrong_thread_drops: Arc::clone(&wrong_thread_drops),
            }
        });
        assert_eq!(drops.load(Ordering::SeqCst), 2);
        let value = stateful.install_fn(|| {
            stateful.get::<WorkerState, _>(|state| {
                assert_eq!(state.creator, current().id());
                assert_eq!(
                    state.value.get(),
                    42 + pool::current_thread_index().expect("pool worker")
                );
                state.value.get()
            })
        });
        assert!((42..44).contains(&value));
        println!("pool: two non-Send worker-local states retained and updated");

        let inputs = [1_u64, 2, 3, 4];
        let mut squares = [0; 4];
        stateful.in_place_scope(|scope| {
            for (input, output) in inputs.iter().zip(&mut squares) {
                scope.spawn(move |_| *output = input * input);
            }
        });
        assert_eq!(squares, [1, 4, 9, 16]);
        let mut borrowed_sum = 0;
        thread::scope(|scope| {
            let worker = scope.spawn(|| borrowed_sum = squares.iter().sum::<u64>());
            worker.join().expect("borrowed scoped worker completes");
        });
        assert_eq!(borrowed_sum, 30);
        println!("scope: borrowed pool and thread work joined; squares=[1, 4, 9, 16]; sum=30");
        drop(stateful);
        assert_eq!(drops.load(Ordering::SeqCst), 4);
        assert_eq!(wrong_thread_drops.load(Ordering::SeqCst), 0);
        println!("pool: four non-Send states dropped on their creating physical threads");
        drop(tasks);
        asynchronous
    });
    assert_eq!(tokio.block_on(asynchronous).expect("native async task completes"), 42);
    println!("async: native Tokio task completed; result=42");
}
