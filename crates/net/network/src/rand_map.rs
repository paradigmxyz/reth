//! Randomized-iteration map and set wrappers around [`IndexMap`] and [`IndexSet`].
//!
//! These types delegate all map/set operations to the underlying `indexmap` types, but
//! randomize iteration order on every `.iter()` call using a coprime-stride permutation.
//! This prevents fairness bugs caused by deterministic iteration order — e.g. peer
//! starvation in [`TransactionFetcher`](crate::transactions::TransactionFetcher), where
//! stable `HashMap` ordering causes the same peers to be selected every time.
//!
//! # Iteration strategy
//!
//! Each call to `.iter()` picks a random starting index and a random stride coprime to
//! `len`. Walking `start, start + stride, start + 2*stride, …` mod `len` visits every
//! index exactly once. This is strictly better than Go's per-call randomization, which
//! only shuffles the starting bucket but walks sequentially within bucket groups.
//!
//! [`IndexMap`]: indexmap::IndexMap
//! [`IndexSet`]: indexmap::IndexSet

use std::{
    collections::hash_map::RandomState,
    fmt,
    hash::{BuildHasher, Hash},
    iter::FusedIterator,
    ops::{Index, IndexMut},
};

use indexmap::{IndexMap, IndexSet};
use rand::Rng;

// ---------------------------------------------------------------------------
// Coprime-stride helpers
// ---------------------------------------------------------------------------

/// Greatest common divisor (Euclid's algorithm).
const fn gcd(mut a: usize, mut b: usize) -> usize {
    while b != 0 {
        let t = b;
        b = a % b;
        a = t;
    }
    a
}

/// Returns `(start, stride)` for a coprime-stride walk over `len` elements.
fn rand_start_stride(len: usize) -> (usize, usize) {
    debug_assert!(len > 0);
    if len == 1 {
        return (0, 1);
    }
    let mut rng = rand::rng();
    let start = rng.random_range(0..len);
    let stride = if len.is_power_of_two() {
        // Any odd number is coprime to a power of two — no rejection needed.
        rng.random_range(0..len / 2) * 2 + 1
    } else {
        loop {
            let candidate = rng.random_range(1..len);
            if gcd(candidate, len) == 1 {
                break candidate;
            }
        }
    };
    (start, stride)
}

// ---------------------------------------------------------------------------
// Randomized iterator
// ---------------------------------------------------------------------------

/// An iterator that visits every entry of an [`IndexMap`] exactly once in a
/// pseudorandom order determined by a coprime-stride permutation.
pub struct RandIter<'a, K, V, S = RandomState> {
    map: &'a IndexMap<K, V, S>,
    current: usize,
    stride: usize,
    remaining: usize,
}

impl<K: fmt::Debug, V: fmt::Debug, S> fmt::Debug for RandIter<'_, K, V, S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RandIter").field("remaining", &self.remaining).finish_non_exhaustive()
    }
}

impl<'a, K, V, S> Iterator for RandIter<'a, K, V, S> {
    type Item = (&'a K, &'a V);

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        if self.remaining == 0 {
            return None;
        }
        let idx = self.current;
        self.current = (self.current + self.stride) % self.map.len();
        self.remaining -= 1;
        self.map.get_index(idx)
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.remaining, Some(self.remaining))
    }
}

impl<K, V, S> ExactSizeIterator for RandIter<'_, K, V, S> {}
impl<K, V, S> FusedIterator for RandIter<'_, K, V, S> {}

/// An iterator that visits every key of an [`IndexMap`] exactly once in random order.
pub struct RandKeys<'a, K, V, S = RandomState> {
    inner: RandIter<'a, K, V, S>,
}

impl<K: fmt::Debug, V: fmt::Debug, S> fmt::Debug for RandKeys<'_, K, V, S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RandKeys").finish_non_exhaustive()
    }
}

impl<'a, K, V, S> Iterator for RandKeys<'a, K, V, S> {
    type Item = &'a K;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next().map(|(k, _)| k)
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl<K, V, S> ExactSizeIterator for RandKeys<'_, K, V, S> {}
impl<K, V, S> FusedIterator for RandKeys<'_, K, V, S> {}

/// An iterator that visits every value of an [`IndexMap`] exactly once in random order.
pub struct RandValues<'a, K, V, S = RandomState> {
    inner: RandIter<'a, K, V, S>,
}

impl<K: fmt::Debug, V: fmt::Debug, S> fmt::Debug for RandValues<'_, K, V, S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RandValues").finish_non_exhaustive()
    }
}

