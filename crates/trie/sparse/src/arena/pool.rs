//! A fixed pool of worker threads for the arena trie's parallel phases.
//!
//! The pool is started once per process, with one worker per available core besides the calling
//! thread, and runs one job at a time: a job is a number of items that the caller and the workers
//! claim from a shared counter until none are left. Once the caller has no more items to claim it
//! closes the job, and it returns once every worker that joined has left: a worker that wakes
//! after the close never touches the job. So nothing outlives a call, a small job does not wait
//! for parked workers to wake, and a job needs no allocation: its closure stays on the caller's
//! stack. Idle workers spin briefly, so that back-to-back phases (updating subtries,
//! then hashing them) do not pay for a wake-up, and then park.
//!
//! A call made while another job runs, such as one storage trie's while another storage trie holds
//! the pool, or a nested one from inside a job, runs its items on rayon's global pool instead of
//! waiting.

use core::{
    cell::UnsafeCell,
    hint,
    sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
};
use rayon::iter::{IntoParallelIterator, ParallelIterator};
use std::{
    any::Any,
    boxed::Box,
    panic::{self, AssertUnwindSafe},
    sync::{Mutex, OnceLock},
    thread::{self, Thread},
    vec::Vec,
};

/// Spin iterations a waiting thread makes before parking.
const SPIN_LIMIT: u32 = 1 << 10;

/// Bits of [`Shared::state`] counting the workers inside the current job.
const JOINED_MASK: u64 = (1 << 32) - 1;
/// Bit of [`Shared::state`] set while workers may join the current job.
const OPEN: u64 = 1 << 32;
/// Shift of the job generation in [`Shared::state`].
const GENERATION_SHIFT: u32 = 33;

/// Runs `f` on every item of `items`, spread over the calling thread and the pool's workers.
pub(super) fn for_each_mut<T: Send>(items: &mut [T], f: impl Fn(&mut T) + Sync) {
    if items.len() <= 1 {
        items.iter_mut().for_each(f);
        return;
    }

    let base = SendPtr(items.as_mut_ptr());
    // SAFETY: `run` hands out every index below `items.len()` exactly once, so each item is
    // borrowed mutably by one call at a time, and `run` returns only after every call ended.
    run(items.len(), &|i| f(unsafe { &mut *base.get().add(i) }));
}

/// Calls `f` once for every index in `0..len`, spread over the calling thread and the pool's
/// workers, and returns once all calls have ended.
fn run(len: usize, f: &(dyn Fn(usize) + Sync)) {
    let workers = workers();
    if workers.is_empty() {
        (0..len).for_each(f);
        return;
    }
    let shared = &SHARED;
    if shared.busy.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() {
        (0..len).into_par_iter().for_each(f);
        return;
    }

    // SAFETY: `busy` is held and the previous job is closed with no worker inside, so no
    // worker reads the job until it is opened below. The lifetime is erased because workers use
    // the job only while inside it, and this function returns only after closing it and every
    // worker has left.
    unsafe {
        *shared.job.get() = Some(Job {
            f: core::mem::transmute::<&(dyn Fn(usize) + Sync), &'static (dyn Fn(usize) + Sync)>(f),
            len,
            caller: thread::current(),
        });
    }
    shared.next.store(0, Ordering::Relaxed);
    let generation = (shared.state.load(Ordering::Relaxed) >> GENERATION_SHIFT) + 1;
    shared.state.store((generation << GENERATION_SHIFT) | OPEN, Ordering::Release);
    // The caller claims items too, so waking more than `len - 1` workers only adds wake-ups.
    for worker in workers.iter().take(len - 1) {
        worker.unpark();
    }

    // A panic on this thread must not return before the workers left the job, which borrows
    // this stack frame.
    let own_panic = panic::catch_unwind(AssertUnwindSafe(|| shared.claim_all(f, len))).err();
    if own_panic.is_some() {
        shared.next.store(len, Ordering::Relaxed);
    }

    shared.state.fetch_and(!OPEN, Ordering::AcqRel);
    let mut spins = 0;
    while shared.state.load(Ordering::Acquire) & JOINED_MASK != 0 {
        if spins < SPIN_LIMIT {
            spins += 1;
            hint::spin_loop();
        } else {
            thread::park();
        }
    }

    // SAFETY: the job is closed and every worker that joined it has left.
    unsafe { *shared.job.get() = None };
    // Always empty the slot, so a worker's panic never leaks into a later job.
    let worker_panic = shared.panic.lock().unwrap_or_else(|e| e.into_inner()).take();
    let panic = own_panic.or(worker_panic);
    shared.busy.store(false, Ordering::Release);
    if let Some(payload) = panic {
        panic::resume_unwind(payload);
    }
}

