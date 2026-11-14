//! Selected-channel ownership and disconnection regressions.

use reth_tasks::executor::{channel, thread, Runtime};

#[test]
fn mpsc_receiver_drop_releases_payloads_before_sender() {
    Runtime::new().run(|| {
        let (producer, receiver) = channel::mpsc::channel();
        let (dropped, notices) = channel::mpsc::channel();
        for index in 0..2 {
            let producer = producer.clone();
            let dropped = dropped.clone();
            assert!(producer
                .send(DropNotice(Box::new({
                    let producer = producer.clone();
                    move || {
                        // Payload destruction must run after the channel's mutex is unlocked.
                        assert!(producer.send(DropNotice(Box::new(|| {}))).is_err());
                        dropped.send(index).unwrap();
                    }
                })))
                .is_ok());
        }
        drop(receiver);
        let mut released = notices.try_iter().collect::<Vec<_>>();
        released.sort_unstable();
        assert_eq!(released, vec![0, 1]);
        drop(producer);
    });
}

#[test]
fn mpsc_bounded_receiver_drop_releases_buffered_payloads() {
    Runtime::new().run(|| {
        let (producer, receiver) = channel::mpsc::sync_channel(2);
        let (dropped, notices) = channel::mpsc::channel();
        for index in 0..2 {
            let dropped = dropped.clone();
            assert!(producer
                .send(DropNotice(Box::new(move || dropped.send(index).unwrap())))
                .is_ok());
        }
        drop(receiver);
        let mut released = notices.try_iter().collect::<Vec<_>>();
        released.sort_unstable();
        assert_eq!(released, vec![0, 1]);
        drop(producer);
    });
}

#[test]
fn mpsc_receiver_drop_disconnects_buffered_nested_receiver_with_live_sender() {
    Runtime::new().run(|| {
        let (completer, nested_receiver) = channel::mpsc::channel::<()>();
        let (dropped, notice) = channel::mpsc::channel();
        let (producer, receiver) = channel::mpsc::channel();
        assert!(producer
            .send((nested_receiver, DropNotice(Box::new(move || dropped.send(()).unwrap()))))
            .is_ok());

        drop(receiver);
        assert_eq!(completer.send(()), Err(channel::mpsc::SendError(())));
        assert_eq!(notice.try_recv(), Ok(()));
        drop(producer);
    });
}

#[test]
fn rendezvous_receiver_drop_returns_unsent_payload() {
    Runtime::new().run(|| {
        let (producer, receiver) = channel::bounded(0);
        let (started, sending) = channel::bounded(0);
        let sender = thread::spawn(move || {
            started.send(()).unwrap();
            producer.send(vec![1, 2, 3])
        });
        sending.recv().unwrap();
        thread::yield_now();
        drop(receiver);
        assert_eq!(sender.join().unwrap(), Err(channel::SendError(vec![1, 2, 3])));
    });
}

struct DropNotice(Box<dyn Fn() + Send>);

impl Drop for DropNotice {
    fn drop(&mut self) {
        self.0();
    }
}
