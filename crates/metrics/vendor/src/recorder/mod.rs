use std::{cell::Cell, marker::PhantomData, ptr::NonNull};

mod cell;
use self::cell::RecorderOnceCell;

mod errors;
pub use self::errors::SetRecorderError;

mod noop;
pub use self::noop::NoopRecorder;

use crate::{Counter, Gauge, Histogram, Key, KeyName, Metadata, SharedString, Unit};

static NOOP_RECORDER: NoopRecorder = NoopRecorder;
static GLOBAL_RECORDER: RecorderOnceCell = RecorderOnceCell::new();

thread_local! {
    static LOCAL_RECORDER: Cell<Option<NonNull<dyn Recorder>>> = Cell::new(None);
}

/// A trait for registering and recording metrics.
///
/// This is the core trait that allows interoperability between exporter implementations and the macros provided by
/// `metrics`.
pub trait Recorder {
    /// Describes a counter.
    ///
    /// Callers may provide the unit or a description of the counter being registered. Whether or not a metric can be
    /// re-registered to provide a unit/description, if one was already passed or not, as well as how units/descriptions
    /// are used by the underlying recorder, is an implementation detail.
    fn describe_counter(&self, key: KeyName, unit: Option<Unit>, description: SharedString);

    /// Describes a gauge.
    ///
    /// Callers may provide the unit or a description of the gauge being registered. Whether or not a metric can be
    /// re-registered to provide a unit/description, if one was already passed or not, as well as how units/descriptions
    /// are used by the underlying recorder, is an implementation detail.
    fn describe_gauge(&self, key: KeyName, unit: Option<Unit>, description: SharedString);

    /// Describes a histogram.
    ///
    /// Callers may provide the unit or a description of the histogram being registered. Whether or not a metric can be
    /// re-registered to provide a unit/description, if one was already passed or not, as well as how units/descriptions
    /// are used by the underlying recorder, is an implementation detail.
    fn describe_histogram(&self, key: KeyName, unit: Option<Unit>, description: SharedString);

    /// Registers a counter.
    fn register_counter(&self, key: &Key, metadata: &Metadata<'_>) -> Counter;

    /// Registers a gauge.
    fn register_gauge(&self, key: &Key, metadata: &Metadata<'_>) -> Gauge;

    /// Registers a histogram.
    fn register_histogram(&self, key: &Key, metadata: &Metadata<'_>) -> Histogram;
}

// Blanket implementations.
macro_rules! impl_recorder {
    ($inner_ty:ident, $ptr_ty:ty) => {
        impl<$inner_ty> $crate::Recorder for $ptr_ty
        where
            $inner_ty: $crate::Recorder + ?Sized,
        {
            fn describe_counter(
                &self,
                key: $crate::KeyName,
                unit: Option<$crate::Unit>,
                description: $crate::SharedString,
            ) {
                std::ops::Deref::deref(self).describe_counter(key, unit, description)
            }

            fn describe_gauge(
                &self,
                key: $crate::KeyName,
                unit: Option<$crate::Unit>,
                description: $crate::SharedString,
            ) {
                std::ops::Deref::deref(self).describe_gauge(key, unit, description)
            }

            fn describe_histogram(
                &self,
                key: $crate::KeyName,
                unit: Option<$crate::Unit>,
                description: $crate::SharedString,
            ) {
                std::ops::Deref::deref(self).describe_histogram(key, unit, description)
            }

            fn register_counter(
                &self,
                key: &$crate::Key,
                metadata: &$crate::Metadata<'_>,
            ) -> $crate::Counter {
                std::ops::Deref::deref(self).register_counter(key, metadata)
            }

            fn register_gauge(
                &self,
                key: &$crate::Key,
                metadata: &$crate::Metadata<'_>,
            ) -> $crate::Gauge {
                std::ops::Deref::deref(self).register_gauge(key, metadata)
            }

            fn register_histogram(
                &self,
                key: &$crate::Key,
                metadata: &$crate::Metadata<'_>,
            ) -> $crate::Histogram {
                std::ops::Deref::deref(self).register_histogram(key, metadata)
            }
        }
    };
}

impl_recorder!(T, &T);
impl_recorder!(T, &mut T);
impl_recorder!(T, std::boxed::Box<T>);
impl_recorder!(T, std::sync::Arc<T>);

/// Guard for setting a local recorder.
///
/// When using a local recorder, we take a reference to the recorder and only hold it for as long as the duration of the
/// closure. However, we must store this reference in a static variable (thread-local storage) so that it can be
/// accessed by the macros. This guard ensures that the pointer we store to the reference is cleared when the guard is
/// dropped, so that it can't be used after the closure has finished, even if the closure panics and unwinds the stack.
///
/// ## Note
///
/// The guard has a lifetime parameter `'a` that is bounded using a `PhantomData` type. This upholds the guard's
/// contravariance, it must live _at most as long_ as the recorder it takes a reference to. The bounded lifetime
/// prevents accidental use-after-free errors when using a guard directly through [`crate::set_default_local_recorder`].
pub struct LocalRecorderGuard<'a> {
    prev_recorder: Option<NonNull<dyn Recorder>>,
    phantom: PhantomData<&'a dyn Recorder>,
}

impl<'a> LocalRecorderGuard<'a> {
    /// Creates a new `LocalRecorderGuard` and sets the thread-local recorder.
    fn new(recorder: &'a (dyn Recorder + 'a)) -> Self {
        // SAFETY: We extend `'a` to `'static` to satisfy the signature of `LOCAL_RECORDER`, which
        // has an implied `'static` bound on `dyn Recorder`. We enforce that all usages of `LOCAL_RECORDER`
        // are limited to `'a` as we mediate its access entirely through `LocalRecorderGuard<'a>`.
        let recorder_ptr = unsafe {
            std::mem::transmute::<*const (dyn Recorder + 'a), *mut (dyn Recorder + 'static)>(
                recorder as &'a (dyn Recorder + 'a),
            )
        };
        // SAFETY: While we take a lifetime-less pointer to the given reference, the reference we derive _from_ the
        // pointer is given the same lifetime of the reference used to construct the guard -- captured in the guard type
        // itself -- and so derived references never outlive the source reference.
        let recorder_ptr = unsafe { NonNull::new_unchecked(recorder_ptr) };

        let prev_recorder =
            LOCAL_RECORDER.with(|local_recorder| local_recorder.replace(Some(recorder_ptr)));

        Self { prev_recorder, phantom: PhantomData }
    }
}

