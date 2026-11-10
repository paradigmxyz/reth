//! The same blocking caller exercises either backend without a runtime trait.
use reth_executor::{
    channel,
    sync::{Arc, Condvar, Mutex, OnceLock},
    thread,
    time::{Duration, Instant},
    Runtime,
};

fn cancelled_sends() {
    for capacity in [0, 1] {
        let (sender, receiver) = channel::bounded(capacity);
        if capacity == 1 {
            sender.send(String::from("buffered")).expect("receiver connected");
        }
        let (cancel, cancelled) = channel::unbounded();
        thread::scope(|scope| {
            let blocked = scope
                .spawn(|| channel::send_or_cancel(&sender, String::from("unsent"), &cancelled));
            // Virtual time cannot advance until the sender is blocked.
            thread::sleep(Duration::from_millis(1));
            cancel.send(()).expect("cancellation receiver connected");
            assert_eq!(
                blocked.join().expect("sender completed"),
                Err(channel::SendError(String::from("unsent")))
            );
        });
        if capacity == 1 {
            assert_eq!(receiver.recv().expect("buffered value retained"), "buffered");
        }
        assert_eq!(receiver.try_recv(), Err(channel::TryRecvError::Empty));
    }
    let (sender, receiver) = channel::bounded(0);
    let (cancel, cancelled) = channel::unbounded::<()>();
    thread::scope(|scope| {
        let blocked = scope.spawn(|| channel::send_or_cancel(&sender, 7, &cancelled));
        thread::sleep(Duration::from_millis(1));
        drop(cancel);
        assert_eq!(blocked.join().expect("sender completed"), Err(channel::SendError(7)));
    });
    assert_eq!(receiver.try_recv(), Err(channel::TryRecvError::Empty));
    let (sender, receiver) = channel::bounded(0);
    let (cancel, cancelled) = channel::unbounded();
    thread::scope(|scope| {
        let committed = scope.spawn(|| channel::send_or_cancel(&sender, 9, &cancelled));
        assert_eq!(receiver.recv().expect("rendezvous committed"), 9);
        cancel.send(()).expect("cancellation receiver connected");
        assert_eq!(committed.join().expect("sender completed"), Ok(()));
    });
}

fn workload() -> Vec<String> {
    cancelled_sends();
    let (finish, finished) = channel::bounded(0);
    let worker = thread::spawn(move || finished.recv().expect("completion signal"));
    assert!(!worker.is_finished());
    finish.send(()).expect("worker waits for completion");
    while !worker.is_finished() {
        thread::sleep(Duration::from_millis(1));
    }
    worker.join().expect("completed worker joins");
    let trace = Arc::new(Mutex::new(Vec::new()));
    let ready = Arc::new(OnceLock::new());
    let start = Instant::now();
    let (send, receive) = channel::bounded(1);
    let (rendezvous, ack) = channel::bounded(0);
    thread::scope(|scope| {
        let trace = &trace;
        let ready = &ready;
        scope.spawn(move || {
            ready.set(3).expect("cell is initialized once");
            for value in 0..3 {
                send.send(value).expect("consumer remains connected");
                trace.lock().expect("trace is not poisoned").push(format!("sent {value}"));
            }
            drop(send);
            rendezvous.send("done").expect("ack receiver remains connected");
        });
        scope.spawn(move || {
            assert_eq!(*ready.wait(), 3);
            for value in receive {
                thread::sleep(Duration::from_millis(1));
                trace.lock().expect("trace is not poisoned").push(format!("received {value}"));
            }
            assert_eq!(ack.recv().expect("producer acknowledges"), "done");
        });
    });
    assert!(start.elapsed() >= Duration::from_millis(3));
    let lock = Mutex::new(false);
    let condition = Condvar::new();
    let before_timeout = Instant::now();
    let (guard, timeout) = condition
        .wait_timeout(lock.lock().expect("mutex is not poisoned"), Duration::from_millis(1))
        .expect("mutex is not poisoned");
    assert!(timeout.timed_out());
    assert!(!*guard);
    drop(guard);
    assert!(before_timeout.elapsed() >= Duration::from_millis(1));
    thread::scope(|scope| {
        let handle = thread::Builder::new()
            .spawn_scoped(scope, || {
                let mut guard = lock.lock().expect("mutex is not poisoned");
                *guard = true;
                condition.notify_one();
            })
            .expect("spawn logical thread");
        let guard = condition
            .wait_timeout_while(
                lock.lock().expect("mutex is not poisoned"),
                Duration::from_secs(1),
                |ready| !*ready,
            )
            .expect("mutex is not poisoned")
            .0;
        assert!(*guard);
        drop(guard);
        handle.join().expect("notification thread completed");
    });
    let (send, receive) = channel::unbounded::<u8>();
    assert_eq!(
        receive.recv_timeout(Duration::from_millis(2)),
        Err(channel::RecvTimeoutError::Timeout)
    );
    send.send(9).expect("receiver remains connected");
    let never = channel::never::<u8>();
    let selected = channel::select_biased! {
        recv(never) -> result => result.expect("never is not selected"),
        recv(receive) -> result => result.expect("queued value"),
    };
    assert_eq!(selected, 9);
    drop(send);
    assert_eq!(receive.recv(), Err(channel::RecvError));
    Arc::try_unwrap(trace)
        .expect("scoped trace owners completed")
        .into_inner()
        .expect("trace is not poisoned")
}

fn main() {
    let runtime = Runtime::new();
    let trace = runtime.run(workload);
    #[cfg(feature = "fuzz")]
    for _ in 0..4 {
        assert_eq!(trace, runtime.run(workload));
    }
    println!("{}", trace.join(", "));
}
