//! Rayon iterator APIs, evaluated serially under the deterministic scheduler.
//!
//! With `fuzz`, adapter construction remains lazy and terminal operations drive
//! Rayon's producers and consumers on the caller. Serial partitions do not reproduce
//! worker schedules, parallel callback ordering, cancellation races, or partition
//! counts. Compute callbacks must not depend on concurrent execution.

#[cfg(feature = "fuzz")]
pub use deterministic::*;
#[cfg(not(feature = "fuzz"))]
pub use rayon::iter::*;
#[cfg(not(feature = "fuzz"))]
pub use rayon::slice::ParallelSliceMut;

#[cfg(feature = "fuzz")]
mod deterministic {
    use rayon::iter::plumbing::{
        Consumer, Folder, Producer, ProducerCallback, Reducer, UnindexedConsumer,
    };
    use std::{
        cmp::Ordering,
        collections::{BTreeMap, BTreeSet, BinaryHeap, HashMap, HashSet, LinkedList, VecDeque},
        iter::Sum,
        ops::{Range, RangeInclusive},
        panic::{catch_unwind, resume_unwind, AssertUnwindSafe},
    };

    // Only constructors in this module mark adapter trees whose drive paths never schedule workers.
    trait SerialDriver: rayon::iter::ParallelIterator {}

    /// A Rayon adapter whose drive delegates to serial source drivers.
    #[derive(Debug, Clone)]
    pub struct Delegated<TIterator>(TIterator);
    impl<TIterator: rayon::iter::ParallelIterator> rayon::iter::ParallelIterator
        for Delegated<TIterator>
    {
        type Item = TIterator::Item;
        fn drive_unindexed<C: UnindexedConsumer<Self::Item>>(self, consumer: C) -> C::Result {
            self.0.drive_unindexed(consumer)
        }
        fn opt_len(&self) -> Option<usize> {
            rayon::iter::ParallelIterator::opt_len(&self.0)
        }
    }
    impl<TIterator: rayon::iter::IndexedParallelIterator> rayon::iter::IndexedParallelIterator
        for Delegated<TIterator>
    {
        fn len(&self) -> usize {
            rayon::iter::IndexedParallelIterator::len(&self.0)
        }
        fn drive<C: Consumer<Self::Item>>(self, consumer: C) -> C::Result {
            drive_serial(self.0, consumer)
        }
        fn with_producer<CB: ProducerCallback<Self::Item>>(self, callback: CB) -> CB::Output {
            self.0.with_producer(callback)
        }
    }
    impl<TIterator: rayon::iter::ParallelIterator> SerialDriver for Delegated<TIterator> {}
    impl<TIterator: rayon::iter::IndexedParallelIterator> SerialDriver for Serial<TIterator> {}
    impl<TIterator: Iterator + Send> SerialDriver for Unindexed<TIterator> where TIterator::Item: Send {}
    impl<TIterator: rayon::iter::IndexedParallelIterator> SerialDriver for Skip<TIterator> {}
    impl<
            TLeft: rayon::iter::ParallelIterator,
            TRight: rayon::iter::ParallelIterator<Item = TLeft::Item>,
        > SerialDriver for Chain<TLeft, TRight>
    {
    }
    impl<TLeft: SerialDriver, TRight: SerialDriver<Item = TLeft::Item>> SerialDriver
        for either::Either<TLeft, TRight>
    {
    }

    /// A lazy indexed iterator driven on the caller without Rayon worker scheduling.
    #[derive(Debug, Clone)]
    pub struct Serial<TIterator>(TIterator);

    impl<TIterator: rayon::iter::IndexedParallelIterator> rayon::iter::ParallelIterator
        for Serial<TIterator>
    {
        type Item = TIterator::Item;

        fn drive_unindexed<C: UnindexedConsumer<Self::Item>>(self, consumer: C) -> C::Result {
            drive_serial(self.0, consumer)
        }

        fn opt_len(&self) -> Option<usize> {
            Some(rayon::iter::IndexedParallelIterator::len(&self.0))
        }
    }

