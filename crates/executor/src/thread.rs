//! Standard-compatible threads and borrowed scopes for the selected backend.

pub use crate::time::sleep;
#[cfg(feature = "fuzz")]
pub use shuttle::thread::{
    current, panicking, park, scope, yield_now, Result, Scope, ScopedJoinHandle, Thread, ThreadId,
};
#[cfg(not(feature = "fuzz"))]
pub use std::thread::{
    current, panicking, park, scope, spawn, yield_now, Builder, JoinHandle, Result, Scope,
    ScopedJoinHandle, Thread, ThreadId,
};

/// Parks until unparked or the selected clock reaches the timeout.
pub fn park_timeout(duration: crate::time::Duration) {
    #[cfg(not(feature = "fuzz"))]
    std::thread::park_timeout(duration);
    #[cfg(feature = "fuzz")]
    {
        let timer = crate::deterministic::Timer::new(crate::time::Instant::now() + duration);
        shuttle::thread::park();
        drop(timer);
    }
}

/// Returns available native parallelism, or one logical execution thread in simulation.
#[cfg_attr(
    feature = "fuzz",
    expect(clippy::missing_const_for_fn, reason = "The native implementation queries the OS.")
)]
pub fn available_parallelism() -> std::io::Result<std::num::NonZeroUsize> {
    #[cfg(not(feature = "fuzz"))]
    {
        std::thread::available_parallelism()
    }
    #[cfg(feature = "fuzz")]
    {
        Ok(std::num::NonZeroUsize::MIN)
    }
}

/// A logical thread builder; scoped tasks inherit their scope's stack settings.
#[cfg(feature = "fuzz")]
#[derive(Debug, Default)]
pub struct Builder(shuttle::thread::Builder);

#[cfg(feature = "fuzz")]
impl Builder {
    /// Creates a thread builder with default settings.
    pub fn new() -> Self {
        Self(shuttle::thread::Builder::new())
    }
    /// Sets the name of a non-scoped logical thread.
    pub fn name(self, name: String) -> Self {
        Self(self.0.name(name))
    }
    /// Sets stack size for a non-scoped logical thread.
    pub fn stack_size(self, size: usize) -> Self {
        Self(self.0.stack_size(size))
    }
    /// Spawns a named logical thread in the current simulation.
    pub fn spawn<F, T>(self, work: F) -> std::io::Result<JoinHandle<T>>
    where
        F: FnOnce() -> T + Send + 'static,
        T: Send + 'static,
    {
        let finished = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let completion = Completion(finished.clone());
        let handle = self.0.spawn(move || {
            let _completion = completion;
            work()
        })?;
        Ok(JoinHandle { handle, finished })
    }
    /// Spawns a borrowing logical thread; Shuttle does not support scoped metadata.
    ///
    /// Configured name and stack size apply only to non-scoped threads.
    pub fn spawn_scoped<'scope, 'env, F, T>(
        self,
        scope: &'scope Scope<'scope, 'env>,
        work: F,
    ) -> std::io::Result<ScopedJoinHandle<'scope, T>>
    where
        F: FnOnce() -> T + Send + 'scope,
        T: Send + 'scope,
    {
        Ok(scope.spawn(work))
    }
}

/// Permission to join a logical thread and inspect whether its workload has finished.
#[cfg(feature = "fuzz")]
#[derive(Debug)]
pub struct JoinHandle<T> {
    handle: shuttle::thread::JoinHandle<T>,
    finished: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

#[cfg(feature = "fuzz")]
impl<T> JoinHandle<T> {
    /// Waits for termination, including thread-local cleanup, and returns the workload's result.
    pub fn join(self) -> Result<T> {
        self.handle.join()
    }

    /// Returns the logical thread associated with this handle.
    pub fn thread(&self) -> &Thread {
        self.handle.thread()
    }

    /// Returns whether the workload has returned or unwound; final thread cleanup may still run.
    pub fn is_finished(&self) -> bool {
        self.finished.load(std::sync::atomic::Ordering::Acquire)
    }
}

/// Spawns a logical thread in the current deterministic execution.
#[cfg(feature = "fuzz")]
pub fn spawn<F, T>(work: F) -> JoinHandle<T>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    Builder::new().spawn(work).expect("Shuttle logical thread creation is infallible")
}

#[cfg(feature = "fuzz")]
struct Completion(std::sync::Arc<std::sync::atomic::AtomicBool>);

#[cfg(feature = "fuzz")]
impl Drop for Completion {
    fn drop(&mut self) {
        self.0.store(true, std::sync::atomic::Ordering::Release);
    }
}
