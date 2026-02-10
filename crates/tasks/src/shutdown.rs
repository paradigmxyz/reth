//! Helper for shutdown signals

use futures_util::{
    future::{FusedFuture, Shared},
    FutureExt,
};
use std::{
    future::Future,
    pin::Pin,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Condvar, Mutex,
    },
    task::{ready, Context, Poll},
};
use tokio::sync::oneshot;

/// A Future that resolves when the shutdown event has been fired.
#[derive(Debug)]
pub struct GracefulShutdown {
    shutdown: Shutdown,
    guard: Option<GracefulShutdownGuard>,
}

impl GracefulShutdown {
    pub(crate) const fn new(shutdown: Shutdown, guard: GracefulShutdownGuard) -> Self {
        Self { shutdown, guard: Some(guard) }
    }

    /// Returns a new shutdown future that ignores the returned [`GracefulShutdownGuard`].
    ///
    /// This just maps the return value of the future to `()`, it does not drop the guard.
    pub fn ignore_guard(self) -> impl Future<Output = ()> + Send + Sync + Unpin + 'static {
        self.map(drop)
    }
}

impl Future for GracefulShutdown {
    type Output = GracefulShutdownGuard;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        ready!(self.shutdown.poll_unpin(cx));
        Poll::Ready(self.get_mut().guard.take().expect("Future polled after completion"))
    }
}

impl Clone for GracefulShutdown {
    fn clone(&self) -> Self {
        Self {
            shutdown: self.shutdown.clone(),
            guard: self.guard.as_ref().map(|g| GracefulShutdownGuard::new(Arc::clone(&g.0))),
        }
    }
}

/// A guard that fires once dropped to signal the [`TaskManager`](crate::TaskManager) that the
/// [`GracefulShutdown`] has completed.
#[derive(Debug)]
#[must_use = "if unused the task will not be gracefully shutdown"]
pub struct GracefulShutdownGuard(Arc<GracefulShutdownCounter>);

impl GracefulShutdownGuard {
    pub(crate) fn new(counter: Arc<GracefulShutdownCounter>) -> Self {
        counter.count.fetch_add(1, Ordering::SeqCst);
        Self(counter)
    }
}

impl Drop for GracefulShutdownGuard {
    fn drop(&mut self) {
        if self.0.count.fetch_sub(1, Ordering::SeqCst) == 1 {
            let _lock = self.0.mutex.lock().unwrap();
            self.0.cvar.notify_all();
        }
    }
}

/// Tracks the number of active graceful shutdown tasks and provides
/// a condvar for efficient waiting until all tasks complete.
#[derive(Debug)]
pub struct GracefulShutdownCounter {
    count: AtomicUsize,
    mutex: Mutex<()>,
    cvar: Condvar,
}

impl GracefulShutdownCounter {
    /// Creates a new counter with zero active tasks.
    pub fn new() -> Self {
        Self { count: AtomicUsize::new(0), mutex: Mutex::new(()), cvar: Condvar::new() }
    }

    /// Returns the current number of active graceful shutdown tasks.
    pub fn count(&self) -> usize {
        self.count.load(Ordering::Relaxed)
    }

    /// Blocks the current thread until all graceful shutdown tasks have completed.
    pub fn wait(&self) {
        let mut lock = self.mutex.lock().unwrap();
        while self.count.load(Ordering::SeqCst) > 0 {
            lock = self.cvar.wait(lock).unwrap();
        }
    }

    /// Blocks the current thread until all graceful shutdown tasks have completed or the
    /// timeout elapses.
    ///
    /// Returns `true` if all tasks completed before the timeout.
    pub fn wait_timeout(&self, timeout: std::time::Duration) -> bool {
        let deadline = std::time::Instant::now() + timeout;
        let mut lock = self.mutex.lock().unwrap();
        while self.count.load(Ordering::SeqCst) > 0 {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                return false;
            }
            let (new_lock, result) = self.cvar.wait_timeout(lock, remaining).unwrap();
            lock = new_lock;
            if result.timed_out() && self.count.load(Ordering::SeqCst) > 0 {
                return false;
            }
        }
        true
    }
}

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
