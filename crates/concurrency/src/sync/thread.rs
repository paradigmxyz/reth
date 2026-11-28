//! Standard-compatible threads and borrowed scopes for the selected backend.
//!
//! In simulation, `panicking()` reads the shared OS thread's unwind flag, not a logical task's
//! state. It can therefore report another task's panic while an unwinding destructor yields.

use std::{io, num::NonZeroUsize};

#[cfg(all(feature = "fuzz", dst))]
use shuttle::{sync::atomic::AtomicBool, thread};
#[cfg(all(feature = "fuzz", dst))]
use std::{
    convert::identity,
    marker::PhantomData,
    panic::{catch_unwind, resume_unwind, AssertUnwindSafe},
    ptr,
    sync::{
        atomic::{AtomicPtr, Ordering},
        Arc, Mutex,
    },
};

pub use crate::sync::time::sleep;
#[cfg(all(feature = "fuzz", dst))]
pub use shuttle::thread::{current, panicking, park, yield_now, Result, Thread, ThreadId};
#[cfg(not(all(feature = "fuzz", dst)))]
pub use std::thread::{
    current, panicking, park, scope, spawn, yield_now, Builder, JoinHandle, Result, Scope,
    ScopedJoinHandle, Thread, ThreadId,
};

/// A logical thread builder; scoped tasks inherit their scope's stack settings.
#[cfg(all(feature = "fuzz", dst))]
#[derive(Debug, Default)]
pub struct Builder(thread::Builder);

#[cfg(all(feature = "fuzz", dst))]
impl Builder {
    /// Creates a thread builder with default settings.
    pub fn new() -> Self {
        Self(thread::Builder::new())
    }
    /// Sets the name of a non-scoped logical thread.
    pub fn name(self, name: String) -> Self {
        Self(self.0.name(name))
    }
    /// Sets stack size for a non-scoped logical thread.
    ///
    /// Shuttle stack-allocation failures can panic during `spawn` instead of returning an I/O
    /// error. Invalid sizes and allocation-failure handling are not simulated like native threads.
    pub fn stack_size(self, size: usize) -> Self {
        Self(self.0.stack_size(size))
    }
    /// Spawns a named logical thread in the current simulation.
    pub fn spawn<F, T>(self, work: F) -> io::Result<JoinHandle<T>>
    where
        F: FnOnce() -> T + Send + 'static,
        T: Send + 'static,
    {
        let finished = Arc::new(AtomicBool::new(false));
        // Publish completion inside the logical task, not from a destructor during teardown.
        let completed = finished.clone();
        let handle = self.0.spawn(move || {
            let result = catch_unwind(AssertUnwindSafe(work));
            completed.store(true, Ordering::Release);
            result
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
    ) -> io::Result<ScopedJoinHandle<'scope, T>>
    where
        F: FnOnce() -> T + Send + 'scope,
        T: Send + 'scope,
    {
        Ok(scope.spawn(work))
    }
}

/// Permission to join a logical thread and inspect whether its workload has finished.
#[cfg(all(feature = "fuzz", dst))]
#[derive(Debug)]
pub struct JoinHandle<T> {
    handle: thread::JoinHandle<Result<T>>,
    finished: Arc<AtomicBool>,
}

#[cfg(all(feature = "fuzz", dst))]
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

/// A borrowing scope that drains its children before propagating a panic.
#[cfg(all(feature = "fuzz", dst))]
#[derive(Debug)]
pub struct Scope<'scope, 'env> {
    // Published before children can access it and unchanged until every child has drained.
    #[allow(
        clippy::disallowed_types,
        reason = "Native pointer publishes the borrowed Shuttle scope."
    )]
    inner: AtomicPtr<()>,
    #[allow(
        clippy::disallowed_types,
        reason = "Native storage is never locked across a scheduler operation."
    )]
    child_panics: Mutex<Vec<Arc<AtomicBool>>>,
    // Both lifetimes are invariant, as on the underlying borrowing scope.
    lifetimes: PhantomData<&'scope mut &'scope &'env mut &'env ()>,
}

