use crossbeam_utils::CachePadded;
use rayon::iter::{IndexedParallelIterator, ParallelIterator};
use parking_lot::{Condvar, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};

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
    ///
    /// # Blocking
    ///
    /// The calling thread blocks (via [`Condvar`]) while waiting for the next item to become
    /// ready. It does **not** participate in rayon's work-stealing while blocked. Callers
    /// should invoke this from a dedicated blocking thread (e.g. via
    /// [`tokio::task::spawn_blocking`]) rather than from within the rayon thread pool.
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

/// A slot holding an optional value and a condvar for notification.
struct Slot<T> {
    value: Mutex<Option<T>>,
    notify: Condvar,
}

impl<T> Slot<T> {
    const fn new() -> Self {
        Self { value: Mutex::new(None), notify: Condvar::new() }
    }
}

struct Shared<T> {
    slots: Box<[CachePadded<Slot<T>>]>,
    panicked: AtomicBool,
}

impl<T> Shared<T> {
    fn new(n: usize) -> Self {
        let slots =
            (0..n).map(|_| CachePadded::new(Slot::new())).collect::<Vec<_>>().into_boxed_slice();
        Self { slots, panicked: AtomicBool::new(false) }
    }

    /// Writes a value into slot `i`. Must only be called once per index.
    #[inline]
    fn write(&self, i: usize, val: T) {
        let slot = &self.slots[i];
        *slot.value.lock() = Some(val);
        slot.notify.notify_one();
    }

    /// Blocks until slot `i` is ready and takes the value.
    /// Returns `None` if the producer panicked.
    fn take(&self, i: usize) -> Option<T> {
        let slot = &self.slots[i];
        let mut guard = slot.value.lock();
        loop {
            if let Some(val) = guard.take() {
                return Some(val);
            }
            if self.panicked.load(Ordering::Acquire) {
                return None;
            }
            slot.notify.wait(&mut guard);
        }
    }
}

/// Executes a parallel iterator and delivers results to a sequential callback in index order.
///
/// Each slot has its own [`Condvar`], so the consumer blocks precisely on the slot it needs
/// with zero spurious wakeups.
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
                    shared.write(i, item);
                });
            }));
            if let Err(payload) = res {
                shared.panicked.store(true, Ordering::Release);
                // Wake all slots so the consumer doesn't hang. Lock each slot's mutex
                // first to serialize with the consumer's panicked check → wait sequence.
                for slot in &*shared.slots {
                    let _guard = slot.value.lock();
                    slot.notify.notify_one();
                }
                std::panic::resume_unwind(payload);
            }
        });

        // Consumer: sequential, ordered, on the calling thread.
        for i in 0..n {
            let Some(value) = shared.take(i) else {
                return;
            };
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
        let barrier = Barrier::new(2);
        let n = 64usize;
        let input: Vec<usize> = (0..n).collect();
        let mut output = Vec::with_capacity(n);

        input
            .par_iter()
            .map(|&i| {
                if i == 0 || i == n - 1 {
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
        let drops = DROP_COUNT.load(Ordering::Relaxed);
        assert!(drops > 0, "some items should have been dropped");
    }

    #[test]
    fn no_double_drop() {
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
        use std::rc::Rc;
        let counter = Rc::new(std::cell::Cell::new(0u64));
        let input: Vec<u64> = (0..100).collect();
        input.par_iter().map(|&x| x).for_each_ordered(|x| {
            counter.set(counter.get() + x);
        });
        assert_eq!(counter.get(), (0..100u64).sum::<u64>());
    }

    #[test]
    fn panic_does_not_deadlock_consumer() {
        // Regression test: producer panics while consumer is waiting on a condvar.
        // Without the lock-before-notify fix, the consumer could miss the wakeup
        // and deadlock.
        for _ in 0..100 {
            let result = std::panic::catch_unwind(|| {
                let input: Vec<usize> = (0..256).collect();
                input
                    .par_iter()
                    .map(|&i| {
                        if i == 128 {
                            // Yield to increase the chance of the consumer being
                            // between the panicked check and condvar wait.
                            std::thread::yield_now();
                            panic!("intentional");
                        }
                        i
                    })
                    .for_each_ordered(|_| {});
            });
            assert!(result.is_err());
        }
    }

    #[test]
    fn early_panic_at_item_zero() {
        let result = std::panic::catch_unwind(|| {
            let input: Vec<u64> = (0..10).collect();
            input
                .par_iter()
                .map(|&i| {
                    assert!(i != 0, "boom at zero");
                    i
                })
                .for_each_ordered(|_| {});
        });
        assert!(result.is_err());
    }

    #[test]
    fn late_panic_at_last_item() {
        let n = 100usize;
        let result = std::panic::catch_unwind(|| {
            let input: Vec<usize> = (0..n).collect();
            input
                .par_iter()
                .map(|&i| {
                    assert!(i != n - 1, "boom at last");
                    i
                })
                .for_each_ordered(|_| {});
        });
        assert!(result.is_err());
    }

    #[test]
    fn large_items() {
        let n = 500usize;
        let input: Vec<usize> = (0..n).collect();
        let mut output = Vec::with_capacity(n);
        input
            .par_iter()
            .map(|&i| {
                // Return a heap-allocated value to stress drop semantics.
                vec![i; 64]
            })
            .for_each_ordered(|v| output.push(v[0]));
        assert_eq!(output, input);
    }

    #[test]
    fn consumer_slower_than_producer() {
        // Producer is fast, consumer is slow. All items should still arrive in order.
        let n = 64usize;
        let input: Vec<usize> = (0..n).collect();
        let mut output = Vec::with_capacity(n);
        input
            .par_iter()
            .map(|&i| i)
            .for_each_ordered(|x| {
                if x % 8 == 0 {
                    std::thread::yield_now();
                }
                output.push(x);
            });
        assert_eq!(output, input);
    }

    #[test]
    fn concurrent_panic_and_drop_no_leak() {
        // Ensure items produced before a panic are all dropped.
        static DROP_COUNT: AtomicUsize = AtomicUsize::new(0);
        static PRODUCED: AtomicUsize = AtomicUsize::new(0);

        struct Tracked;
        impl Drop for Tracked {
            fn drop(&mut self) {
                DROP_COUNT.fetch_add(1, Ordering::Relaxed);
            }
        }

        DROP_COUNT.store(0, Ordering::Relaxed);
        PRODUCED.store(0, Ordering::Relaxed);

        let barrier = Barrier::new(2);
        let result = std::panic::catch_unwind(|| {
            let input: Vec<usize> = (0..64).collect();
            input
                .par_iter()
                .map(|&i| {
                    if i == 32 {
                        barrier.wait();
                        panic!("intentional");
                    }
                    if i == 0 {
                        barrier.wait();
                    }
                    PRODUCED.fetch_add(1, Ordering::Relaxed);
                    Tracked
                })
                .for_each_ordered(|_| {});
        });

        assert!(result.is_err());
        let produced = PRODUCED.load(Ordering::Relaxed);
        let dropped = DROP_COUNT.load(Ordering::Relaxed);
        assert_eq!(dropped, produced, "all produced items must be dropped");
    }
}
