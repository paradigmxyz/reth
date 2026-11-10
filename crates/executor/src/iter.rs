//! Rayon iterator APIs, evaluated serially under the deterministic scheduler.

#[cfg(feature = "fuzz")]
pub use deterministic::*;
#[cfg(not(feature = "fuzz"))]
pub use rayon::iter::*;
#[cfg(not(feature = "fuzz"))]
pub use rayon::slice::ParallelSliceMut;

#[cfg(feature = "fuzz")]
mod deterministic {
    use std::{
        collections::{BTreeMap, BTreeSet, HashMap, HashSet},
        iter::{Chain, Enumerate, Filter, FilterMap, OnceWith, Skip, Take, Zip},
    };

    /// A lazy serial iterator with the compute facade's parallel-iterator API.
    #[derive(Debug, Clone)]
    pub struct Serial<TIterator>(TIterator);

    /// Converts a collection or serial iterator into a deterministic compute iterator.
    pub trait IntoParallelIterator {
        /// The values yielded by this iterator.
        type Item: Send;
        /// The resulting deterministic iterator.
        type Iter: ParallelIterator<Item = Self::Item>;
        /// Converts this value without executing its lazy operations.
        fn into_par_iter(self) -> Self::Iter;
    }
    impl<T: ParallelIterator> IntoParallelIterator for T {
        type Item = T::Item;
        type Iter = Self;
        fn into_par_iter(self) -> Self {
            self
        }
    }

