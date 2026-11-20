//! Blocking execution primitives selected at compile time.
//!
//! The default backend uses native threads, Crossbeam channels, and Rayon. With
//! `fuzz`, run callers inside `Runtime::run`: stackful Shuttle tasks execute on
//! one OS thread in fixed round-robin order. Only primitives from this crate may
//! block simulated tasks; native blocking or OS I/O is outside the simulation.

#[cfg(not(feature = "fuzz"))]
pub use reth_primitives_traits::sync::{LazyLock, OnceLock};

#[cfg(all(feature = "std", not(feature = "fuzz")))]
pub use std::{sync::*, thread_local};

#[cfg(feature = "fuzz")]
pub use shuttle::thread_local;
#[cfg(feature = "fuzz")]
pub use shuttle_sync::sync::{
    atomic, Arc, Barrier, BarrierWaitResult, LockResult, Once, OnceState, PoisonError, RwLock,
    RwLockReadGuard, RwLockWriteGuard, TryLockError, TryLockResult, Weak,
};

#[cfg(feature = "std")]
pub mod channel;
#[cfg(feature = "std")]
pub mod iter;
#[cfg(feature = "std")]
pub mod pool;
#[cfg(feature = "std")]
pub mod runtime;

#[cfg(feature = "std")]
pub mod thread;
#[cfg(feature = "std")]
pub mod time;

#[cfg(feature = "fuzz")]
mod deterministic;

#[cfg(feature = "fuzz")]
mod condvar;
#[cfg(feature = "fuzz")]
pub use condvar::{Condvar, WaitTimeoutResult};

#[cfg(feature = "fuzz")]
mod mutex;
#[cfg(feature = "fuzz")]
pub use mutex::{Mutex, MutexGuard};

#[cfg(feature = "fuzz")]
pub use channel::mpsc;

#[cfg(feature = "fuzz")]
mod once;
#[cfg(feature = "fuzz")]
pub use once::{LazyLock, OnceLock};

#[cfg(feature = "std")]
pub use pool::{ThreadPool, ThreadPoolBuilder};

#[cfg(feature = "std")]
pub use runtime::Runtime;

/// Rayon-compatible compute operations selected with the executor backend.
#[cfg(feature = "std")]
pub mod rayon {
    pub use crate::sync::pool::*;
    /// Parallel iterator extension traits for the selected compute backend.
    pub mod prelude {
        #[cfg(feature = "fuzz")]
        pub use crate::sync::iter::*;
        #[cfg(not(feature = "fuzz"))]
        pub use ::rayon::prelude::*;
    }
    /// Slice sorting operations for the selected compute backend.
    pub mod slice {
        #[cfg(feature = "fuzz")]
        pub use crate::sync::iter::ParallelSliceMut;
        #[cfg(not(feature = "fuzz"))]
        pub use ::rayon::slice::*;
    }
    /// Parallel iterator types for the selected compute backend.
    pub mod iter {
        pub use crate::sync::iter::*;
    }
}

/// Native non-poisoning locks, or their Shuttle equivalents under `fuzz`.
///
/// Shuttle's `RwLock` currently queues readers behind pending writers even when an
/// upgradable reader prevents that writer from acquiring the lock. Native `parking_lot`
/// admits readers in that situation, so simulations of this pattern are unsupported.
#[cfg(feature = "std")]
pub mod parking_lot {
    #[cfg(not(feature = "fuzz"))]
    pub use ::parking_lot::*;
    #[cfg(feature = "fuzz")]
    pub use shuttle_parking_lot::*;
}

/// Whether these primitives require an active simulated runtime.
///
/// Crates with backend-specific integration must select the same `fuzz` feature.
pub const IS_SIMULATED: bool = cfg!(feature = "fuzz");

#[cfg(all(test, feature = "std"))]
mod tests {
    use super::*;
    use crate::sync::{thread, time, Runtime};
    use std::{
        any::Any,
        panic::{catch_unwind, AssertUnwindSafe},
    };

    #[test]
    fn lazy_lock_initializes_once_for_racing_threads() {
        Runtime::new().run(|| {
            let calls = atomic::AtomicUsize::new(0);
            let lazy = LazyLock::new(|| {
                calls.fetch_add(1, atomic::Ordering::SeqCst);
                // A simulated initializer must be able to block while contenders wait.
                time::sleep(time::Duration::from_millis(1));
                42
            });
            thread::scope(|scope| {
                let handles = (0..4)
                    .map(|_| scope.spawn(|| assert_eq!(*LazyLock::force(&lazy), 42)))
                    .collect::<Vec<_>>();
                for handle in handles {
                    handle.join().unwrap();
                }
            });
            assert_eq!(*lazy, 42);
            assert_eq!(calls.load(atomic::Ordering::SeqCst), 1);
        });
    }