impl<'a, K, V, S> Iterator for RandValues<'a, K, V, S> {
    type Item = &'a V;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next().map(|(_, v)| v)
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl<K, V, S> ExactSizeIterator for RandValues<'_, K, V, S> {}
impl<K, V, S> FusedIterator for RandValues<'_, K, V, S> {}

/// An owning iterator that consumes a [`RandMap`] and yields entries in random order.
///
/// Each call to [`next()`](Iterator::next) picks a uniformly random index and
/// swap-removes it from the underlying [`IndexMap`], giving a uniform permutation
/// with O(1) extra memory.
pub struct RandIntoIter<K, V, S = RandomState> {
    inner: IndexMap<K, V, S>,
}

impl<K: fmt::Debug, V: fmt::Debug, S> fmt::Debug for RandIntoIter<K, V, S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RandIntoIter").field("remaining", &self.inner.len()).finish_non_exhaustive()
    }
}

impl<K, V, S> Iterator for RandIntoIter<K, V, S> {
    type Item = (K, V);

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        let len = self.inner.len();
        if len == 0 {
            return None;
        }
        let idx = rand::rng().random_range(0..len);
        self.inner.swap_remove_index(idx)
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        let len = self.inner.len();
        (len, Some(len))
    }
}

impl<K, V, S> ExactSizeIterator for RandIntoIter<K, V, S> {}
impl<K, V, S> FusedIterator for RandIntoIter<K, V, S> {}

// ---------------------------------------------------------------------------
// Set iterator
// ---------------------------------------------------------------------------

/// An iterator that visits every element of an [`IndexSet`] exactly once in random order.
pub struct RandSetIter<'a, T, S = RandomState> {
    set: &'a IndexSet<T, S>,
    current: usize,
    stride: usize,
    remaining: usize,
}

impl<T: fmt::Debug, S> fmt::Debug for RandSetIter<'_, T, S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RandSetIter").field("remaining", &self.remaining).finish_non_exhaustive()
    }
}

impl<'a, T, S> Iterator for RandSetIter<'a, T, S> {
    type Item = &'a T;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        if self.remaining == 0 {
            return None;
        }
        let idx = self.current;
        self.current = (self.current + self.stride) % self.set.len();
        self.remaining -= 1;
        self.set.get_index(idx)
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.remaining, Some(self.remaining))
    }
}

impl<T, S> ExactSizeIterator for RandSetIter<'_, T, S> {}
impl<T, S> FusedIterator for RandSetIter<'_, T, S> {}

/// An owning iterator that consumes a [`RandSet`] and yields elements in random order.
///
/// See [`RandIntoIter`] for details on the strategy.
pub struct RandSetIntoIter<T, S = RandomState> {
    inner: IndexSet<T, S>,
}

impl<T: fmt::Debug, S> fmt::Debug for RandSetIntoIter<T, S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RandSetIntoIter")
            .field("remaining", &self.inner.len())
            .finish_non_exhaustive()
    }
}

impl<T, S> Iterator for RandSetIntoIter<T, S> {
    type Item = T;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        let len = self.inner.len();
        if len == 0 {
            return None;
        }
        let idx = rand::rng().random_range(0..len);
        self.inner.swap_remove_index(idx)
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        let len = self.inner.len();
        (len, Some(len))
    }
}

impl<T, S> ExactSizeIterator for RandSetIntoIter<T, S> {}
impl<T, S> FusedIterator for RandSetIntoIter<T, S> {}

// ---------------------------------------------------------------------------
// RandMap
// ---------------------------------------------------------------------------

/// A [`IndexMap`]-backed map whose `.iter()` yields entries in a **random order** on every
/// call.
///
/// All map operations (`insert`, `remove`, `get`, …) delegate directly to [`IndexMap`].
/// Only iteration is affected: each `.iter()` / `.keys()` / `.values()` call picks a
/// fresh random offset and coprime stride, producing a full permutation with zero
/// allocation overhead.
///
/// This prevents fairness bugs caused by deterministic iteration order. See the
/// [module documentation](self) for details.
#[must_use]
pub struct RandMap<K, V, S = RandomState> {
    inner: IndexMap<K, V, S>,
}

impl<K: fmt::Debug, V: fmt::Debug, S> fmt::Debug for RandMap<K, V, S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_map().entries(self.inner.iter()).finish()
    }
}

impl<K: Clone, V: Clone, S: Clone> Clone for RandMap<K, V, S> {
    fn clone(&self) -> Self {
        Self { inner: self.inner.clone() }
    }
}

impl<K, V, S: Default> Default for RandMap<K, V, S> {
    fn default() -> Self {
        Self { inner: IndexMap::default() }
    }
}

impl<K, V> RandMap<K, V> {
    /// Creates an empty `RandMap` with the specified capacity.
    pub fn with_capacity(n: usize) -> Self {
        Self { inner: IndexMap::with_capacity_and_hasher(n, RandomState::default()) }
    }
}

impl<K, V, S> RandMap<K, V, S> {
    /// Creates an empty `RandMap` with the given hasher.
    pub const fn with_hasher(hash_builder: S) -> Self {
        Self { inner: IndexMap::with_hasher(hash_builder) }
    }