impl<'a> Drop for LocalRecorderGuard<'a> {
    fn drop(&mut self) {
        // Clear the thread-local recorder.
        LOCAL_RECORDER.with(|local_recorder| local_recorder.replace(self.prev_recorder.take()));
    }
}

/// Sets the global recorder.
///
/// This function may only be called once in the lifetime of a program. Any metrics recorded before this method is
/// called will be completely ignored.
///
/// This function does not typically need to be called manually.  Metrics implementations should provide an
/// initialization method that installs the recorder internally.
///
/// # Errors
///
/// An error is returned if a recorder has already been set.
pub fn set_global_recorder<R>(recorder: R) -> Result<(), SetRecorderError<R>>
where
    R: Recorder + Sync + 'static,
{
    GLOBAL_RECORDER.set(recorder)
}

/// Sets the recorder as the default for the current thread for the duration of the lifetime of the returned
/// [`LocalRecorderGuard`].
///
/// This function is suitable for capturing metrics in asynchronous code, particularly when using a single-threaded
/// runtime. Any metrics registered prior to the returned guard will remain attached to the recorder that was present at
/// the time of registration, so this cannot be used to intercept existing metrics.
///
/// Additionally, local recorders can be used in a nested fashion. When setting a new default local recorder, the
/// previous default local recorder will be captured if one was set, and will be restored when the returned guard drops.
/// the lifetime of the returned [`LocalRecorderGuard`].
///
/// Any metrics recorded before a guard is returned will be completely ignored.  Metrics implementations should provide
/// an initialization method that installs the recorder internally.
///
/// The function is suitable for capturing metrics in asynchronous code that uses a single threaded runtime.
///
/// If a global recorder is set, it will be restored once the guard is dropped.
#[must_use]
pub fn set_default_local_recorder(recorder: &dyn Recorder) -> LocalRecorderGuard<'_> {
    LocalRecorderGuard::new(recorder)
}

/// Runs the closure with the given recorder set as the global recorder for the duration.
///
/// This only applies as long as the closure is running, and on the thread where `with_local_recorder` is called. This
/// does not extend to other threads, and so is not suitable for capturing metrics in asynchronous code where multiple
/// threads are involved.
pub fn with_local_recorder<T>(recorder: &dyn Recorder, f: impl FnOnce() -> T) -> T {
    let _local = LocalRecorderGuard::new(recorder);
    f()
}