    #[test]
    fn lazy_lock_poison_wakes_contenders_without_retrying_initializer() {
        Runtime::new().run(|| {
            let calls = atomic::AtomicUsize::new(0);
            let lazy = LazyLock::new(|| -> usize {
                calls.fetch_add(1, atomic::Ordering::SeqCst);
                time::sleep(time::Duration::from_millis(1));
                panic!("lazy initializer failed");
            });
            let mut messages = thread::scope(|scope| {
                let handles = (0..2)
                    .map(|_| {
                        scope.spawn(|| {
                            panic_message(
                                catch_unwind(AssertUnwindSafe(|| LazyLock::force(&lazy)))
                                    .unwrap_err(),
                            )
                        })
                    })
                    .collect::<Vec<_>>();
                handles.into_iter().map(|handle| handle.join().unwrap()).collect::<Vec<_>>()
            });
            messages.sort();
            assert_eq!(
                messages,
                ["LazyLock instance has previously been poisoned", "lazy initializer failed"]
            );
            assert_eq!(calls.load(atomic::Ordering::SeqCst), 1);
        });
    }

    #[test]
    fn static_lazy_lock_retains_value_across_executions() {
        static VALUE: LazyLock<Arc<usize>> = LazyLock::new(|| {
            thread::yield_now();
            Arc::new(42)
        });
        let values = (0..2)
            .map(|_| {
                Runtime::new().run(|| {
                    thread::scope(|scope| {
                        let first = scope.spawn(|| LazyLock::force(&VALUE));
                        let second = scope.spawn(|| LazyLock::force(&VALUE));
                        let first = first.join().unwrap();
                        let second = second.join().unwrap();
                        assert!(std::ptr::eq(first, second));
                        assert_eq!(**second, 42);
                        first.clone()
                    })
                })
            })
            .collect::<Vec<_>>();
        // Keeping the first allocation alive also detects a newly initialized replacement.
        assert!(Arc::ptr_eq(&values[0], &values[1]));
    }

    #[test]
    fn static_lazy_lock_retains_poison_across_executions() {
        static VALUE: LazyLock<usize> = LazyLock::new(|| panic!("static initializer failed"));
        for expected in
            ["static initializer failed", "LazyLock instance has previously been poisoned"]
        {
            Runtime::new().run(move || {
                assert_eq!(
                    panic_message(
                        catch_unwind(AssertUnwindSafe(|| LazyLock::force(&VALUE))).unwrap_err()
                    ),
                    expected
                );
            });
        }
    }

    #[test]
    fn lazy_lock_default_and_value_drop() {
        Runtime::new().run(|| {
            let default: LazyLock<usize> = LazyLock::default();
            assert_eq!(*default, 0);
            let dropped = Arc::new(atomic::AtomicUsize::new(0));
            let lazy = LazyLock::new({
                let dropped = dropped.clone();
                move || DropCounter(dropped)
            });
            LazyLock::force(&lazy);
            assert_eq!(dropped.load(atomic::Ordering::SeqCst), 0);
            drop(lazy);
            assert_eq!(dropped.load(atomic::Ordering::SeqCst), 1);
        });
    }

    #[cfg(not(feature = "fuzz"))]
    #[test]
    fn native_lazy_lock_is_the_standard_type() {
        let standard = std::sync::LazyLock::new(|| 42);
        let selected: LazyLock<usize, _> = standard;
        let round_trip: std::sync::LazyLock<usize, _> = selected;
        assert_eq!(*std::sync::LazyLock::force(&round_trip), 42);
    }

    struct DropCounter(Arc<atomic::AtomicUsize>);

    impl Drop for DropCounter {
        fn drop(&mut self) {
            self.0.fetch_add(1, atomic::Ordering::SeqCst);
        }
    }

    fn panic_message(panic: Box<dyn Any + Send>) -> String {
        if let Some(message) = panic.downcast_ref::<&str>() {
            (*message).to_owned()
        } else {
            *panic.downcast::<String>().unwrap()
        }
    }
}
