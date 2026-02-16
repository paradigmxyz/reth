use crossbeam_utils::CachePadded;
use rayon::iter::{IndexedParallelIterator, ParallelIterator};
use std::{
    cell::UnsafeCell,
    mem::MaybeUninit,
    sync::atomic::{AtomicBool, Ordering},
};

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

/// A cache-line-padded slot with an atomic ready flag.
struct Slot<T> {
    value: UnsafeCell<MaybeUninit<T>>,
    ready: AtomicBool,
}

// SAFETY: Each slot is written by exactly one producer and read by exactly one consumer.
// The AtomicBool synchronizes access (Release on write, Acquire on read).
unsafe impl<T: Send> Send for Slot<T> {}
unsafe impl<T: Send> Sync for Slot<T> {}

struct Shared<T> {
    slots: Box<[CachePadded<Slot<T>>]>,
    panicked: AtomicBool,
}

impl<T> Shared<T> {
    fn new(n: usize) -> Self {
        Self {
            // SAFETY: Zero is a valid bit pattern for Slot.
            // Needs to be zero for `ready` to be false.
            slots: unsafe {
                Box::<[_]>::assume_init(Box::<[CachePadded<Slot<T>>]>::new_zeroed_slice(n))
            },
            panicked: AtomicBool::new(false),
        }
    }

    /// # Safety
    /// Index `i` must be in bounds and must only be written once.
    #[inline]
    unsafe fn write(&self, i: usize, val: T) {
        let slot = unsafe { self.slots.get_unchecked(i) };
        unsafe { (*slot.value.get()).write(val) };
        slot.ready.store(true, Ordering::Release);
    }

    /// # Safety
    /// Index `i` must be in bounds. Must only be called after `ready` is observed `true`.
    #[inline]
    unsafe fn take(&self, i: usize) -> T {
        let slot = unsafe { self.slots.get_unchecked(i) };
        let v = unsafe { (*slot.value.get()).assume_init_read() };
        // Clear ready so Drop doesn't double-free this slot.
        slot.ready.store(false, Ordering::Relaxed);
        v
    }

    #[inline]
    fn is_ready(&self, i: usize) -> bool {
        // SAFETY: caller ensures `i < n`.
        unsafe { self.slots.get_unchecked(i) }.ready.load(Ordering::Acquire)
    }
}

impl<T> Drop for Shared<T> {
    fn drop(&mut self) {
        for slot in &mut *self.slots {
            if *slot.ready.get_mut() {
                unsafe { (*slot.value.get()).assume_init_drop() };
            }
        }
    }
}

