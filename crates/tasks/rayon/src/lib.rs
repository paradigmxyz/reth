//! Routes bounded parallel work to Rayon or inline execution.
//!
//! [`deterministic`] installs an inline context for each poll, restoring the previous context
//! before returning to the executor. Nested helpers inherit that context. Inline work is atomic
//! to the simulator: jobs must not wait for another simulated actor to make progress.

use rayon::prelude::*;
use std::{
    cell::Cell,
    cmp::Ordering,
    future::Future,
    panic::{catch_unwind, resume_unwind, AssertUnwindSafe},
    pin::Pin,
    sync::Mutex,
    task::{Context, Poll},
};

thread_local! {
    static INLINE: Cell<bool> = const { Cell::new(false) };
}

/// Runs a future with inline parallel operations during each poll, including after migration
/// between executor threads. The context does not leak across suspension or unwinding.
pub fn deterministic<F: Future>(future: F) -> impl Future<Output = F::Output> {
    DeterministicFuture { future: Some(Box::pin(future)) }
}

/// Runs synchronous work with inline parallel operations. Nesting and panic unwinding restore
/// the caller's context. This does not intercept raw Rayon or OS-thread calls.
pub fn inline<F: FnOnce() -> R, R>(f: F) -> R {
    let _guard = InlineGuard(INLINE.replace(true));
    f()
}

/// Whether this call is inside an inline execution context.
pub fn is_inline() -> bool {
    INLINE.get()
}

/// Runs two independent computations. Both finish before a panic is propagated.
pub fn join<A, B, RA, RB>(a: A, b: B) -> (RA, RB)
where
    A: FnOnce() -> RA + Send,
    B: FnOnce() -> RB + Send,
    RA: Send,
    RB: Send,
{
    if !is_inline() {
        return rayon::join(a, b)
    }
    let a = catch_unwind(AssertUnwindSafe(a));
    let b = catch_unwind(AssertUnwindSafe(b));
    match (a, b) {
        (Ok(a), Ok(b)) => (a, b),
        (Err(panic), _) | (_, Err(panic)) => resume_unwind(panic),
    }
}

/// Submits a bounded independent job to the global pool, or completes it inline.
///
/// In inline mode this returns only after the job completes. The job must not block on its
/// caller or on a later submission; use asynchronous runtime tasks for those dependencies.
pub fn spawn<F: FnOnce() + Send + 'static>(job: F) {
    if is_inline() {
        job()
    } else {
        rayon::spawn(job)
    }
}

/// Dispatches a bounded independent job using a caller-supplied pool, or completes it inline.
pub fn spawn_with<F, S>(job: F, submit: S)
where
    F: FnOnce() + Send + 'static,
    S: FnOnce(F),
{
    if is_inline() {
        job()
    } else {
        submit(job)
    }
}

/// Executes a bounded computation using a caller-supplied pool, or inline.
pub fn run_with<F, S, R>(job: F, submit: S) -> R
where
    F: FnOnce() -> R + Send,
    S: FnOnce(F) -> R,
    R: Send,
{
    if is_inline() {
        job()
    } else {
        submit(job)
    }
}

/// Collects an opaque parallel iterator. Prefer [`map_collect`] when ordinary iteration is
/// available: arbitrary Rayon iterators cannot be converted back to sequential iterators.
///
/// Inline mode synchronously uses one private Rayon worker. No other simulated actor can run
/// until it returns. The worker inherits inline routing for nested application operations.
pub fn collect<I: IntoParallelIterator>(input: I) -> Vec<I::Item> {
    let iter = input.into_par_iter();
    if !is_inline() {
        return iter.collect()
    }
    thread_local! {
        static SERIAL: rayon::ThreadPool = rayon::ThreadPoolBuilder::new()
            .num_threads(1).thread_name(|_| "dst-rayon-compat".into()).build()
            .expect("create deterministic Rayon compatibility worker");
        static IN_SERIAL: Cell<bool> = const { Cell::new(false) };
    }
    if IN_SERIAL.get() {
        return iter.collect()
    }
    SERIAL.with(|pool| {
        pool.install(|| {
            inline(|| {
                struct Reset;
                impl Drop for Reset {
                    fn drop(&mut self) {
                        IN_SERIAL.set(false);
                    }
                }
                IN_SERIAL.set(true);
                let _reset = Reset;
                iter.collect()
            })
        })
    })
}

/// Visits an indexed parallel iterator sequentially on the caller using its producer.
/// This preserves lazy production and permits a non-`Send` consumer.
pub fn for_each_indexed<I: IndexedParallelIterator, F: FnMut(I::Item)>(iter: I, f: F) {
    use rayon::iter::plumbing::{Producer, ProducerCallback};
    struct Visit<F>(F);
    impl<T, F: FnMut(T)> ProducerCallback<T> for Visit<F> {
        type Output = ();
        fn callback<P: Producer<Item = T>>(mut self, producer: P) {
            producer.into_iter().for_each(&mut self.0);
        }
    }
    iter.with_producer(Visit(f));
}