#[cfg(all(feature = "fuzz", dst))]
impl<'scope, 'env> Scope<'scope, 'env> {
    /// Spawns a borrowing thread; an unjoined panic propagates after the scope drains.
    #[allow(
        clippy::disallowed_methods,
        reason = "The bookkeeping lock only appends a flag before spawning."
    )]
    pub fn spawn<F, T>(&'scope self, work: F) -> ScopedJoinHandle<'scope, T>
    where
        F: FnOnce() -> T + Send + 'scope,
        T: Send + 'scope,
    {
        let finished = Arc::new(AtomicBool::new(false));
        let completed = finished.clone();
        let panicked = Arc::new(AtomicBool::new(false));
        self.child_panics.lock().unwrap_or_else(|error| error.into_inner()).push(panicked.clone());
        let child_panicked = panicked.clone();
        // SAFETY: `scope` initializes this pointer before exposing `self`. It keeps this wrapper
        // alive until Shuttle drains every child, even if the callback panics. The invariant
        // lifetimes prevent either the wrapper or a borrowing child from escaping that scope.
        let inner =
            unsafe { &*self.inner.load(Ordering::Acquire).cast::<thread::Scope<'scope, 'env>>() };
        let handle = inner.spawn(move || {
            let result = catch_unwind(AssertUnwindSafe(work));
            child_panicked.store(result.is_err(), Ordering::Release);
            completed.store(true, Ordering::Release);
            result
        });
        ScopedJoinHandle { handle, finished, panicked, lifetime: PhantomData }
    }
}

/// Permission to join a borrowing logical thread and recover its workload's panic.
#[cfg(all(feature = "fuzz", dst))]
#[derive(Debug)]
pub struct ScopedJoinHandle<'scope, T> {
    handle: thread::ScopedJoinHandle<'scope, Result<T>>,
    finished: Arc<AtomicBool>,
    panicked: Arc<AtomicBool>,
    lifetime: PhantomData<&'scope mut &'scope ()>,
}

#[cfg(all(feature = "fuzz", dst))]
impl<T> ScopedJoinHandle<'_, T> {
    /// Waits for termination and returns `Err` if the workload panicked.
    ///
    /// Joining handles the panic, so it will not propagate again at the end of the scope.
    pub fn join(self) -> Result<T> {
        let result = self.handle.join().and_then(identity);
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

// SAFETY: Shuttle's handle synchronizes result access internally. Its `PhantomData<&T>` is not
// an actual shared borrow, so the panic payload need not be Sync to move or share the handle.
#[cfg(all(feature = "fuzz", dst))]
unsafe impl<T: Send> Send for ScopedJoinHandle<'_, T> {}

// SAFETY: Shared access only exposes thread metadata and atomic flags; joining consumes the handle.
#[cfg(all(feature = "fuzz", dst))]
unsafe impl<T: Send> Sync for ScopedJoinHandle<'_, T> {}

/// Returns available native parallelism, or one logical execution thread in simulation.
#[cfg_attr(
    all(feature = "fuzz", dst),
    expect(clippy::missing_const_for_fn, reason = "The native implementation queries the OS.")
)]
pub fn available_parallelism() -> io::Result<NonZeroUsize> {
    #[cfg(not(all(feature = "fuzz", dst)))]
    {
        std::thread::available_parallelism()
    }
    #[cfg(all(feature = "fuzz", dst))]
    {
        Ok(NonZeroUsize::MIN)
    }
}

/// Spawns a logical thread in the current deterministic execution.
#[cfg(all(feature = "fuzz", dst))]
pub fn spawn<F, T>(work: F) -> JoinHandle<T>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    Builder::new().spawn(work).expect("Shuttle logical thread creation is infallible")
}

/// Runs a borrowing scope, waiting for all children even when the callback panics.
///
/// Unjoined child panics propagate after draining. A callback panic takes precedence.
#[cfg(all(feature = "fuzz", dst))]
#[allow(
    clippy::disallowed_types,
    clippy::disallowed_methods,
    reason = "Native scope pointer and panic-list storage are initialized before spawning and read after children drain."
)]
pub fn scope<'env, F, T>(work: F) -> T
where
    F: for<'scope> FnOnce(&'scope Scope<'scope, 'env>) -> T,
{
    let scope = Scope {
        inner: AtomicPtr::new(ptr::null_mut()),
        child_panics: Mutex::new(Vec::new()),
        lifetimes: PhantomData,
    };
    let result = thread::scope(|inner| {
        scope.inner.store(ptr::from_ref(inner).cast_mut().cast(), Ordering::Release);
        // Shuttle only drains children when its callback returns normally.
        catch_unwind(AssertUnwindSafe(|| work(&scope)))
    });
    match result {
        Err(panic) => resume_unwind(panic),
        Ok(value) => {
            let child_panics = std::mem::take(
                &mut *scope.child_panics.lock().unwrap_or_else(|error| error.into_inner()),
            );
            assert!(
                !child_panics.iter().any(|panicked| panicked.load(Ordering::Acquire)),
                "a scoped thread panicked"
            );
            value
        }
    }
}