/// Runs the closure with a reference to the current recorder for this scope.
///
/// If a local recorder has been set, it will be used. Otherwise, the global recorder will be used.  If neither a local
/// recorder or global recorder have been set, a no-op recorder will be used.
///
/// It should typically not be necessary to call this function directly, as it is used primarily by generated code. You
/// should prefer working with the macros provided by `metrics` instead: `counter!`, `gauge!`, `histogram!`, etc.
#[cfg(not(feature = "fuzz"))]
pub fn with_recorder<T>(f: impl FnOnce(&dyn Recorder) -> T) -> T {
    LOCAL_RECORDER.with(|local_recorder| {
        if let Some(recorder) = local_recorder.get() {
            // SAFETY: If we have a local recorder, we know that it is valid because it can only be set during the
            // duration of a closure that is passed to `with_local_recorder`, which is the only time this method can be
            // called and have a local recorder set. This ensures that the lifetime of the recorder is valid for the
            // duration of this method call.
            unsafe { f(recorder.as_ref()) }
        } else if let Some(global_recorder) = GLOBAL_RECORDER.try_load() {
            f(global_recorder)
        } else {
            f(&NOOP_RECORDER)
        }
    })
}

/// Runs the closure with an inert recorder, without consulting native TLS or global state.
///
/// This disables facade instrumentation even when an exporter installed a recorder. Recorder
/// installation APIs remain unchanged so application setup and explicit recorder users keep their
/// original behavior.
#[cfg(feature = "fuzz")]
pub fn with_recorder<T>(f: impl FnOnce(&dyn Recorder) -> T) -> T {
    f(&NOOP_RECORDER)
}

#[cfg(test)]
mod tests {
    use std::sync::{atomic::Ordering, Arc};

    use crate::{with_local_recorder, NoopRecorder};

    use super::{Recorder, RecorderOnceCell};

    #[test]
    fn boxed_recorder_dropped_on_existing_set() {
        // This test simply ensures that if a boxed recorder is handed to us to install, and another
        // recorder has already been installed, that we drop the new boxed recorder instead of
        // leaking it.
        let recorder_cell = RecorderOnceCell::new();

        // This is the first set of the cell, so it should always succeed.
        let (first_recorder, _) = test_recorders::TrackOnDropRecorder::new();
        let first_set_result = recorder_cell.set(first_recorder);
        assert!(first_set_result.is_ok());

        // Since the cell is already set, this second set should fail. We'll also then assert that
        // our atomic boolean is set to `true`, indicating the drop logic ran for it.
        let (second_recorder, was_dropped) = test_recorders::TrackOnDropRecorder::new();
        assert!(!was_dropped.load(Ordering::SeqCst));

        let second_set_result = recorder_cell.set(second_recorder);
        assert!(second_set_result.is_err());
        assert!(!was_dropped.load(Ordering::SeqCst));
        drop(second_set_result);
        assert!(was_dropped.load(Ordering::SeqCst));
    }

    #[test]
    fn blanket_implementations() {
        fn is_recorder<T: Recorder>(_recorder: T) {}

        let mut local = NoopRecorder;

        is_recorder(NoopRecorder);
        is_recorder(Arc::new(NoopRecorder));
        is_recorder(Box::new(NoopRecorder));
        is_recorder(&local);
        is_recorder(&mut local);
    }

    #[test]
    fn thread_scoped_recorder_guards() {
        // This test ensures that when a recorder is installed through
        // `crate::set_default_local_recorder` it will only be valid in the scope of the
        // thread.
        //
        // The goal of the test is to give confidence that no invalid memory
        // access errors are present when operating with locally scoped
        // recorders.
        let t1_recorder = test_recorders::SimpleCounterRecorder::new();
        let t2_recorder = test_recorders::SimpleCounterRecorder::new();
        let t3_recorder = test_recorders::SimpleCounterRecorder::new();
        // Start a new thread scope to take references to each recorder in the
        // closures passed to the thread.
        std::thread::scope(|s| {
            s.spawn(|| {
                let _guard = crate::set_default_local_recorder(&t1_recorder);
                crate::counter!("t1_counter").increment(1);
            });

            s.spawn(|| {
                with_local_recorder(&t2_recorder, || {
                    crate::counter!("t2_counter").increment(2);
                })
            });

            s.spawn(|| {
                let _guard = crate::set_default_local_recorder(&t3_recorder);
                crate::counter!("t3_counter").increment(3);
            });
        });

        assert!(t1_recorder.get_value() == 1);
        assert!(t2_recorder.get_value() == 2);
        assert!(t3_recorder.get_value() == 3);
    }