    /// Creates an empty `RandMap` with the given capacity and hasher.
    pub fn with_capacity_and_hasher(n: usize, hash_builder: S) -> Self {
        Self { inner: IndexMap::with_capacity_and_hasher(n, hash_builder) }
    }

    /// Returns the number of elements in the map.
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// Returns `true` if the map contains no elements.
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Returns a reference to the underlying [`IndexMap`].
    pub const fn inner(&self) -> &IndexMap<K, V, S> {
        &self.inner
    }

    /// Returns a mutable reference to the underlying [`IndexMap`].
    pub fn inner_mut(&mut self) -> &mut IndexMap<K, V, S> {
        &mut self.inner
    }

    /// Consumes the wrapper, returning the underlying [`IndexMap`].
    pub fn into_inner(self) -> IndexMap<K, V, S> {
        self.inner
    }

    /// Returns an iterator visiting all key-value pairs in **random order**.
    ///
    /// Each call produces a different permutation.
    pub fn iter(&self) -> RandIter<'_, K, V, S> {
        let len = self.inner.len();
        if len == 0 {
            return RandIter { map: &self.inner, current: 0, stride: 1, remaining: 0 };
        }
        let (current, stride) = rand_start_stride(len);
        RandIter { map: &self.inner, current, stride, remaining: len }
    }

    /// Returns an iterator visiting all keys in **random order**.
    pub fn keys(&self) -> RandKeys<'_, K, V, S> {
        RandKeys { inner: self.iter() }
    }

    /// Returns an iterator visiting all values in **random order**.
    pub fn values(&self) -> RandValues<'_, K, V, S> {
        RandValues { inner: self.iter() }
    }

    /// Returns an iterator visiting all key-value pairs with mutable references to the
    /// values.
    ///
    /// Iteration order is the underlying [`IndexMap`] order (not randomized), since
    /// mutable iteration is typically used for bulk mutation rather than selection.
    pub fn iter_mut(&mut self) -> indexmap::map::IterMut<'_, K, V> {
        self.inner.iter_mut()
    }

    /// Returns an iterator visiting all values mutably.
    ///
    /// Iteration order is the underlying [`IndexMap`] order (not randomized).
    pub fn values_mut(&mut self) -> indexmap::map::ValuesMut<'_, K, V> {
        self.inner.values_mut()
    }

    /// Clears the map, removing all entries.
    pub fn clear(&mut self) {
        self.inner.clear();
    }

    /// Reserves capacity for at least `additional` more elements.
    pub fn reserve(&mut self, additional: usize) {
        self.inner.reserve(additional);
    }

    /// Shrinks the capacity of the map as much as possible.
    pub fn shrink_to_fit(&mut self) {
        self.inner.shrink_to_fit();
    }

    /// Retains only the elements specified by the predicate.
    pub fn retain<F>(&mut self, keep: F)
    where
        F: FnMut(&K, &mut V) -> bool,
    {
        self.inner.retain(keep);
    }
}