    impl<TIterator: rayon::iter::IndexedParallelIterator> rayon::iter::IndexedParallelIterator
        for Serial<TIterator>
    {
        fn len(&self) -> usize {
            rayon::iter::IndexedParallelIterator::len(&self.0)
        }

        fn drive<C: Consumer<Self::Item>>(self, consumer: C) -> C::Result {
            drive_serial(self.0, consumer)
        }

        fn with_producer<CB: ProducerCallback<Self::Item>>(self, callback: CB) -> CB::Output {
            self.0.with_producer(callback)
        }
    }

    fn drive_serial<I: rayon::iter::IndexedParallelIterator, C: Consumer<I::Item>>(
        iter: I,
        consumer: C,
    ) -> C::Result {
        struct Callback<C> {
            consumer: C,
            len: usize,
        }
        impl<T, C: Consumer<T>> ProducerCallback<T> for Callback<C> {
            type Output = C::Result;
            fn callback<P: Producer<Item = T>>(self, producer: P) -> Self::Output {
                let (producer, _) = producer.split_at(self.len);
                if self.consumer.full() {
                    self.consumer.into_folder().complete()
                } else {
                    producer.fold_with(self.consumer.into_folder()).complete()
                }
            }
        }
        let len = rayon::iter::IndexedParallelIterator::len(&iter);
        iter.with_producer(Callback { consumer, len })
    }

    /// A serial source with no indexed splitting contract, including `par_bridge` inputs.
    #[derive(Debug, Clone)]
    pub struct Unindexed<TIterator> {
        iter: TIterator,
        len: Option<usize>,
    }