    macro_rules! into_parallel {
        ([$($generics:tt)*] $source:ty) => {
            impl<$($generics)*> IntoParallelIterator for $source
            where Self: IntoIterator, <Self as IntoIterator>::IntoIter: Send,
                  <Self as IntoIterator>::Item: Send {
                type Item = <Self as IntoIterator>::Item;
                type Iter = Serial<<Self as IntoIterator>::IntoIter>;
                fn into_par_iter(self) -> Self::Iter { Serial(self.into_iter()) }
            }
        };
    }
    into_parallel!([T] Vec<T>);
    into_parallel!([T] Option<T>);
    into_parallel!([T, E] Result<T, E>);
    into_parallel!([T] std::ops::Range<T>);
    into_parallel!([T] std::ops::RangeInclusive<T>);
    into_parallel!([T, const N: usize] [T; N]);
    into_parallel!(['a, T] &'a [T]);
    into_parallel!(['a, T] &'a mut [T]);
    into_parallel!(['a, T, const N: usize] &'a [T; N]);
    into_parallel!(['a, T, const N: usize] &'a mut [T; N]);
    into_parallel!(['a, T] &'a Vec<T>);
    into_parallel!(['a, T] &'a mut Vec<T>);
    into_parallel!([K, V] BTreeMap<K, V>);
    into_parallel!([T] BTreeSet<T>);
    into_parallel!([K, V, S] HashMap<K, V, S>);
    into_parallel!([T, S] HashSet<T, S>);
    into_parallel!([T] std::collections::VecDeque<T>);
    into_parallel!([T] std::collections::LinkedList<T>);
    into_parallel!([T] std::collections::BinaryHeap<T>);

    /// A lazy map adapter retaining the source compute iterator's indexed guarantees.
    pub type Map<TIterator, TMap> =
        Serial<std::iter::Map<<TIterator as ParallelIterator>::Inner, TMap>>;

    /// Borrows a collection as a deterministic compute iterator.
    pub trait IntoParallelRefIterator<'a> {
        /// The borrowed item type.
        type Item: Send + 'a;
        /// The resulting iterator over shared references.
        type Iter: ParallelIterator<Item = Self::Item>;
        /// Iterates over shared references in collection order.
        fn par_iter(&'a self) -> Self::Iter;
    }
    impl<'a, T: ?Sized + 'a> IntoParallelRefIterator<'a> for T
    where
        &'a T: IntoIterator,
        <&'a T as IntoIterator>::IntoIter: Send,
        <&'a T as IntoIterator>::Item: Send + 'a,
    {
        type Item = <&'a T as IntoIterator>::Item;
        type Iter = Serial<<&'a T as IntoIterator>::IntoIter>;
        fn par_iter(&'a self) -> Self::Iter {
            Serial(self.into_iter())
        }
    }

    /// Mutably borrows a collection as a deterministic compute iterator.
    pub trait IntoParallelRefMutIterator<'a> {
        /// The mutably borrowed item type.
        type Item: Send + 'a;
        /// The resulting iterator over mutable references.
        type Iter: ParallelIterator<Item = Self::Item>;
        /// Iterates over mutable references in collection order.
        fn par_iter_mut(&'a mut self) -> Self::Iter;
    }
    impl<'a, T: ?Sized + 'a> IntoParallelRefMutIterator<'a> for T
    where
        &'a mut T: IntoIterator,
        <&'a mut T as IntoIterator>::IntoIter: Send,
        <&'a mut T as IntoIterator>::Item: Send + 'a,
    {
        type Item = <&'a mut T as IntoIterator>::Item;
        type Iter = Serial<<&'a mut T as IntoIterator>::IntoIter>;
        fn par_iter_mut(&'a mut self) -> Self::Iter {
            Serial(self.into_iter())
        }
    }

    /// Adapts a serial iterator without introducing native worker threads.
    pub trait ParallelBridge: Iterator + Sized + Send
    where
        Self::Item: Send,
    {
        /// Preserves the source iterator's order and laziness.
        fn par_bridge(self) -> Serial<Self> {
            Serial(self)
        }
    }
    impl<T: Iterator + Send> ParallelBridge for T where T::Item: Send {}

    /// Builds a collection from deterministic compute results.
    pub trait FromParallelIterator<TItem>: Sized
    where
        TItem: Send,
    {
        /// Collects all results in deterministic iterator order.
        fn from_par_iter<T: IntoParallelIterator<Item = TItem>>(iter: T) -> Self;
    }
    macro_rules! from_parallel {
        ([$($generics:tt)*] $item:ty => $collection:ty) => {
            impl<$($generics)*> FromParallelIterator<$item> for $collection
            where
                Self: FromIterator<$item>,
                $item: Send,
            {
                fn from_par_iter<T: IntoParallelIterator<Item = $item>>(iter: T) -> Self {
                    iter.into_par_iter().into_serial().collect()
                }
            }
        };
    }
    from_parallel!([TItem] TItem => Vec<TItem>);
    from_parallel!([TItem] TItem => BTreeSet<TItem>);
    from_parallel!([TKey, TValue] (TKey, TValue) => BTreeMap<TKey, TValue>);
    from_parallel!([TItem, THasher] TItem => HashSet<TItem, THasher>);
    from_parallel!([TKey, TValue, THasher] (TKey, TValue) => HashMap<TKey, TValue, THasher>);

    impl<TItem: Send, TError: Send, TCollection: FromParallelIterator<TItem>>
        FromParallelIterator<Result<TItem, TError>> for Result<TCollection, TError>
    {
        fn from_par_iter<T: IntoParallelIterator<Item = Result<TItem, TError>>>(iter: T) -> Self {
            let items: Result<Vec<_>, _> = iter.into_par_iter().into_serial().collect();
            items.map(TCollection::from_par_iter)
        }
    }

    /// A lazy compute iterator; deterministic builds execute pure compute on the caller.
    pub trait ParallelIterator: Sized + Send {
        /// The yielded value type.
        type Item: Send;
        /// The underlying lazy serial iterator.
        type Inner: Iterator<Item = Self::Item> + Send;
        /// Returns the serial iterator used by deterministic adapters.
        fn into_serial(self) -> Self::Inner;
        /// Lazily transforms each item.
        fn map<TOutput: Send, TMap: Fn(Self::Item) -> TOutput + Send + Sync>(
            self,
            f: TMap,
        ) -> Map<Self, TMap> {
            Serial(self.into_serial().map(f))
        }
        /// Retains items satisfying the predicate.
        fn filter<TFilter: Fn(&Self::Item) -> bool + Send + Sync>(
            self,
            f: TFilter,
        ) -> Serial<Filter<Self::Inner, TFilter>> {
            Serial(self.into_serial().filter(f))
        }
        /// Transforms and retains items with a nonempty result.
        fn filter_map<TOutput: Send, TMap: Fn(Self::Item) -> Option<TOutput> + Send + Sync>(
            self,
            f: TMap,
        ) -> Serial<FilterMap<Self::Inner, TMap>> {
            Serial(self.into_serial().filter_map(f))
        }
        /// Expands each item into another deterministic iterator.
        fn flat_map<
            TOutput: IntoParallelIterator,
            TMap: Fn(Self::Item) -> TOutput + Send + Sync,
        >(
            self,
            f: TMap,
        ) -> impl ParallelIterator<Item = TOutput::Item> {
            Serial(self.into_serial().flat_map(move |item| f(item).into_par_iter().into_serial()))
        }
        /// Runs a callback for every item in source order.
        fn for_each(self, f: impl Fn(Self::Item) + Send + Sync) {
            self.into_serial().for_each(f);
        }
        /// Runs fallible callbacks until the first error, preserving that error.
        fn try_for_each<TError: Send>(
            self,
            f: impl Fn(Self::Item) -> Result<(), TError> + Send + Sync,
        ) -> Result<(), TError> {
            self.into_serial().try_for_each(f)
        }
        /// Collects results using the collection's parallel-construction implementation.
        fn collect<T: FromParallelIterator<Self::Item>>(self) -> T {
            T::from_par_iter(Serial(self.into_serial()))
        }
        /// Folds this serial partition into one value, exposed as a one-item compute iterator.
        fn fold<TAccumulator: Send>(
            self,
            identity: impl Fn() -> TAccumulator + Send + Sync,
            fold: impl Fn(TAccumulator, Self::Item) -> TAccumulator + Send + Sync,
        ) -> Serial<OnceWith<impl FnOnce() -> TAccumulator + Send>> {
            Serial(std::iter::once_with(move || self.into_serial().fold(identity(), fold)))
        }
        /// Combines all items starting with the identity value.
        fn reduce(
            self,
            identity: impl Fn() -> Self::Item + Send + Sync,
            reduce: impl Fn(Self::Item, Self::Item) -> Self::Item + Send + Sync,
        ) -> Self::Item {
            self.into_serial().fold(identity(), reduce)
        }
        /// Combines all items, returning `None` for an empty iterator.
        fn reduce_with(
            self,
            reduce: impl Fn(Self::Item, Self::Item) -> Self::Item + Send + Sync,
        ) -> Option<Self::Item> {
            self.into_serial().reduce(reduce)
        }
        /// Sums all yielded values.
        fn sum<T: std::iter::Sum<Self::Item> + Send>(self) -> T {
            self.into_serial().sum()
        }
        /// Counts all yielded values.
        fn count(self) -> usize {
            self.into_serial().count()
        }
        /// Returns the first matching item in deterministic source order, or `None`.
        fn find_any(
            self,
            predicate: impl Fn(&Self::Item) -> bool + Send + Sync,
        ) -> Option<Self::Item> {
            self.into_serial().find(predicate)
        }
        /// Returns whether any value satisfies the predicate.
        fn any(self, f: impl Fn(Self::Item) -> bool + Send + Sync) -> bool {
            self.into_serial().any(f)
        }
        /// Returns whether all values satisfy the predicate.
        fn all(self, f: impl Fn(Self::Item) -> bool + Send + Sync) -> bool {
            self.into_serial().all(f)
        }
        /// Concatenates two compute iterators in source order.
        fn chain<T: IntoParallelIterator<Item = Self::Item>>(
            self,
            other: T,
        ) -> Serial<Chain<Self::Inner, <T::Iter as ParallelIterator>::Inner>> {
            Serial(self.into_serial().chain(other.into_par_iter().into_serial()))
        }
    }
    impl<TIterator: Iterator + Send> ParallelIterator for Serial<TIterator>
    where
        TIterator::Item: Send,
    {
        type Item = TIterator::Item;
        type Inner = TIterator;
        fn into_serial(self) -> TIterator {
            self.0
        }
    }

    impl<TLeft, TRight> ParallelIterator for either::Either<TLeft, TRight>
    where
        TLeft: ParallelIterator,
        TRight: ParallelIterator<Item = TLeft::Item>,
    {
        type Item = TLeft::Item;
        type Inner = either::Either<TLeft::Inner, TRight::Inner>;
        fn into_serial(self) -> Self::Inner {
            match self {
                Self::Left(iter) => either::Either::Left(iter.into_serial()),
                Self::Right(iter) => either::Either::Right(iter.into_serial()),
            }
        }
    }
    impl<TLeft, TRight> IndexedParallelIterator for either::Either<TLeft, TRight>
    where
        TLeft: IndexedParallelIterator,
        TRight: IndexedParallelIterator<Item = TLeft::Item>,
    {
        fn len(&self) -> usize {
            match self {
                Self::Left(iter) => iter.len(),
                Self::Right(iter) => iter.len(),
            }
        }
    }

    /// In-place slice sorting without native workers in deterministic builds.
    pub trait ParallelSliceMut<T: Send> {
        /// Stably sorts items by their derived keys.
        fn par_sort_by_key<TKey: Ord, TKeyFn: Fn(&T) -> TKey + Sync>(&mut self, key: TKeyFn);
        /// Sorts items by their derived keys without preserving equal-key order.
        fn par_sort_unstable_by_key<TKey: Ord, TKeyFn: Fn(&T) -> TKey + Sync>(
            &mut self,
            key: TKeyFn,
        );
        /// Sorts items by a comparator without preserving equal-item order.
        fn par_sort_unstable_by<TCompare: Fn(&T, &T) -> std::cmp::Ordering + Sync>(
            &mut self,
            compare: TCompare,
        );
    }
    impl<T: Send> ParallelSliceMut<T> for [T] {
        fn par_sort_by_key<TKey: Ord, TKeyFn: Fn(&T) -> TKey + Sync>(&mut self, key: TKeyFn) {
            self.sort_by_key(key);
        }
        fn par_sort_unstable_by_key<TKey: Ord, TKeyFn: Fn(&T) -> TKey + Sync>(
            &mut self,
            key: TKeyFn,
        ) {
            self.sort_unstable_by_key(key);
        }
        fn par_sort_unstable_by<TCompare: Fn(&T, &T) -> std::cmp::Ordering + Sync>(
            &mut self,
            compare: TCompare,
        ) {
            self.sort_unstable_by(compare);
        }
    }

    /// A compute iterator with an exact length and stable source indices.
    pub trait IndexedParallelIterator: ParallelIterator<Inner: ExactSizeIterator> {
        /// Returns the number of items without evaluating lazy mappings.
        fn len(&self) -> usize;
        /// Returns whether there are no items.
        fn is_empty(&self) -> bool {
            self.len() == 0
        }
        /// Attaches each item's zero-based source index.
        fn enumerate(self) -> Serial<Enumerate<Self::Inner>> {
            Serial(self.into_serial().enumerate())
        }
        /// Preserves index order; no partition scheduling is needed for serial compute.
        fn by_exponential_blocks(self) -> Self {
            self
        }
        /// Takes at most the first `count` items.
        fn take(self, count: usize) -> Serial<Take<Self::Inner>> {
            Serial(self.into_serial().take(count))
        }
        /// Skips the first `count` items.
        fn skip(self, count: usize) -> Serial<Skip<Self::Inner>> {
            Serial(self.into_serial().skip(count))
        }
        /// Pairs values at matching indices, stopping at the shorter iterator.
        fn zip<T: IntoParallelIterator>(
            self,
            other: T,
        ) -> Serial<Zip<Self::Inner, <T::Iter as ParallelIterator>::Inner>>
        where
            T::Iter: IndexedParallelIterator,
        {
            Serial(self.into_serial().zip(other.into_par_iter().into_serial()))
        }
    }
    impl<TIterator: ExactSizeIterator + Send> IndexedParallelIterator for Serial<TIterator>
    where
        TIterator::Item: Send,
    {
        fn len(&self) -> usize {
            self.0.len()
        }
    }
}