impl<K, V, S> RandMap<K, V, S>
where
    K: Hash + Eq,
    S: BuildHasher,
{
    /// Returns a reference to the value corresponding to the key.
    pub fn get<Q>(&self, key: &Q) -> Option<&V>
    where
        Q: ?Sized + Hash + indexmap::Equivalent<K>,
    {
        self.inner.get(key)
    }

    /// Returns a mutable reference to the value corresponding to the key.
    pub fn get_mut<Q>(&mut self, key: &Q) -> Option<&mut V>
    where
        Q: ?Sized + Hash + indexmap::Equivalent<K>,
    {
        self.inner.get_mut(key)
    }

    /// Returns the key-value pair corresponding to the supplied key.
    pub fn get_key_value<Q>(&self, key: &Q) -> Option<(&K, &V)>
    where
        Q: ?Sized + Hash + indexmap::Equivalent<K>,
    {
        self.inner.get_key_value(key)
    }

    /// Returns `true` if the map contains a value for the specified key.
    pub fn contains_key<Q>(&self, key: &Q) -> bool
    where
        Q: ?Sized + Hash + indexmap::Equivalent<K>,
    {
        self.inner.contains_key(key)
    }

    /// Inserts a key-value pair into the map.
    pub fn insert(&mut self, key: K, value: V) -> Option<V> {
        self.inner.insert(key, value)
    }

    /// Inserts a key-value pair, returning the full result.
    pub fn insert_full(&mut self, key: K, value: V) -> (usize, Option<V>) {
        self.inner.insert_full(key, value)
    }

    /// Gets the entry for the given key for in-place manipulation.
    pub fn entry(&mut self, key: K) -> indexmap::map::Entry<'_, K, V> {
        self.inner.entry(key)
    }

    /// Removes a key from the map, returning the value if the key was previously in the
    /// map.
    ///
    /// This is equivalent to [`swap_remove`](Self::swap_remove). Because iteration order
    /// is randomized, the swap-remove reordering is unobservable.
    pub fn remove<Q>(&mut self, key: &Q) -> Option<V>
    where
        Q: ?Sized + Hash + indexmap::Equivalent<K>,
    {
        self.swap_remove(key)
    }

    /// Removes a key from the map, returning the stored key and value if the
    /// key was previously in the map.
    pub fn remove_entry<Q>(&mut self, key: &Q) -> Option<(K, V)>
    where
        Q: ?Sized + Hash + indexmap::Equivalent<K>,
    {
        self.swap_remove_entry(key)
    }

    /// Removes a key from the map using swap-remove.
    pub fn swap_remove<Q>(&mut self, key: &Q) -> Option<V>
    where
        Q: ?Sized + Hash + indexmap::Equivalent<K>,
    {
        self.inner.swap_remove(key)
    }

    /// Removes a key from the map using swap-remove, returning both key and value.
    pub fn swap_remove_entry<Q>(&mut self, key: &Q) -> Option<(K, V)>
    where
        Q: ?Sized + Hash + indexmap::Equivalent<K>,
    {
        self.inner.swap_remove_entry(key)
    }

    /// Removes a key from the map using shift-remove (preserves insertion order at cost of
    /// O(n)).
    pub fn shift_remove<Q>(&mut self, key: &Q) -> Option<V>
    where
        Q: ?Sized + Hash + indexmap::Equivalent<K>,
    {
        self.inner.shift_remove(key)
    }

    /// Removes a key from the map using shift-remove, returning both key and value.
    pub fn shift_remove_entry<Q>(&mut self, key: &Q) -> Option<(K, V)>
    where
        Q: ?Sized + Hash + indexmap::Equivalent<K>,
    {
        self.inner.shift_remove_entry(key)
    }
}

impl<K, V, S> From<IndexMap<K, V, S>> for RandMap<K, V, S> {
    fn from(inner: IndexMap<K, V, S>) -> Self {
        Self { inner }
    }
}

impl<K, V, S> From<RandMap<K, V, S>> for IndexMap<K, V, S> {
    fn from(rand_map: RandMap<K, V, S>) -> Self {
        rand_map.inner
    }
}

impl<K, V, Q, S> Index<&Q> for RandMap<K, V, S>
where
    Q: ?Sized + Hash + indexmap::Equivalent<K>,
    K: Hash + Eq,
    S: BuildHasher,
{
    type Output = V;

    fn index(&self, key: &Q) -> &V {
        self.inner.index(key)
    }
}

impl<K, V, Q, S> IndexMut<&Q> for RandMap<K, V, S>
where
    Q: ?Sized + Hash + indexmap::Equivalent<K>,
    K: Hash + Eq,
    S: BuildHasher,
{
    fn index_mut(&mut self, key: &Q) -> &mut V {
        self.inner.index_mut(key)
    }
}

impl<K, V, S> IntoIterator for RandMap<K, V, S> {
    type Item = (K, V);
    type IntoIter = RandIntoIter<K, V, S>;

    fn into_iter(self) -> Self::IntoIter {
        RandIntoIter { inner: self.inner }
    }
}

impl<'a, K, V, S> IntoIterator for &'a RandMap<K, V, S> {
    type Item = (&'a K, &'a V);
    type IntoIter = RandIter<'a, K, V, S>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl<K, V, S> FromIterator<(K, V)> for RandMap<K, V, S>
where
    K: Hash + Eq,
    S: BuildHasher + Default,
{
    fn from_iter<I: IntoIterator<Item = (K, V)>>(iter: I) -> Self {
        Self { inner: IndexMap::from_iter(iter) }
    }
}

impl<K, V, S> Extend<(K, V)> for RandMap<K, V, S>
where
    K: Hash + Eq,
    S: BuildHasher,
{
    fn extend<I: IntoIterator<Item = (K, V)>>(&mut self, iter: I) {
        self.inner.extend(iter);
    }
}

impl<K, V, S> PartialEq for RandMap<K, V, S>
where
    K: Hash + Eq,
    V: PartialEq,
    S: BuildHasher,
{
    fn eq(&self, other: &Self) -> bool {
        self.inner == other.inner
    }
}

impl<K, V, S> Eq for RandMap<K, V, S>
where
    K: Hash + Eq,
    V: Eq,
    S: BuildHasher,
{
}

// ---------------------------------------------------------------------------
// RandSet
// ---------------------------------------------------------------------------

/// A [`IndexSet`]-backed set whose `.iter()` yields elements in a **random order** on
/// every call.
///
/// See [`RandMap`] for design rationale.
#[must_use]
pub struct RandSet<T, S = RandomState> {
    inner: IndexSet<T, S>,
}

impl<T: fmt::Debug, S> fmt::Debug for RandSet<T, S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_set().entries(self.inner.iter()).finish()
    }
}

