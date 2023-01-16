//! Helper for shutdown signals

use futures_util::{
    future::{FusedFuture, Shared},
    FutureExt,
};
use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};
use tokio::sync::oneshot;

/// A Future that resolves when the shutdown event has been fired.
#[derive(Debug, Clone)]
pub struct Shutdown(Shared<oneshot::Receiver<()>>);

impl Future for Shutdown {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let pin = self.get_mut();
        if pin.0.is_terminated() || pin.0.poll_unpin(cx).is_ready() {
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    }
}

/// Shutdown signal that fires either manually or on drop by closing the channel
#[derive(Debug)]
pub struct Signal(oneshot::Sender<()>);

impl Signal {
    /// Fire the signal manually.
    pub fn fire(self) {
        let _ = self.0.send(());
    }
}

/// Create a channel pair that's used to propagate shutdown event
pub fn signal() -> (Signal, Shutdown) {
    let (sender, receiver) = oneshot::channel();
    (Signal(sender), Shutdown(receiver.shared()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::future::join_all;
    use std::time::Duration;

    #[tokio::test(flavor = "multi_thread")]
    async fn test_shutdown() {
        let (_signal, _shutdown) = signal();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_drop_signal() {
        let (signal, shutdown) = signal();

        tokio::task::spawn(async move {
            tokio::time::sleep(Duration::from_millis(500)).await;
            drop(signal)
        });

        shutdown.await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_multi_shutdowns() {
        let (signal, shutdown) = signal();

        let mut tasks = Vec::with_capacity(100);
        for _ in 0..100 {
            let shutdown = shutdown.clone();
            let task = tokio::task::spawn(async move {
                shutdown.await;
            });
            tasks.push(task);
        }

        drop(signal);

        join_all(tasks).await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_drop_signal_from_thread() {
        let (signal, shutdown) = signal();

        let _thread = std::thread::spawn(|| {
            std::thread::sleep(Duration::from_millis(500));
            drop(signal)
        });

        shutdown.await;
    }
}