/// Filters and maps in source order in inline mode.
pub fn filter_map_collect<I, F, R>(input: I, f: F) -> Vec<R>
where
    I: IntoIterator + IntoParallelIterator<Item = <I as IntoIterator>::Item>,
    <I as IntoIterator>::Item: Send,
    F: Fn(<I as IntoIterator>::Item) -> Option<R> + Send + Sync,
    R: Send,
{
    if is_inline() {
        input.into_iter().filter_map(f).collect()
    } else {
        input.into_par_iter().filter_map(f).collect()
    }
}

/// Maps and collects using the source's ordinary iteration order in inline mode.
pub fn map_collect<I, F, R>(input: I, f: F) -> Vec<R>
where
    I: IntoIterator + IntoParallelIterator<Item = <I as IntoIterator>::Item>,
    <I as IntoIterator>::Item: Send,
    F: Fn(<I as IntoIterator>::Item) -> R + Send + Sync,
    R: Send,
{
    if is_inline() {
        input.into_iter().map(f).collect()
    } else {
        input.into_par_iter().map(f).collect()
    }
}

/// Fallible mapping and collection. Inline mode stops at the first error in iteration order.
pub fn try_map_collect<I, F, R, E>(input: I, f: F) -> Result<Vec<R>, E>
where
    I: IntoIterator + IntoParallelIterator<Item = <I as IntoIterator>::Item>,
    <I as IntoIterator>::Item: Send,
    F: Fn(<I as IntoIterator>::Item) -> Result<R, E> + Send + Sync,
    R: Send,
    E: Send,
{
    if is_inline() {
        input.into_iter().map(f).collect()
    } else {
        input.into_par_iter().map(f).collect()
    }
}

/// Sorts stably by key, preserving equal-key order in both modes.
pub fn sort_by_key<T, K, F>(slice: &mut [T], f: F)
where
    T: Send,
    K: Ord,
    F: Fn(&T) -> K + Sync,
{
    if is_inline() {
        slice.sort_by_key(f)
    } else {
        slice.par_sort_by_key(f)
    }
}

/// Sorts by key without promising equal-key order. Callers that observe ties must provide a
/// total key or use [`sort_by_key`].
pub fn sort_unstable_by_key<T, K, F>(slice: &mut [T], f: F)
where
    T: Send,
    K: Ord,
    F: Fn(&T) -> K + Sync,
{
    if is_inline() {
        slice.sort_unstable_by_key(f)
    } else {
        slice.par_sort_unstable_by_key(f)
    }
}

/// Sorts by comparison without promising equal-key order.
pub fn sort_unstable_by<T, F>(slice: &mut [T], f: F)
where
    T: Send,
    F: Fn(&T, &T) -> Ordering + Sync,
{
    if is_inline() {
        slice.sort_unstable_by(f)
    } else {
        slice.par_sort_unstable_by(f)
    }
}

/// Runs bounded scoped jobs on the supplied pool, or inline without entering the pool.
/// Child jobs may borrow the caller's data. They must be independent: an inline child cannot
/// wait for a later child. All submitted children finish before propagating a child panic.
pub fn in_place_scope<'scope, F, R>(pool: &rayon::ThreadPool, f: F) -> R
where
    F: for<'env> FnOnce(&Scope<'scope, 'env>) -> R,
{
    if is_inline() {
        let scope = Scope { native: None, panic: Mutex::new(None) };
        let result = f(&scope);
        if let Some(panic) = scope.panic.into_inner().expect("scope panic lock poisoned") {
            resume_unwind(panic);
        }
        result
    } else {
        pool.in_place_scope(|native| f(&Scope { native: Some(native), panic: Mutex::new(None) }))
    }
}

/// A borrowed scope for independent jobs dispatched by [`in_place_scope`].
pub struct Scope<'scope, 'env> {
    native: Option<&'env rayon::Scope<'scope>>,
    panic: Mutex<Option<Box<dyn std::any::Any + Send>>>,
}

impl<'scope> Scope<'scope, '_> {
    /// Runs a child inline or submits it to the native scope.
    pub fn spawn<F>(&self, f: F)
    where
        F: FnOnce() + Send + 'scope,
    {
        if let Some(native) = self.native {
            native.spawn(move |_| f());
        } else if let Err(panic) = catch_unwind(AssertUnwindSafe(f)) {
            self.panic.lock().expect("scope panic lock poisoned").get_or_insert(panic);
        }
    }
}

impl std::fmt::Debug for Scope<'_, '_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Scope").field("native", &self.native.is_some()).finish_non_exhaustive()
    }
}

struct InlineGuard(bool);

impl Drop for InlineGuard {
    fn drop(&mut self) {
        INLINE.set(self.0);
    }
}

struct DeterministicFuture<F: Future> {
    future: Option<Pin<Box<F>>>,
}

impl<F: Future> Future for DeterministicFuture<F> {
    type Output = F::Output;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        inline(|| self.future.as_mut().expect("future exists until drop").as_mut().poll(cx))
    }
}

impl<F: Future> Drop for DeterministicFuture<F> {
    fn drop(&mut self) {
        inline(|| drop(self.future.take()));
    }
}

#[cfg(test)]
mod tests;