impl<T: Clone, S: Clone> Clone for RandSet<T, S> {
    fn clone(&self) -> Self {
        Self { inner: self.inner.clone() }
    }
}

impl<T, S: Default> Default for RandSet<T, S> {
    fn default() -> Self {
        Self { inner: IndexSet::default() }
    }
}

impl<T> RandSet<T> {
    /// Creates an empty `RandSet` with the specified capacity.
    pub fn with_capacity(n: usize) -> Self {
        Self { inner: IndexSet::with_capacity_and_hasher(n, RandomState::default()) }
    }
}

impl<T, S> RandSet<T, S> {
    /// Creates an empty `RandSet` with the given hasher.
    pub const fn with_hasher(hash_builder: S) -> Self {
        Self { inner: IndexSet::with_hasher(hash_builder) }
    }

    /// Creates an empty `RandSet` with the given capacity and hasher.
    pub fn with_capacity_and_hasher(n: usize, hash_builder: S) -> Self {
        Self { inner: IndexSet::with_capacity_and_hasher(n, hash_builder) }
    }

    /// Returns the number of elements in the set.
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// Returns `true` if the set contains no elements.
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Returns a reference to the underlying [`IndexSet`].
    pub const fn inner(&self) -> &IndexSet<T, S> {
        &self.inner
    }

    /// Returns a mutable reference to the underlying [`IndexSet`].
    pub fn inner_mut(&mut self) -> &mut IndexSet<T, S> {
        &mut self.inner
    }

    /// Consumes the wrapper, returning the underlying [`IndexSet`].
    pub fn into_inner(self) -> IndexSet<T, S> {
        self.inner
    }

    /// Returns an iterator visiting all elements in **random order**.
    pub fn iter(&self) -> RandSetIter<'_, T, S> {
        let len = self.inner.len();
        if len == 0 {
            return RandSetIter { set: &self.inner, current: 0, stride: 1, remaining: 0 };
        }
        let (current, stride) = rand_start_stride(len);
        RandSetIter { set: &self.inner, current, stride, remaining: len }
    }

    /// Clears the set, removing all elements.
    pub fn clear(&mut self) {
        self.inner.clear();
    }

    /// Reserves capacity for at least `additional` more elements.
    pub fn reserve(&mut self, additional: usize) {
        self.inner.reserve(additional);
    }

    /// Shrinks the capacity of the set as much as possible.
    pub fn shrink_to_fit(&mut self) {
        self.inner.shrink_to_fit();
    }

    /// Retains only the elements specified by the predicate.
    pub fn retain<F>(&mut self, keep: F)
    where
        F: FnMut(&T) -> bool,
    {
        self.inner.retain(keep);
    }
}

impl<T, S> RandSet<T, S>
where
    T: Hash + Eq,
    S: BuildHasher,
{
    /// Returns `true` if the set contains the specified value.
    pub fn contains<Q>(&self, value: &Q) -> bool
    where
        Q: ?Sized + Hash + indexmap::Equivalent<T>,
    {
        self.inner.contains(value)
    }

    /// Adds a value to the set. Returns `true` if the value was newly inserted.
    pub fn insert(&mut self, value: T) -> bool {
        self.inner.insert(value)
    }

    /// Inserts a value, returning the index and whether it was newly inserted.
    pub fn insert_full(&mut self, value: T) -> (usize, bool) {
        self.inner.insert_full(value)
    }

    /// Removes a value from the set. Returns `true` if the value was present.
    ///
    /// This is equivalent to [`swap_remove`](Self::swap_remove). Because iteration order
    /// is randomized, the swap-remove reordering is unobservable.
    pub fn remove<Q>(&mut self, value: &Q) -> bool
    where
        Q: ?Sized + Hash + indexmap::Equivalent<T>,
    {
        self.swap_remove(value)
    }

    /// Removes a value from the set using swap-remove.
    pub fn swap_remove<Q>(&mut self, value: &Q) -> bool
    where
        Q: ?Sized + Hash + indexmap::Equivalent<T>,
    {
        self.inner.swap_remove(value)
    }

    /// Removes a value from the set using shift-remove (preserves insertion order at cost
    /// of O(n)).
    pub fn shift_remove<Q>(&mut self, value: &Q) -> bool
    where
        Q: ?Sized + Hash + indexmap::Equivalent<T>,
    {
        self.inner.shift_remove(value)
    }

    /// Returns a reference to the value in the set, if any, that is equal to the given
    /// value.
    pub fn get<Q>(&self, value: &Q) -> Option<&T>
    where
        Q: ?Sized + Hash + indexmap::Equivalent<T>,
    {
        self.inner.get(value)
    }
}