    impl<TIterator: Iterator + Send> rayon::iter::ParallelIterator for Unindexed<TIterator>
    where
        TIterator::Item: Send,
    {
        type Item = TIterator::Item;
        fn drive_unindexed<C: UnindexedConsumer<Self::Item>>(self, consumer: C) -> C::Result {
            if consumer.full() {
                consumer.into_folder().complete()
            } else {
                consumer.into_folder().consume_iter(self.iter).complete()
            }
        }
        fn opt_len(&self) -> Option<usize> {
            self.len
        }
    }

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
            where Self: rayon::iter::IntoParallelIterator,
                  <Self as rayon::iter::IntoParallelIterator>::Iter: rayon::iter::IndexedParallelIterator {
                type Item = <Self as rayon::iter::IntoParallelIterator>::Item;
                type Iter = Serial<<Self as rayon::iter::IntoParallelIterator>::Iter>;
                fn into_par_iter(self) -> Self::Iter {
                    Serial(rayon::iter::IntoParallelIterator::into_par_iter(self))
                }
            }
        };
    }
    into_parallel!([T] Vec<T>);
    into_parallel!([T] Option<T>);
    into_parallel!([T, E] Result<T, E>);
    into_parallel!(['a, T] &'a Option<T>);
    into_parallel!(['a, T] &'a mut Option<T>);
    into_parallel!(['a, T, E] &'a Result<T, E>);
    into_parallel!(['a, T, E] &'a mut Result<T, E>);
    into_parallel!([T, const N: usize] [T; N]);
    into_parallel!(['a, T] &'a [T]);
    into_parallel!(['a, T] &'a mut [T]);
    into_parallel!(['a, T, const N: usize] &'a [T; N]);
    into_parallel!(['a, T, const N: usize] &'a mut [T; N]);
    into_parallel!(['a, T] &'a Vec<T>);
    into_parallel!(['a, T] &'a mut Vec<T>);

    macro_rules! into_unindexed {
        ([$($generics:tt)*] $source:ty) => {
            impl<$($generics)*> IntoParallelIterator for $source
            where Self: IntoIterator, <Self as IntoIterator>::IntoIter: Send,
                  <Self as IntoIterator>::Item: Send {
                type Item = <Self as IntoIterator>::Item;
                type Iter = Unindexed<<Self as IntoIterator>::IntoIter>;
                fn into_par_iter(self) -> Self::Iter {
                    let iter = self.into_iter();
                    let (lower, upper) = iter.size_hint();
                    Unindexed { iter, len: upper.filter(|&upper| upper == lower) }
                }
            }
        };
    }
    into_unindexed!([K, V] BTreeMap<K, V>);
    into_unindexed!([T] BTreeSet<T>);
    into_unindexed!([K, V, S] HashMap<K, V, S>);
    into_unindexed!([T, S] HashSet<T, S>);
    into_parallel!([T] VecDeque<T>);
    into_parallel!(['a, T] &'a VecDeque<T>);
    into_parallel!(['a, T] &'a mut VecDeque<T>);
    into_unindexed!([T] LinkedList<T>);
    into_parallel!([T] BinaryHeap<T>);
    into_parallel!(['a, T] &'a BinaryHeap<T>);
    into_unindexed!(['a, K, V] &'a BTreeMap<K, V>);
    into_unindexed!(['a, K, V] &'a mut BTreeMap<K, V>);
    into_unindexed!(['a, T] &'a BTreeSet<T>);
    into_unindexed!(['a, K, V, S] &'a HashMap<K, V, S>);
    into_unindexed!(['a, K, V, S] &'a mut HashMap<K, V, S>);
    into_unindexed!(['a, T, S] &'a HashSet<T, S>);
    into_unindexed!(['a, T] &'a LinkedList<T>);
    into_unindexed!(['a, T] &'a mut LinkedList<T>);

    /// A numeric range source with serial iteration and Rayon's indexed producer, when available.
    #[derive(Debug, Clone)]
    pub struct RangeSource<TIterator>(TIterator);
    impl<TIterator> rayon::iter::ParallelIterator for RangeSource<TIterator>
    where
        TIterator: Iterator
            + Clone
            + Send
            + rayon::iter::IntoParallelIterator<Item = <TIterator as Iterator>::Item>,
        <TIterator as Iterator>::Item: Send,
    {
        type Item = <TIterator as Iterator>::Item;
        fn drive_unindexed<C: UnindexedConsumer<Self::Item>>(self, consumer: C) -> C::Result {
            if consumer.full() {
                consumer.into_folder().complete()
            } else {
                consumer.into_folder().consume_iter(self.0).complete()
            }
        }
        fn opt_len(&self) -> Option<usize> {
            rayon::iter::ParallelIterator::opt_len(
                &rayon::iter::IntoParallelIterator::into_par_iter(self.0.clone()),
            )
        }
    }
    impl<TIterator> rayon::iter::IndexedParallelIterator for RangeSource<TIterator>
    where
        TIterator: Iterator
            + Clone
            + Send
            + rayon::iter::IntoParallelIterator<Item = <TIterator as Iterator>::Item>,
        <TIterator as Iterator>::Item: Send,
        <TIterator as rayon::iter::IntoParallelIterator>::Iter:
            rayon::iter::IndexedParallelIterator,
    {
        fn len(&self) -> usize {
            rayon::iter::IndexedParallelIterator::len(
                &rayon::iter::IntoParallelIterator::into_par_iter(self.0.clone()),
            )
        }
        fn drive<C: Consumer<Self::Item>>(self, consumer: C) -> C::Result {
            drive_serial(self, consumer)
        }
        fn with_producer<CB: ProducerCallback<Self::Item>>(self, callback: CB) -> CB::Output {
            rayon::iter::IndexedParallelIterator::with_producer(
                rayon::iter::IntoParallelIterator::into_par_iter(self.0),
                callback,
            )
        }
    }
    impl<TIterator> SerialDriver for RangeSource<TIterator>
    where
        TIterator: Iterator
            + Clone
            + Send
            + rayon::iter::IntoParallelIterator<Item = <TIterator as Iterator>::Item>,
        <TIterator as Iterator>::Item: Send,
    {
    }
    macro_rules! into_range {
        ($range:ident) => {
            impl<T> IntoParallelIterator for $range<T>
            where
                Self: Iterator
                    + Clone
                    + Send
                    + rayon::iter::IntoParallelIterator<Item = <Self as Iterator>::Item>,
                <Self as Iterator>::Item: Send,
            {
                type Item = <Self as Iterator>::Item;
                type Iter = RangeSource<Self>;
                fn into_par_iter(self) -> Self::Iter {
                    RangeSource(self)
                }
            }
        };
    }
    into_range!(Range);
    into_range!(RangeInclusive);

    /// A lazy map adapter retaining the source compute iterator's indexed guarantees.
    pub type Map<TIterator, TMap> =
        Delegated<rayon::iter::Map<<TIterator as ParallelIterator>::Inner, TMap>>;

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
        &'a T: IntoParallelIterator,
        <&'a T as IntoParallelIterator>::Item: 'a,
    {
        type Item = <&'a T as IntoParallelIterator>::Item;
        type Iter = <&'a T as IntoParallelIterator>::Iter;
        fn par_iter(&'a self) -> Self::Iter {
            self.into_par_iter()
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
        &'a mut T: IntoParallelIterator,
        <&'a mut T as IntoParallelIterator>::Item: 'a,
    {
        type Item = <&'a mut T as IntoParallelIterator>::Item;
        type Iter = <&'a mut T as IntoParallelIterator>::Iter;
        fn par_iter_mut(&'a mut self) -> Self::Iter {
            self.into_par_iter()
        }
    }

    /// Adapts a serial iterator without introducing native worker threads.
    pub trait ParallelBridge: Iterator + Sized + Send
    where
        Self::Item: Send,
    {
        /// Preserves source order and laziness without promising an indexed length.
        fn par_bridge(self) -> Unindexed<Self> {
            Unindexed { iter: self, len: None }
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
            where Self: FromIterator<$item>, $item: Send {
                fn from_par_iter<T: IntoParallelIterator<Item = $item>>(iter: T) -> Self {
                    let items = rayon::iter::ParallelIterator::collect::<Vec<_>>(iter.into_par_iter().into_rayon());
                    items.into_iter().collect()
                }
            }
        };
    }
    from_parallel!([TItem] TItem => Vec<TItem>);
    from_parallel!([TItem] TItem => BTreeSet<TItem>);
    from_parallel!([TKey, TValue] (TKey, TValue) => BTreeMap<TKey, TValue>);
    from_parallel!([TItem, THasher] TItem => HashSet<TItem, THasher>);
    from_parallel!([TKey, TValue, THasher] (TKey, TValue) => HashMap<TKey, TValue, THasher>);
    struct Collected<TCollection>(TCollection);

    impl<TItem: Send, TCollection: FromParallelIterator<TItem>>
        rayon::iter::FromParallelIterator<TItem> for Collected<TCollection>
    {
        fn from_par_iter<T: rayon::iter::IntoParallelIterator<Item = TItem>>(iter: T) -> Self {
            // Only our Result collector constructs this bridge; Rayon retains its serial source
            // drivers while adding the lazy error adapter passed to the destination collection.
            Self(TCollection::from_par_iter(Delegated(
                rayon::iter::IntoParallelIterator::into_par_iter(iter),
            )))
        }
    }

    impl<TItem: Send, TError: Send, TCollection: FromParallelIterator<TItem>>
        FromParallelIterator<Result<TItem, TError>> for Result<TCollection, TError>
    {
        fn from_par_iter<T: IntoParallelIterator<Item = Result<TItem, TError>>>(iter: T) -> Self {
            rayon::iter::ParallelIterator::collect::<Result<Collected<TCollection>, TError>>(
                iter.into_par_iter().into_rayon(),
            )
            .map(|Collected(collection)| collection)
        }
    }

    /// A lazy compute iterator; deterministic builds drive producers on the caller.
    pub trait ParallelIterator: Sized + Send {
        /// The yielded value type.
        type Item: Send;
        /// The lazy Rayon adapter tree, with serial drivers at its scheduling boundaries.
        type Inner: rayon::iter::ParallelIterator<Item = Self::Item>;
        /// Returns the lazy adapter tree without driving it.
        fn into_rayon(self) -> Self::Inner;
        /// Returns the exact result length when known without evaluating callbacks.
        fn opt_len(&self) -> Option<usize> {
            None
        }
        /// Lazily transforms each item.
        fn map<TOutput: Send, TMap: Fn(Self::Item) -> TOutput + Send + Sync>(
            self,
            f: TMap,
        ) -> Map<Self, TMap> {
            Delegated(rayon::iter::ParallelIterator::map(self.into_rayon(), f))
        }
        /// Retains items satisfying the predicate.
        fn filter<TFilter: Fn(&Self::Item) -> bool + Send + Sync>(
            self,
            f: TFilter,
        ) -> Delegated<rayon::iter::Filter<Self::Inner, TFilter>> {
            Delegated(rayon::iter::ParallelIterator::filter(self.into_rayon(), f))
        }
        /// Transforms and retains items with a nonempty result.
        fn filter_map<TOutput: Send, TMap: Fn(Self::Item) -> Option<TOutput> + Send + Sync>(
            self,
            f: TMap,
        ) -> Delegated<rayon::iter::FilterMap<Self::Inner, TMap>> {
            Delegated(rayon::iter::ParallelIterator::filter_map(self.into_rayon(), f))
        }
        /// Expands each item into another deterministic iterator.
        fn flat_map<
            TOutput: IntoParallelIterator,
            TMap: Fn(Self::Item) -> TOutput + Send + Sync,
        >(
            self,
            f: TMap,
        ) -> impl ParallelIterator<Item = TOutput::Item> {
            Delegated(rayon::iter::ParallelIterator::flat_map(self.into_rayon(), move |item| {
                f(item).into_par_iter().into_rayon()
            }))
        }
        /// Runs a callback for every item in source order.
        fn for_each(self, f: impl Fn(Self::Item) + Send + Sync) {
            rayon::iter::ParallelIterator::for_each(self.into_rayon(), f);
        }
        /// Runs fallible callbacks until the first error, preserving that error.
        fn try_for_each<TError: Send>(
            self,
            f: impl Fn(Self::Item) -> Result<(), TError> + Send + Sync,
        ) -> Result<(), TError> {
            rayon::iter::ParallelIterator::try_for_each(self.into_rayon(), f)
        }
        /// Collects results using the collection's parallel-construction implementation.
        fn collect<T: FromParallelIterator<Self::Item>>(self) -> T {
            T::from_par_iter(self)
        }
        /// Folds each serial partition into one value.
        fn fold<
            TAccumulator: Send,
            TIdentity: Fn() -> TAccumulator + Send + Sync,
            TFold: Fn(TAccumulator, Self::Item) -> TAccumulator + Send + Sync,
        >(
            self,
            identity: TIdentity,
            fold: TFold,
        ) -> Delegated<rayon::iter::Fold<Self::Inner, TIdentity, TFold>> {
            Delegated(rayon::iter::ParallelIterator::fold(self.into_rayon(), identity, fold))
        }
        /// Combines all items starting with the identity value.
        fn reduce(
            self,
            identity: impl Fn() -> Self::Item + Send + Sync,
            reduce: impl Fn(Self::Item, Self::Item) -> Self::Item + Send + Sync,
        ) -> Self::Item {
            rayon::iter::ParallelIterator::reduce(self.into_rayon(), identity, reduce)
        }
        /// Combines all items, returning `None` for an empty iterator.
        fn reduce_with(
            self,
            reduce: impl Fn(Self::Item, Self::Item) -> Self::Item + Send + Sync,
        ) -> Option<Self::Item> {
            rayon::iter::ParallelIterator::reduce_with(self.into_rayon(), reduce)
        }
        /// Sums all yielded values.
        fn sum<T: Sum<Self::Item> + Sum<T> + Send>(self) -> T {
            rayon::iter::ParallelIterator::sum(self.into_rayon())
        }
        /// Counts all yielded values.
        fn count(self) -> usize {
            rayon::iter::ParallelIterator::count(self.into_rayon())
        }
        /// Returns the first matching item in deterministic source order, or `None`.
        fn find_any(
            self,
            predicate: impl Fn(&Self::Item) -> bool + Send + Sync,
        ) -> Option<Self::Item> {
            rayon::iter::ParallelIterator::find_any(self.into_rayon(), predicate)
        }
        /// Returns whether any value satisfies the predicate.
        fn any(self, f: impl Fn(Self::Item) -> bool + Send + Sync) -> bool {
            rayon::iter::ParallelIterator::any(self.into_rayon(), f)
        }
        /// Returns whether all values satisfy the predicate.
        fn all(self, f: impl Fn(Self::Item) -> bool + Send + Sync) -> bool {
            rayon::iter::ParallelIterator::all(self.into_rayon(), f)
        }
        /// Concatenates two compute iterators in source order.
        fn chain<T: IntoParallelIterator<Item = Self::Item>>(
            self,
            other: T,
        ) -> Chain<Self::Inner, <T::Iter as ParallelIterator>::Inner> {
            Chain(self.into_rayon(), other.into_par_iter().into_rayon())
        }
    }
    impl<TIterator: SerialDriver> ParallelIterator for TIterator {
        type Item = TIterator::Item;
        type Inner = Self;
        fn into_rayon(self) -> Self {
            self
        }
        fn opt_len(&self) -> Option<usize> {
            rayon::iter::ParallelIterator::opt_len(self)
        }
    }

    /// A compute iterator with an exact length and stable source indices.
    pub trait IndexedParallelIterator:
        ParallelIterator<Inner: rayon::iter::IndexedParallelIterator>
    {
        /// Returns the number of items without evaluating lazy mappings.
        fn len(&self) -> usize;
        /// Returns whether there are no items.
        fn is_empty(&self) -> bool {
            self.len() == 0
        }
        /// Attaches each item's zero-based source index.
        fn enumerate(self) -> Serial<rayon::iter::Enumerate<Self::Inner>> {
            Serial(rayon::iter::IndexedParallelIterator::enumerate(self.into_rayon()))
        }
        /// Preserves source order without partition scheduling.
        fn by_exponential_blocks(self) -> Self {
            self
        }
        /// Takes at most the first `count` items.
        fn take(self, count: usize) -> Serial<rayon::iter::Take<Self::Inner>> {
            Serial(rayon::iter::IndexedParallelIterator::take(self.into_rayon(), count))
        }
        /// Skips the first `count` items, driving the skipped prefix when consumed.
        fn skip(self, count: usize) -> Skip<Self::Inner> {
            let count = count.min(self.len());
            Skip { base: self.into_rayon(), count }
        }
        /// Reverses source indices without evaluating lazy mappings.
        fn rev(self) -> Serial<rayon::iter::Rev<Self::Inner>> {
            Serial(rayon::iter::IndexedParallelIterator::rev(self.into_rayon()))
        }
        /// Pairs values at matching indices, stopping at the shorter iterator.
        fn zip<T: IntoParallelIterator>(
            self,
            other: T,
        ) -> Serial<rayon::iter::Zip<Self::Inner, <T::Iter as ParallelIterator>::Inner>>
        where
            T::Iter: IndexedParallelIterator,
        {
            Serial(rayon::iter::IndexedParallelIterator::zip(
                self.into_rayon(),
                other.into_par_iter().into_rayon(),
            ))
        }
    }
    impl<TIterator: SerialDriver + rayon::iter::IndexedParallelIterator> IndexedParallelIterator
        for TIterator
    {
        fn len(&self) -> usize {
            rayon::iter::IndexedParallelIterator::len(self)
        }
    }

    /// A concatenation with serial branch drivers and producer-level indexed splitting.
    #[derive(Debug, Clone)]
    pub struct Chain<TLeft, TRight>(TLeft, TRight);
    impl<
            TLeft: rayon::iter::ParallelIterator,
            TRight: rayon::iter::ParallelIterator<Item = TLeft::Item>,
        > rayon::iter::ParallelIterator for Chain<TLeft, TRight>
    {
        type Item = TLeft::Item;
        fn drive_unindexed<C: UnindexedConsumer<Self::Item>>(self, consumer: C) -> C::Result {
            let (left, right, reducer) =
                if let Some(len) = rayon::iter::ParallelIterator::opt_len(&self.0) {
                    consumer.split_at(len)
                } else {
                    let reducer = consumer.to_reducer();
                    (consumer.split_off_left(), consumer, reducer)
                };
            // Both branches must be driven even if the other branch panics or short-circuits.
            let left = catch_unwind(AssertUnwindSafe(|| self.0.drive_unindexed(left)));
            let right = catch_unwind(AssertUnwindSafe(|| self.1.drive_unindexed(right)));
            // Rayon's join propagates the left panic when both branches panic.
            match (left, right) {
                (Ok(left), Ok(right)) => reducer.reduce(left, right),
                (Err(panic), _) | (_, Err(panic)) => resume_unwind(panic),
            }
        }
        fn opt_len(&self) -> Option<usize> {
            rayon::iter::ParallelIterator::opt_len(&self.0)?
                .checked_add(rayon::iter::ParallelIterator::opt_len(&self.1)?)
        }
    }
    impl<
            TLeft: rayon::iter::IndexedParallelIterator,
            TRight: rayon::iter::IndexedParallelIterator<Item = TLeft::Item>,
        > rayon::iter::IndexedParallelIterator for Chain<TLeft, TRight>
    {
        fn len(&self) -> usize {
            rayon::iter::IndexedParallelIterator::len(&self.0)
                .checked_add(rayon::iter::IndexedParallelIterator::len(&self.1))
                .expect("indexed chain length fits usize")
        }
        fn drive<C: Consumer<Self::Item>>(self, consumer: C) -> C::Result {
            drive_serial(self, consumer)
        }
        fn with_producer<CB: ProducerCallback<Self::Item>>(self, callback: CB) -> CB::Output {
            rayon::iter::IndexedParallelIterator::with_producer(
                rayon::iter::ParallelIterator::chain(self.0, self.1),
                callback,
            )
        }
    }

    /// An indexed skip whose prefix is driven at terminal producer preparation.
    #[derive(Debug, Clone)]
    pub struct Skip<TIterator> {
        base: TIterator,
        count: usize,
    }
    impl<TIterator: rayon::iter::IndexedParallelIterator> rayon::iter::ParallelIterator
        for Skip<TIterator>
    {
        type Item = TIterator::Item;
        fn drive_unindexed<C: UnindexedConsumer<Self::Item>>(self, consumer: C) -> C::Result {
            drive_serial(self, consumer)
        }
        fn opt_len(&self) -> Option<usize> {
            Some(rayon::iter::IndexedParallelIterator::len(self))
        }
    }
    impl<TIterator: rayon::iter::IndexedParallelIterator> rayon::iter::IndexedParallelIterator
        for Skip<TIterator>
    {
        fn len(&self) -> usize {
            rayon::iter::IndexedParallelIterator::len(&self.base) - self.count
        }
        fn drive<C: Consumer<Self::Item>>(self, consumer: C) -> C::Result {
            drive_serial(self, consumer)
        }
        fn with_producer<CB: ProducerCallback<Self::Item>>(self, callback: CB) -> CB::Output {
            struct Callback<CB> {
                callback: CB,
                count: usize,
            }
            impl<T, CB: ProducerCallback<T>> ProducerCallback<T> for Callback<CB> {
                type Output = CB::Output;
                fn callback<P: Producer<Item = T>>(self, base: P) -> CB::Output {
                    let (before, after) = base.split_at(self.count);
                    // Like Rayon's skip scope, a prefix panic must not cancel the suffix callback.
                    let prefix =
                        catch_unwind(AssertUnwindSafe(|| before.into_iter().for_each(drop)));
                    let output = self.callback.callback(after);
                    if let Err(panic) = prefix {
                        resume_unwind(panic);
                    }
                    output
                }
            }
            self.base.with_producer(Callback { callback, count: self.count })
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
        fn par_sort_unstable_by<TCompare: Fn(&T, &T) -> Ordering + Sync>(
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
        fn par_sort_unstable_by<TCompare: Fn(&T, &T) -> Ordering + Sync>(
            &mut self,
            compare: TCompare,
        ) {
            self.sort_unstable_by(compare);
        }
    }
}