/// Executes a parallel iterator and delivers results to a sequential callback in index order.
///
/// This works by pre-allocating one cache-line-padded slot per item. Each slot holds an
/// `UnsafeCell<MaybeUninit<T>>` and an `AtomicBool` ready flag. A rayon task computes all
/// items in parallel, writing each result into its slot and setting the flag (`Release`).
/// The calling thread walks slots 0, 1, 2, … in order, spinning on the flag (`Acquire`),
/// then reading the value and passing it to `f`.
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
        // Producer: compute items in parallel and write them into their slots.
        s.spawn(|_| {
            let res = catch_unwind(AssertUnwindSafe(|| {
                iter.enumerate().for_each(|(i, item)| {
                    // SAFETY: `enumerate()` on an IndexedParallelIterator yields each
                    // index exactly once.
                    unsafe { shared.write(i, item) };
                });
            }));
            if let Err(payload) = res {
                shared.panicked.store(true, Ordering::Release);
                std::panic::resume_unwind(payload);
            }
        });

        // Consumer: sequential, ordered, on the calling thread.
        // Exponential backoff: 1, 2, 4, …, 64 pause instructions, then OS yields.
        const SPIN_SHIFT_LIMIT: u32 = 6;
        for i in 0..n {
            let mut backoff = 0u32;
            'wait: loop {
                if shared.is_ready(i) {
                    break 'wait;
                }

                if shared.panicked.load(Ordering::Relaxed) {
                    return;
                }

                // Yield to rayon's work-stealing so the producer can make progress,
                // especially important when the thread pool is small.
                if rayon::yield_now() == Some(rayon::Yield::Executed) {
                    continue 'wait;
                }

                if backoff < SPIN_SHIFT_LIMIT {
                    for _ in 0..(1u32 << backoff) {
                        std::hint::spin_loop();
                    }
                    backoff += 1;
                } else {
                    // Producer is genuinely slow; fall back to OS-level yield.
                    std::thread::yield_now();
                }
            }
            // SAFETY: `i < n` and we just observed the ready flag with Acquire ordering.
            let value = unsafe { shared.take(i) };
            f(value);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use rayon::prelude::*;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Barrier,
    };

    #[test]
    fn preserves_order() {
        let input: Vec<u64> = (0..1000).collect();
        let mut output = Vec::with_capacity(input.len());
        input.par_iter().map(|x| x * 2).for_each_ordered(|x| output.push(x));
        let expected: Vec<u64> = (0..1000).map(|x| x * 2).collect();
        assert_eq!(output, expected);
    }

    #[test]
    fn empty_iterator() {
        let input: Vec<u64> = vec![];
        let mut output = Vec::new();
        input.par_iter().map(|x| *x).for_each_ordered(|x| output.push(x));
        assert!(output.is_empty());
    }

    #[test]
    fn single_element() {
        let mut output = Vec::new();
        vec![42u64].par_iter().map(|x| *x).for_each_ordered(|x| output.push(x));
        assert_eq!(output, vec![42]);
    }

    #[test]
    fn slow_early_items_still_delivered_in_order() {
        // Item 0 is deliberately delayed; all other items complete quickly.
        // The consumer must still deliver items in order 0, 1, 2, … regardless.
        let barrier = Barrier::new(2);
        let n = 64usize;
        let input: Vec<usize> = (0..n).collect();
        let mut output = Vec::with_capacity(n);

        input
            .par_iter()
            .map(|&i| {
                if i == 0 {
                    // Wait until at least one other item has been produced.
                    barrier.wait();
                } else if i == n - 1 {
                    // Signal that other items are ready.
                    barrier.wait();
                }
                i
            })
            .for_each_ordered(|x| output.push(x));

        assert_eq!(output, input);
    }

    #[test]
    fn drops_unconsumed_slots_on_panic() {
        static DROP_COUNT: AtomicUsize = AtomicUsize::new(0);

        #[derive(Clone)]
        struct Tracked(#[allow(dead_code)] u64);
        impl Drop for Tracked {
            fn drop(&mut self) {
                DROP_COUNT.fetch_add(1, Ordering::Relaxed);
            }
        }

        DROP_COUNT.store(0, Ordering::Relaxed);

        let input: Vec<u64> = (0..100).collect();
        let result = std::panic::catch_unwind(|| {
            input
                .par_iter()
                .map(|&i| {
                    assert!(i != 50, "intentional");
                    Tracked(i)
                })
                .for_each_ordered(|_item| {});
        });

        assert!(result.is_err());
        // All produced Tracked values must have been dropped (either consumed or cleaned up).
        // We can't assert an exact count since the panic may cut production short.
        let drops = DROP_COUNT.load(Ordering::Relaxed);
        assert!(drops > 0, "some items should have been dropped");
    }

    #[test]
    fn no_double_drop() {
        // Verify that consumed items are dropped exactly once (not double-freed by Drop).
        static DROP_COUNT: AtomicUsize = AtomicUsize::new(0);

        struct Counted(#[allow(dead_code)] u64);
        impl Drop for Counted {
            fn drop(&mut self) {
                DROP_COUNT.fetch_add(1, Ordering::Relaxed);
            }
        }

        DROP_COUNT.store(0, Ordering::Relaxed);
        let n = 200u64;
        let input: Vec<u64> = (0..n).collect();
        input.par_iter().map(|&i| Counted(i)).for_each_ordered(|_item| {});

        assert_eq!(DROP_COUNT.load(Ordering::Relaxed), n as usize);
    }

    #[test]
    fn callback_is_not_send() {
        // Verify that the callback does not need to be Send.
        use std::rc::Rc;
        let counter = Rc::new(std::cell::Cell::new(0u64));
        let input: Vec<u64> = (0..100).collect();
        input.par_iter().map(|&x| x).for_each_ordered(|x| {
            counter.set(counter.get() + x);
        });
        assert_eq!(counter.get(), (0..100u64).sum::<u64>());
    }
}
