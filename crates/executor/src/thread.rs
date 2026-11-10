//! Standard-compatible threads and borrowed scopes for the selected backend.

#[cfg(feature = "fuzz")]
use std::{
    marker::PhantomData,
    panic::{catch_unwind, resume_unwind, AssertUnwindSafe},
    sync::{
        atomic::{AtomicBool, AtomicPtr, Ordering},
        Arc, Mutex,
    },
};

pub use crate::time::sleep;
#[cfg(feature = "fuzz")]
pub use shuttle::thread::{current, panicking, park, yield_now, Result, Thread, ThreadId};
#[cfg(not(feature = "fuzz"))]
pub use std::thread::{
    current, panicking, park, scope, spawn, yield_now, Builder, JoinHandle, Result, Scope,
    ScopedJoinHandle, Thread, ThreadId,
};

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
        let finished = Arc::new(AtomicBool::new(false));
        let completion = Completion(finished.clone());
        let handle = self.0.spawn(move || {
            let _completion = completion;
            catch_unwind(AssertUnwindSafe(work))
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
    handle: shuttle::thread::JoinHandle<Result<T>>,
    finished: Arc<AtomicBool>,
}

#[cfg(feature = "fuzz")]
impl<T> JoinHandle<T> {
    /// Waits for termination, including thread-local cleanup, and returns the workload's result.
    pub fn join(self) -> Result<T> {
        self.handle.join()?
    }

    /// Returns the logical thread associated with this handle.
    pub fn thread(&self) -> &Thread {
        self.handle.thread()
    }

    /// Returns whether the workload has returned or unwound; final thread cleanup may still run.
    pub fn is_finished(&self) -> bool {
        self.finished.load(Ordering::Acquire)
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

/// A borrowing scope that drains its children before propagating a panic.
#[cfg(feature = "fuzz")]
#[derive(Debug)]
pub struct Scope<'scope, 'env> {
    inner: AtomicPtr<()>,
    child_panics: Mutex<Vec<Arc<AtomicBool>>>,
    // Both lifetimes are invariant, as on the underlying borrowing scope.
    lifetimes: PhantomData<&'scope mut &'scope &'env mut &'env ()>,
}

#[cfg(feature = "fuzz")]
impl<'scope, 'env> Scope<'scope, 'env> {
    /// Spawns a borrowing thread; an unjoined panic propagates after the scope drains.
    pub fn spawn<F, T>(&'scope self, work: F) -> ScopedJoinHandle<'scope, T>
    where
        F: FnOnce() -> T + Send + 'scope,
        T: Send + 'scope,
    {
        let finished = Arc::new(AtomicBool::new(false));
        let completion = Completion(finished.clone());
        let panicked = Arc::new(AtomicBool::new(false));
        self.child_panics.lock().unwrap_or_else(|error| error.into_inner()).push(panicked.clone());
        let child_panicked = panicked.clone();
        // SAFETY: `scope` initializes this pointer before exposing `self`. It keeps this wrapper
        // alive until Shuttle drains every child, even if the callback panics. The invariant
        // lifetimes prevent either the wrapper or a borrowing child from escaping that scope.
        let inner = unsafe {
            &*self.inner.load(Ordering::Acquire).cast::<shuttle::thread::Scope<'scope, 'env>>()
        };
        let handle = inner.spawn(move || {
            let _completion = completion;
            let result = catch_unwind(AssertUnwindSafe(work));
            child_panicked.store(result.is_err(), Ordering::Release);
            result
        });
        ScopedJoinHandle { handle, finished, panicked, lifetime: PhantomData }
    }
}

/// Permission to join a borrowing logical thread and recover its workload's panic.
#[cfg(feature = "fuzz")]
#[derive(Debug)]
pub struct ScopedJoinHandle<'scope, T> {
    handle: shuttle::thread::ScopedJoinHandle<'scope, Result<T>>,
    finished: Arc<AtomicBool>,
    panicked: Arc<AtomicBool>,
    lifetime: PhantomData<&'scope mut &'scope ()>,
}

// SAFETY: Shuttle's handle synchronizes result access internally. Its `PhantomData<&T>` is not
// an actual shared borrow, so the panic payload need not be Sync to move or share the handle.
#[cfg(feature = "fuzz")]
unsafe impl<T: Send> Send for ScopedJoinHandle<'_, T> {}

// SAFETY: Shared access only exposes thread metadata and atomic flags; joining consumes the handle.
#[cfg(feature = "fuzz")]
unsafe impl<T: Send> Sync for ScopedJoinHandle<'_, T> {}

#[cfg(feature = "fuzz")]
impl<T> ScopedJoinHandle<'_, T> {
    /// Waits for termination and returns `Err` if the workload panicked.
    ///
    /// Joining handles the panic, so it will not propagate again at the end of the scope.
    pub fn join(self) -> Result<T> {
        let result = self.handle.join().and_then(std::convert::identity);
        self.panicked.store(false, Ordering::Release);
        result
    }

    /// Returns the logical thread associated with this handle.
    pub fn thread(&self) -> &Thread {
        self.handle.thread()
    }

    /// Returns whether the workload has returned or unwound; final thread cleanup may still run.
    pub fn is_finished(&self) -> bool {
        self.finished.load(Ordering::Acquire)
    }
}

/// Runs a borrowing scope, waiting for all children even when the callback panics.
///
/// Unjoined child panics propagate after draining. A callback panic takes precedence.
#[cfg(feature = "fuzz")]
pub fn scope<'env, F, T>(work: F) -> T
where
    F: for<'scope> FnOnce(&'scope Scope<'scope, 'env>) -> T,
{
    let scope = Scope {
        inner: AtomicPtr::new(std::ptr::null_mut()),
        child_panics: Mutex::new(Vec::new()),
        lifetimes: PhantomData,
    };
    let result = shuttle::thread::scope(|inner| {
        scope.inner.store(std::ptr::from_ref(inner).cast_mut().cast(), Ordering::Release);
        // Shuttle only drains children when its callback returns normally.
        catch_unwind(AssertUnwindSafe(|| work(&scope)))
    });
    match result {
        Err(panic) => resume_unwind(panic),
        Ok(value) => {
            assert!(
                !scope
                    .child_panics
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .iter()
                    .any(|panicked| panicked.load(Ordering::Acquire)),
                "a scoped thread panicked"
            );
            value
        }
    }
}

#[cfg(feature = "fuzz")]
struct Completion(Arc<AtomicBool>);

#[cfg(feature = "fuzz")]
impl Drop for Completion {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}