impl<T, S> From<IndexSet<T, S>> for RandSet<T, S> {
    fn from(inner: IndexSet<T, S>) -> Self {
        Self { inner }
    }
}

impl<T, S> From<RandSet<T, S>> for IndexSet<T, S> {
    fn from(rand_set: RandSet<T, S>) -> Self {
        rand_set.inner
    }
}

impl<T, S> IntoIterator for RandSet<T, S> {
    type Item = T;
    type IntoIter = RandSetIntoIter<T, S>;

    fn into_iter(self) -> Self::IntoIter {
        RandSetIntoIter { inner: self.inner }
    }
}

impl<'a, T, S> IntoIterator for &'a RandSet<T, S> {
    type Item = &'a T;
    type IntoIter = RandSetIter<'a, T, S>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl<T, S> FromIterator<T> for RandSet<T, S>
where
    T: Hash + Eq,
    S: BuildHasher + Default,
{
    fn from_iter<I: IntoIterator<Item = T>>(iter: I) -> Self {
        Self { inner: IndexSet::from_iter(iter) }
    }
}

impl<T, S> Extend<T> for RandSet<T, S>
where
    T: Hash + Eq,
    S: BuildHasher,
{
    fn extend<I: IntoIterator<Item = T>>(&mut self, iter: I) {
        self.inner.extend(iter);
    }
}

impl<T, S> PartialEq for RandSet<T, S>
where
    T: Hash + Eq,
    S: BuildHasher,
{
    fn eq(&self, other: &Self) -> bool {
        self.inner == other.inner
    }
}

impl<T, S> Eq for RandSet<T, S>
where
    T: Hash + Eq,
    S: BuildHasher,
{
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn map_basic_operations() {
        let mut map = RandMap::<&str, i32>::default();
        assert!(map.is_empty());

        map.insert("a", 1);
        map.insert("b", 2);
        map.insert("c", 3);
        assert_eq!(map.len(), 3);
        assert_eq!(map.get("a"), Some(&1));
        assert_eq!(map.get("d"), None);
        assert!(map.contains_key("b"));
        assert!(!map.contains_key("z"));

        assert_eq!(map["a"], 1);

        map.swap_remove("b");
        assert_eq!(map.len(), 2);
        assert!(!map.contains_key("b"));
    }

    #[test]
    fn map_remove_shims() {
        let mut map = RandMap::<&str, i32>::default();
        map.insert("a", 1);
        map.insert("b", 2);
        map.insert("c", 3);

        assert_eq!(map.remove("b"), Some(2));
        assert_eq!(map.len(), 2);
        assert!(!map.contains_key("b"));
        assert_eq!(map.remove("z"), None);

        assert_eq!(map.remove_entry("a"), Some(("a", 1)));
        assert_eq!(map.len(), 1);
        assert!(!map.contains_key("a"));
        assert_eq!(map.remove_entry("a"), None);
    }

    #[test]
    fn set_basic_operations() {
        let mut set = RandSet::<&str>::default();
        assert!(set.is_empty());

        assert!(set.insert("x"));
        assert!(set.insert("y"));
        assert!(!set.insert("x"));
        assert_eq!(set.len(), 2);
        assert!(set.contains("x"));
        assert!(!set.contains("z"));

        set.swap_remove("x");
        assert_eq!(set.len(), 1);
        assert!(!set.contains("x"));
    }

    #[test]
    fn set_remove_shim() {
        let mut set = RandSet::<&str>::default();
        set.insert("a");
        set.insert("b");

        assert!(set.remove("a"));
        assert_eq!(set.len(), 1);
        assert!(!set.contains("a"));
        assert!(!set.remove("a"));
    }

    #[test]
    fn iter_visits_all_elements_exactly_once() {
        let map: RandMap<i32, i32> = (0..100).map(|i| (i, i * 10)).collect();
        let keys: BTreeSet<i32> = map.keys().copied().collect();
        let expected: BTreeSet<i32> = (0..100).collect();
        assert_eq!(keys, expected);

        let values: BTreeSet<i32> = map.values().copied().collect();
        let expected_values: BTreeSet<i32> = (0..100).map(|i| i * 10).collect();
        assert_eq!(values, expected_values);
    }

    #[test]
    fn set_iter_visits_all_elements_exactly_once() {
        let set: RandSet<i32> = (0..100).collect();
        let elements: BTreeSet<i32> = set.iter().copied().collect();
        let expected: BTreeSet<i32> = (0..100).collect();
        assert_eq!(elements, expected);
    }

    #[test]
    fn into_iter_visits_all_elements() {
        let map: RandMap<i32, i32> = (0..50).map(|i| (i, i)).collect();
        let keys: BTreeSet<i32> = map.into_iter().map(|(k, _)| k).collect();
        let expected: BTreeSet<i32> = (0..50).collect();
        assert_eq!(keys, expected);
    }

    #[test]
    fn set_into_iter_visits_all_elements() {
        let set: RandSet<i32> = (0..50).collect();
        let elements: BTreeSet<i32> = set.into_iter().collect();
        let expected: BTreeSet<i32> = (0..50).collect();
        assert_eq!(elements, expected);
    }

    #[test]
    fn two_iterations_can_differ() {
        let map: RandMap<i32, i32> = (0..20).map(|i| (i, i)).collect();

        let order1: Vec<i32> = map.keys().copied().collect();
        let order2: Vec<i32> = map.keys().copied().collect();

        let mut any_differ = order1 != order2;
        for _ in 0..10 {
            let order_n: Vec<i32> = map.keys().copied().collect();
            if order_n != order1 {
                any_differ = true;
                break;
            }
        }
        assert!(any_differ, "iteration order should differ between calls");
    }

    #[test]
    fn set_two_iterations_can_differ() {
        let set: RandSet<i32> = (0..20).collect();
        let order1: Vec<&i32> = set.iter().collect();
        let mut any_differ = false;
        for _ in 0..10 {
            let order_n: Vec<&i32> = set.iter().collect();
            if order_n != order1 {
                any_differ = true;
                break;
            }
        }
        assert!(any_differ, "set iteration order should differ between calls");
    }

    #[test]
    fn empty_iteration() {
        let map = RandMap::<i32, i32>::default();
        assert_eq!(map.iter().count(), 0);
        assert_eq!(map.keys().count(), 0);
        assert_eq!(map.values().count(), 0);

        let set = RandSet::<i32>::default();
        assert_eq!(set.iter().count(), 0);
    }

    #[test]
    fn single_element() {
        let map: RandMap<i32, i32> = [(42, 100)].into_iter().collect();
        let items: Vec<_> = map.iter().collect();
        assert_eq!(items, [(&42, &100)]);

        let set: RandSet<i32> = [42].into_iter().collect();
        let items: Vec<_> = set.iter().collect();
        assert_eq!(items, [&42]);
    }

    #[test]
    fn from_iterator_and_extend() {
        let mut map: RandMap<i32, i32> = [(1, 10), (2, 20)].into_iter().collect();
        map.extend([(3, 30), (4, 40)]);
        assert_eq!(map.len(), 4);

        let mut set: RandSet<i32> = [1, 2].into_iter().collect();
        set.extend([3, 4]);
        assert_eq!(set.len(), 4);
    }

    #[test]
    fn exact_size_iterator() {
        let map: RandMap<i32, i32> = (0..10).map(|i| (i, i)).collect();
        let iter = map.iter();
        assert_eq!(iter.len(), 10);

        let set: RandSet<i32> = (0..10).collect();
        let iter = set.iter();
        assert_eq!(iter.len(), 10);
    }

    #[test]
    fn gcd_correctness() {
        assert_eq!(gcd(12, 8), 4);
        assert_eq!(gcd(7, 13), 1);
        assert_eq!(gcd(100, 10), 10);
        assert_eq!(gcd(1, 1), 1);
        assert_eq!(gcd(17, 0), 17);
        assert_eq!(gcd(0, 5), 5);
    }

    #[test]
    fn rand_start_stride_produces_full_permutation() {
        for len in [2, 3, 4, 5, 7, 8, 10, 16, 17, 32, 100] {
            let (start, stride) = rand_start_stride(len);
            assert_eq!(gcd(stride, len), 1, "stride {stride} not coprime to {len}");

            let mut visited = vec![false; len];
            let mut idx = start;
            for _ in 0..len {
                visited[idx] = true;
                idx = (idx + stride) % len;
            }
            assert!(visited.iter().all(|&v| v), "stride {stride} didn't visit all of {len}");
        }
    }

    #[test]
    fn equality() {
        let a: RandMap<i32, i32> = [(1, 10), (2, 20)].into_iter().collect();
        let b: RandMap<i32, i32> = [(2, 20), (1, 10)].into_iter().collect();
        assert_eq!(a, b);
    }

    #[test]
    fn index_map_roundtrip() {
        let map: RandMap<i32, i32> = [(1, 10)].into_iter().collect();
        let idx: IndexMap<i32, i32, _> = map.into();
        let map2: RandMap<i32, i32, _> = idx.into();
        assert_eq!(map2.get(&1), Some(&10));
    }

    #[test]
    fn index_mut() {
        let mut map: RandMap<&str, i32> = [("a", 1), ("b", 2)].into_iter().collect();
        map["a"] = 42;
        assert_eq!(map.get("a"), Some(&42));
    }

    #[test]
    fn iter_mut_visits_all() {
        let mut map: RandMap<i32, i32> = (0..10).map(|i| (i, i)).collect();
        for (_, v) in map.iter_mut() {
            *v *= 10;
        }
        for i in 0..10 {
            assert_eq!(map.get(&i), Some(&(i * 10)));
        }
    }

    #[test]
    fn values_mut_visits_all() {
        let mut map: RandMap<i32, i32> = (0..5).map(|i| (i, 0)).collect();
        for v in map.values_mut() {
            *v = 99;
        }
        assert!(map.values().all(|v| *v == 99));
    }

    #[test]
    fn into_iter_no_extra_alloc() {
        let map: RandMap<i32, i32> = (0..100).map(|i| (i, i * 10)).collect();
        let mut keys: Vec<i32> = map.into_iter().map(|(k, _)| k).collect();
        keys.sort();
        let expected: Vec<i32> = (0..100).collect();
        assert_eq!(keys, expected);
    }

    #[test]
    fn set_into_iter_no_extra_alloc() {
        let set: RandSet<i32> = (0..100).collect();
        let mut elements: Vec<i32> = set.into_iter().collect();
        elements.sort();
        let expected: Vec<i32> = (0..100).collect();
        assert_eq!(elements, expected);
    }

    #[test]
    fn into_iter_exact_size() {
        let map: RandMap<i32, i32> = (0..10).map(|i| (i, i)).collect();
        let mut iter = map.into_iter();
        assert_eq!(iter.len(), 10);
        iter.next();
        assert_eq!(iter.len(), 9);

        let set: RandSet<i32> = (0..10).collect();
        let mut iter = set.into_iter();
        assert_eq!(iter.len(), 10);
        iter.next();
        assert_eq!(iter.len(), 9);
    }

    #[test]
    fn with_capacity() {
        let map: RandMap<i32, i32> = RandMap::with_capacity(100);
        assert!(map.is_empty());

        let set: RandSet<i32> = RandSet::with_capacity(100);
        assert!(set.is_empty());
    }

    #[test]
    fn into_iter_randomized() {
        let map: RandMap<i32, i32> = (0..20).map(|i| (i, i)).collect();
        let order1: Vec<i32> = map.clone().into_iter().map(|(k, _)| k).collect();
        let mut any_differ = false;
        for _ in 0..10 {
            let order_n: Vec<i32> = map.clone().into_iter().map(|(k, _)| k).collect();
            if order_n != order1 {
                any_differ = true;
                break;
            }
        }
        assert!(any_differ, "into_iter order should differ between calls");
    }

    #[test]
    fn into_iter_drop_partial() {
        let map: RandMap<i32, i32> = (0..100).map(|i| (i, i)).collect();
        let mut iter = map.into_iter();
        for _ in 0..50 {
            iter.next();
        }
        drop(iter);
    }

    #[test]
    fn entry_or_default() {
        let mut map: RandMap<i32, RandMap<i32, i32>> = RandMap::default();

        map.entry(1).or_default().insert(10, 100);
        assert_eq!(map.get(&1).unwrap().get(&10), Some(&100));

        map.entry(1).or_default().insert(20, 200);
        assert_eq!(map.get(&1).unwrap().len(), 2);
        assert_eq!(map.get(&1).unwrap().get(&20), Some(&200));
    }

    #[test]
    fn entry_or_insert() {
        let mut map: RandMap<&str, i32> = RandMap::default();
        map.entry("a").or_insert(42);
        assert_eq!(map["a"], 42);

        map.entry("a").or_insert(99);
        assert_eq!(map["a"], 42);
    }

    #[test]
    fn map_shift_remove() {
        let mut map: RandMap<i32, i32> = (0..5).map(|i| (i, i * 10)).collect();

        assert_eq!(map.shift_remove(&2), Some(20));
        assert_eq!(map.len(), 4);
        assert!(!map.contains_key(&2));

        assert_eq!(map.shift_remove(&2), None);
        assert_eq!(map.len(), 4);

        for i in [0, 1, 3, 4] {
            assert_eq!(map.get(&i), Some(&(i * 10)));
        }
    }

    #[test]
    fn map_shift_remove_entry() {
        let mut map: RandMap<&str, i32> = [("a", 1), ("b", 2), ("c", 3)].into_iter().collect();

        assert_eq!(map.shift_remove_entry("b"), Some(("b", 2)));
        assert_eq!(map.len(), 2);
        assert!(!map.contains_key("b"));

        assert_eq!(map.shift_remove_entry("b"), None);
    }

    #[test]
    fn set_shift_remove() {
        let mut set: RandSet<i32> = (0..5).collect();

        assert!(set.shift_remove(&3));
        assert_eq!(set.len(), 4);
        assert!(!set.contains(&3));

        assert!(!set.shift_remove(&3));
        assert_eq!(set.len(), 4);

        for i in [0, 1, 2, 4] {
            assert!(set.contains(&i));
        }
    }
}