    #[test]
    fn local_recorder_restored_when_dropped() {
        // This test ensures that any previously installed local recorders are
        // restored when the subsequently installed recorder's guard is dropped.
        let root_recorder = test_recorders::SimpleCounterRecorder::new();
        // Install the root recorder and increment the counter once.
        let _guard = crate::set_default_local_recorder(&root_recorder);
        crate::counter!("test_counter").increment(1);

        // Install a second recorder and increment its counter once.
        let next_recorder = test_recorders::SimpleCounterRecorder::new();
        let next_guard = crate::set_default_local_recorder(&next_recorder);
        crate::counter!("test_counter").increment(1);
        let final_recorder = test_recorders::SimpleCounterRecorder::new();
        crate::with_local_recorder(&final_recorder, || {
            // Final recorder increments the counter by 10. At the end of the
            // closure, the guard should be dropped, and `next_recorder`
            // restored.
            crate::counter!("test_counter").increment(10);
        });
        // Since `next_recorder` is restored, we can increment it once and check
        // that the value is 2 (+1 before and after the closure).
        crate::counter!("test_counter").increment(1);
        assert!(next_recorder.get_value() == 2);
        drop(next_guard);

        // At the end, increment the counter again by an arbitrary value. Since
        // `next_guard` is dropped, the root recorder is restored.
        crate::counter!("test_counter").increment(20);
        assert!(root_recorder.get_value() == 21);
    }

    mod test_recorders {
        use std::sync::{
            atomic::{AtomicBool, AtomicU64, Ordering},
            Arc,
        };

        use crate::Recorder;

        #[derive(Debug)]
        // Tracks how many times the recorder was dropped
        pub struct TrackOnDropRecorder(Arc<AtomicBool>);

        impl TrackOnDropRecorder {
            pub fn new() -> (Self, Arc<AtomicBool>) {
                let arc = Arc::new(AtomicBool::new(false));
                (Self(arc.clone()), arc)
            }
        }

        // === impl TrackOnDropRecorder ===

        impl Recorder for TrackOnDropRecorder {
            fn describe_counter(
                &self,
                _: crate::KeyName,
                _: Option<crate::Unit>,
                _: crate::SharedString,
            ) {
            }
            fn describe_gauge(
                &self,
                _: crate::KeyName,
                _: Option<crate::Unit>,
                _: crate::SharedString,
            ) {
            }
            fn describe_histogram(
                &self,
                _: crate::KeyName,
                _: Option<crate::Unit>,
                _: crate::SharedString,
            ) {
            }

            fn register_counter(&self, _: &crate::Key, _: &crate::Metadata<'_>) -> crate::Counter {
                crate::Counter::noop()
            }

            fn register_gauge(&self, _: &crate::Key, _: &crate::Metadata<'_>) -> crate::Gauge {
                crate::Gauge::noop()
            }

            fn register_histogram(
                &self,
                _: &crate::Key,
                _: &crate::Metadata<'_>,
            ) -> crate::Histogram {
                crate::Histogram::noop()
            }
        }

        impl Drop for TrackOnDropRecorder {
            fn drop(&mut self) {
                self.0.store(true, Ordering::SeqCst);
            }
        }

        // A simple recorder that only implements `register_counter`.
        #[derive(Debug)]
        pub struct SimpleCounterRecorder {
            state: Arc<AtomicU64>,
        }

        impl SimpleCounterRecorder {
            pub fn new() -> Self {
                Self { state: Arc::new(AtomicU64::default()) }
            }

            pub fn get_value(&self) -> u64 {
                self.state.load(Ordering::Acquire)
            }
        }

        struct SimpleCounterHandle {
            state: Arc<AtomicU64>,
        }

        impl crate::CounterFn for SimpleCounterHandle {
            fn increment(&self, value: u64) {
                self.state.fetch_add(value, Ordering::Acquire);
            }

            fn absolute(&self, _value: u64) {
                unimplemented!()
            }
        }

        // === impl SimpleCounterRecorder ===

        impl Recorder for SimpleCounterRecorder {
            fn describe_counter(
                &self,
                _: crate::KeyName,
                _: Option<crate::Unit>,
                _: crate::SharedString,
            ) {
            }
            fn describe_gauge(
                &self,
                _: crate::KeyName,
                _: Option<crate::Unit>,
                _: crate::SharedString,
            ) {
            }
            fn describe_histogram(
                &self,
                _: crate::KeyName,
                _: Option<crate::Unit>,
                _: crate::SharedString,
            ) {
            }

            fn register_counter(&self, _: &crate::Key, _: &crate::Metadata<'_>) -> crate::Counter {
                crate::Counter::from_arc(Arc::new(SimpleCounterHandle {
                    state: self.state.clone(),
                }))
            }

            fn register_gauge(&self, _: &crate::Key, _: &crate::Metadata<'_>) -> crate::Gauge {
                crate::Gauge::noop()
            }

            fn register_histogram(
                &self,
                _: &crate::Key,
                _: &crate::Metadata<'_>,
            ) -> crate::Histogram {
                crate::Histogram::noop()
            }
        }
    }
}