/// State shared by the caller and the pool's workers.
static SHARED: Shared = Shared {
    busy: AtomicBool::new(false),
    state: AtomicU64::new(0),
    job: UnsafeCell::new(None),
    next: AtomicUsize::new(0),
    panic: Mutex::new(None),
};

/// Returns the pool's workers, starting them on first use.
fn workers() -> &'static [Thread] {
    static WORKERS: OnceLock<Vec<Thread>> = OnceLock::new();
    WORKERS.get_or_init(|| {
        // Size the pool like rayon's global pool, which reth limits to its configured cores.
        let threads = rayon::current_num_threads();
        (1..threads)
            .filter_map(|i| {
                thread::Builder::new()
                    .name(std::format!("trie-arena-{i}"))
                    .spawn(|| SHARED.work())
                    .ok()
                    .map(|handle| handle.thread().clone())
            })
            .collect()
    })
}

/// The job currently published to the workers.
struct Job {
    f: &'static (dyn Fn(usize) + Sync),
    len: usize,
    caller: Thread,
}

/// State shared by the caller and the workers.
struct Shared {
    /// Held by the caller whose job is running.
    busy: AtomicBool,
    /// The job generation, whether the job is open, and how many workers are inside it.
    state: AtomicU64,
    /// The published job, written only while no worker is inside a job.
    job: UnsafeCell<Option<Job>>,
    /// The next unclaimed index of the job.
    next: AtomicUsize,
    /// The first panic of a call made by a worker, resumed by the caller.
    panic: Mutex<Option<Box<dyn Any + Send>>>,
}

// SAFETY: `job` is written only by the caller holding `busy` while no worker is inside a job,
// and read by workers only between joining an open job and leaving it.
unsafe impl Sync for Shared {}

impl Shared {
    /// A worker's loop: waits for each new job, joins it while it is open, claims its items,
    /// and leaves it.
    fn work(&self) {
        // Workers start at generation zero, before any job: a worker that starts after the
        // first job was published must still join it.
        let mut seen = 0;
        loop {
            let mut spins = 0;
            let mut state = self.state.load(Ordering::Acquire);
            while state >> GENERATION_SHIFT == seen {
                if spins < SPIN_LIMIT {
                    spins += 1;
                    hint::spin_loop();
                } else {
                    thread::park();
                }
                state = self.state.load(Ordering::Acquire);
            }
            seen = state >> GENERATION_SHIFT;

            // Join the job unless it was closed, or replaced by a newer one, in the meantime.
            let joined = loop {
                if state >> GENERATION_SHIFT != seen || state & OPEN == 0 {
                    break false
                }
                match self.state.compare_exchange_weak(
                    state,
                    state + 1,
                    Ordering::Acquire,
                    Ordering::Acquire,
                ) {
                    Ok(_) => break true,
                    Err(current) => state = current,
                }
            };
            if !joined {
                continue
            }

            // SAFETY: the job was published before it was opened, and stays until this worker
            // leaves it.
            let job = unsafe { (*self.job.get()).as_ref().expect("published job") };
            let (f, len, caller) = (job.f, job.len, job.caller.clone());
            if let Err(payload) = panic::catch_unwind(AssertUnwindSafe(|| self.claim_all(f, len))) {
                // Stop the others claiming more items, and keep the first panic.
                self.next.store(len, Ordering::Relaxed);
                self.panic.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert(payload);
            }
            let left = self.state.fetch_sub(1, Ordering::AcqRel) - 1;
            if left & JOINED_MASK == 0 && left & OPEN == 0 {
                caller.unpark();
            }
        }
    }

    /// Claims and runs items of the current job until none are left.
    fn claim_all(&self, f: &(dyn Fn(usize) + Sync), len: usize) {
        loop {
            let i = self.next.fetch_add(1, Ordering::Relaxed);
            if i >= len {
                break;
            }
            f(i);
        }
    }
}

/// A raw pointer that may cross threads, for handing out disjoint items of a slice.
struct SendPtr<T>(*mut T);

impl<T> SendPtr<T> {
    /// Returns the pointer. A method rather than a field access, so closures capture the
    /// whole [`SendPtr`] and with it its `Send` and `Sync` implementations.
    const fn get(&self) -> *mut T {
        self.0
    }
}

// SAFETY: the pointer is only used to reach disjoint items, see `for_each_mut`.
unsafe impl<T: Send> Send for SendPtr<T> {}
// SAFETY: as above.
unsafe impl<T: Send> Sync for SendPtr<T> {}
