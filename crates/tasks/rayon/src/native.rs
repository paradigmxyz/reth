//! Direct Rayon dispatch used by normal builds.

use rayon::prelude::*;
use std::cmp::Ordering;

/// Runs synchronous work immediately.
#[inline]
pub fn inline<F: FnOnce() -> R, R>(f: F) -> R {
    f()
}

/// Returns false in builds without deterministic scheduling.
#[inline]
pub const fn is_inline() -> bool {
    false
}

/// Runs two independent computations on Rayon.
pub fn join<A, B, RA, RB>(a: A, b: B) -> (RA, RB)
where
    A: FnOnce() -> RA + Send,
    B: FnOnce() -> RB + Send,
    RA: Send,
    RB: Send,
{
    rayon::join(a, b)
}

/// Submits an independent job to the global Rayon pool.
pub fn spawn<F: FnOnce() + Send + 'static>(job: F) {
    rayon::spawn(job)
}

/// Dispatches a job using a caller-supplied submission function.
pub fn spawn_with<F, S>(job: F, submit: S)
where
    F: FnOnce() + Send + 'static,
    S: FnOnce(F),
{
    submit(job)
}

/// Executes a computation using a caller-supplied pool.
pub fn run_with<F, S, R>(job: F, submit: S) -> R
where
    F: FnOnce() -> R + Send,
    S: FnOnce(F) -> R,
    R: Send,
{
    submit(job)
}

/// Collects a parallel iterator.
pub fn collect<I: IntoParallelIterator>(input: I) -> Vec<I::Item> {
    input.into_par_iter().collect()
}

/// Visits an indexed iterator through its producer without constraining the consumer to `Send`.
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

/// Filters, maps, and collects in parallel.
pub fn filter_map_collect<I, F, R>(input: I, f: F) -> Vec<R>
where
    I: IntoIterator + IntoParallelIterator<Item = <I as IntoIterator>::Item>,
    <I as IntoIterator>::Item: Send,
    F: Fn(<I as IntoIterator>::Item) -> Option<R> + Send + Sync,
    R: Send,
{
    input.into_par_iter().filter_map(f).collect()
}

/// Maps and collects in parallel.
pub fn map_collect<I, F, R>(input: I, f: F) -> Vec<R>
where
    I: IntoIterator + IntoParallelIterator<Item = <I as IntoIterator>::Item>,
    <I as IntoIterator>::Item: Send,
    F: Fn(<I as IntoIterator>::Item) -> R + Send + Sync,
    R: Send,
{
    input.into_par_iter().map(f).collect()
}

/// Fallibly maps and collects in parallel.
pub fn try_map_collect<I, F, R, E>(input: I, f: F) -> Result<Vec<R>, E>
where
    I: IntoIterator + IntoParallelIterator<Item = <I as IntoIterator>::Item>,
    <I as IntoIterator>::Item: Send,
    F: Fn(<I as IntoIterator>::Item) -> Result<R, E> + Send + Sync,
    R: Send,
    E: Send,
{
    input.into_par_iter().map(f).collect()
}

/// Sorts in parallel by key.
pub fn sort_by_key<T, K, F>(slice: &mut [T], f: F)
where
    T: Send,
    K: Ord,
    F: Fn(&T) -> K + Sync,
{
    slice.par_sort_by_key(f)
}

/// Sorts in parallel by key without preserving equal-key order.
pub fn sort_unstable_by_key<T, K, F>(slice: &mut [T], f: F)
where
    T: Send,
    K: Ord,
    F: Fn(&T) -> K + Sync,
{
    slice.par_sort_unstable_by_key(f)
}

/// Sorts in parallel using a comparison function.
pub fn sort_unstable_by<T, F>(slice: &mut [T], f: F)
where
    T: Send,
    F: Fn(&T, &T) -> Ordering + Sync,
{
    slice.par_sort_unstable_by(f)
}

/// Runs scoped jobs on a caller-supplied Rayon pool.
pub fn in_place_scope<'scope, F, R>(pool: &rayon::ThreadPool, f: F) -> R
where
    F: for<'env> FnOnce(&Scope<'scope, 'env>) -> R,
{
    pool.in_place_scope(|native| f(&Scope { native }))
}

/// A borrowed native Rayon scope.
pub struct Scope<'scope, 'env> {
    native: &'env rayon::Scope<'scope>,
}

impl<'scope> Scope<'scope, '_> {
    /// Submits a child to the native scope.
    pub fn spawn<F>(&self, f: F)
    where
        F: FnOnce() + Send + 'scope,
    {
        self.native.spawn(move |_| f());
    }
}

impl std::fmt::Debug for Scope<'_, '_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Scope").finish_non_exhaustive()
    }
}
