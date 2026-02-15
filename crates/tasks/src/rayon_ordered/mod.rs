use std::{
    cell::UnsafeCell,
    mem::MaybeUninit,
    sync::atomic::{AtomicBool, Ordering},
};

use rayon::iter::{IndexedParallelIterator, ParallelIterator};

mod bitmap;
use bitmap::AtomicBitmap;

struct Slot<T>(UnsafeCell<MaybeUninit<T>>);

// SAFETY: Each index is written by exactly one producer and read by exactly one consumer.
// The AtomicBitmap synchronizes access (Release on set, Acquire on check).
unsafe impl<T: Send> Sync for Slot<T> {}
unsafe impl<T: Send> Send for Slot<T> {}

struct Shared<T> {
    slots: Box<[Slot<T>]>,
    ready: AtomicBitmap,
    panicked: AtomicBool,
}

impl<T> Shared<T> {
    fn new(n: usize) -> Self {
        let slots = (0..n).map(|_| Slot(UnsafeCell::new(MaybeUninit::uninit()))).collect();
        Self { slots, ready: AtomicBitmap::new(n), panicked: AtomicBool::new(false) }
    }

    /// # Safety
    /// Index `i` must be in bounds and must only be written once.
    unsafe fn write(&self, i: usize, value: T) {
        unsafe { (*self.slots[i].0.get()).write(value) };
        self.ready.set(i, Ordering::Release);
    }

    /// # Safety
    /// Must only be called after observing readiness via `ready.is_set(i)`.
    unsafe fn take(&self, i: usize) -> T {
        let v = unsafe { (*self.slots[i].0.get()).assume_init_read() };
        self.ready.clear(i, Ordering::Relaxed);
        v
    }
}

impl<T> Drop for Shared<T> {
    fn drop(&mut self) {
        for idx in self.ready.iter_set_mut() {
            unsafe {
                (*self.slots[idx].0.get()).assume_init_drop();
            }
        }
    }
}

/// Extension trait for [`IndexedParallelIterator`]
/// that streams results to a sequential consumer in index order.
pub trait ForEachOrdered: IndexedParallelIterator {
    /// Executes the parallel iterator, calling `f` on each result **sequentially in index
    /// order**.
    ///
    /// Items are computed in parallel, but `f` is invoked as `f(item_0)`, `f(item_1)`, …,
    /// `f(item_{n-1})` on the calling thread. The calling thread receives each item as soon
    /// as it (and all preceding items) are ready.
    ///
    /// `f` does **not** need to be [`Send`] — it runs exclusively on the calling thread.
    fn for_each_ordered<F>(self, f: F)
    where
        Self::Item: Send,
        F: FnMut(Self::Item);
}

impl<I: IndexedParallelIterator> ForEachOrdered for I {
    fn for_each_ordered<F>(self, f: F)
    where
        Self::Item: Send,
        F: FnMut(Self::Item),
    {
        ordered_impl(self, f);
    }
}

fn ordered_impl<I, F>(iter: I, mut f: F)
where
    I: IndexedParallelIterator,
    I::Item: Send,
    F: FnMut(I::Item),
{
    use std::panic::{catch_unwind, AssertUnwindSafe};

    let n = iter.len();
    if n == 0 {
        return;
    }

    let shared = Shared::<I::Item>::new(n);

    rayon::in_place_scope(|s| {
        s.spawn(|_| {
            let res = catch_unwind(AssertUnwindSafe(|| {
                iter.enumerate().for_each(|(i, item)| {
                    // SAFETY: `enumerate()` on an IndexedParallelIterator yields each
                    // index exactly once.
                    unsafe {
                        shared.write(i, item);
                    }
                });
            }));
            if let Err(payload) = res {
                shared.panicked.store(true, Ordering::Release);
                std::panic::resume_unwind(payload);
            }
        });

        // Consumer: sequential, ordered, on the calling thread.
        let mut spins = 0u32;
        for i in 0..n {
            loop {
                if shared.ready.is_set(i, Ordering::Acquire) {
                    break;
                }
                if shared.panicked.load(Ordering::Relaxed) {
                    return;
                }
                // Yield to rayon's work-stealing so the producer can make progress,
                // especially important when the thread pool is small.
                if rayon::yield_now() == Some(rayon::Yield::Executed) {
                    continue;
                }
                std::hint::spin_loop();
                spins = spins.wrapping_add(1);
                if spins.is_multiple_of(128) {
                    std::thread::yield_now();
                }
            }
            spins = 0;
            // SAFETY: we just observed the ready bit with Acquire ordering.
            let value = unsafe { shared.take(i) };
            f(value);
        }
    });
}
