//! Randomized-iteration map wrapper around [`IndexMap`].
//!
//! This type delegates all map operations to the underlying `indexmap` type, but
//! randomizes iteration order on every `.iter()` call using a coprime-stride permutation.
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

use std::{
    collections::hash_map::RandomState,
    fmt::{Debug, Formatter, Result},
    hash::{BuildHasher, Hash},
    iter::FusedIterator,
    ops::{Index, IndexMut},
};

use indexmap::{map::Entry, Equivalent, IndexMap};
use rand::Rng;

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

/// An iterator that visits every entry of an [`IndexMap`] exactly once in a
/// pseudorandom order determined by a coprime-stride permutation.
pub struct RandIter<'a, K, V, S = RandomState> {
    map: &'a IndexMap<K, V, S>,
    current: usize,
    stride: usize,
    remaining: usize,
}

impl<K: Debug, V: Debug, S> Debug for RandIter<'_, K, V, S> {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
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

/// A mutable iterator that visits every entry of an [`IndexMap`] exactly once in a
/// pseudorandom order determined by a coprime-stride permutation.
///
/// Keys are yielded by shared reference since they are part of the map's index structure.
/// Values are yielded by mutable reference.
pub struct RandIterMut<'a, K, V, S = RandomState> {
    map: &'a mut IndexMap<K, V, S>,
    indices: Vec<usize>,
    pos: usize,
}

impl<K: Debug, V: Debug, S> Debug for RandIterMut<'_, K, V, S> {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
        f.debug_struct("RandIterMut")
            .field("remaining", &(self.indices.len() - self.pos))
            .finish_non_exhaustive()
    }
}

impl<'a, K, V, S> Iterator for RandIterMut<'a, K, V, S> {
    type Item = (&'a K, &'a mut V);

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        let &idx = self.indices.get(self.pos)?;
        self.pos += 1;
        // SAFETY: Each index in the coprime-stride walk appears exactly once, so we
        // never yield two mutable references to the same entry. We reborrow through
        // raw pointers to decouple the returned lifetime from `&mut self`.
        let (k, v) = self.map.get_index_mut(idx)?;
        unsafe { Some((&*(k as *const K), &mut *(v as *mut V))) }
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = self.indices.len() - self.pos;
        (remaining, Some(remaining))
    }
}

impl<K, V, S> ExactSizeIterator for RandIterMut<'_, K, V, S> {}
impl<K, V, S> FusedIterator for RandIterMut<'_, K, V, S> {}

/// An iterator that visits every key of an [`IndexMap`] exactly once in random order.
pub struct RandKeys<'a, K, V, S = RandomState> {
    inner: RandIter<'a, K, V, S>,
}

impl<K: Debug, V: Debug, S> Debug for RandKeys<'_, K, V, S> {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
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

impl<K: Debug, V: Debug, S> Debug for RandValues<'_, K, V, S> {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
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

impl<K: Debug, V: Debug, S> Debug for RandIntoIter<K, V, S> {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
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

impl<K: Debug, V: Debug, S> Debug for RandMap<K, V, S> {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
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

    /// Returns a mutable iterator visiting all key-value pairs in **random order**.
    ///
    /// Keys are yielded by shared reference; values by mutable reference.
    /// Each call produces a different permutation.
    pub fn iter_mut(&mut self) -> RandIterMut<'_, K, V, S> {
        let len = self.inner.len();
        if len == 0 {
            return RandIterMut { map: &mut self.inner, indices: Vec::new(), pos: 0 };
        }
        let (start, stride) = rand_start_stride(len);
        let mut indices = Vec::with_capacity(len);
        let mut current = start;
        for _ in 0..len {
            indices.push(current);
            current = (current + stride) % len;
        }
        RandIterMut { map: &mut self.inner, indices, pos: 0 }
    }

    /// Returns an iterator visiting all keys in **random order**.
    pub fn keys(&self) -> RandKeys<'_, K, V, S> {
        RandKeys { inner: self.iter() }
    }

    /// Returns an iterator visiting all values in **random order**.
    pub fn values(&self) -> RandValues<'_, K, V, S> {
        RandValues { inner: self.iter() }
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
        Q: ?Sized + Hash + Equivalent<K>,
    {
        self.inner.get(key)
    }

    /// Returns a mutable reference to the value corresponding to the key.
    pub fn get_mut<Q>(&mut self, key: &Q) -> Option<&mut V>
    where
        Q: ?Sized + Hash + Equivalent<K>,
    {
        self.inner.get_mut(key)
    }

    /// Returns the key-value pair corresponding to the supplied key.
    pub fn get_key_value<Q>(&self, key: &Q) -> Option<(&K, &V)>
    where
        Q: ?Sized + Hash + Equivalent<K>,
    {
        self.inner.get_key_value(key)
    }

    /// Returns `true` if the map contains a value for the specified key.
    pub fn contains_key<Q>(&self, key: &Q) -> bool
    where
        Q: ?Sized + Hash + Equivalent<K>,
    {
        self.inner.contains_key(key)
    }

    /// Inserts a key-value pair into the map.
    pub fn insert(&mut self, key: K, value: V) -> Option<V> {
        self.inner.insert(key, value)
    }

    /// Gets the entry for the given key for in-place manipulation.
    pub fn entry(&mut self, key: K) -> Entry<'_, K, V> {
        self.inner.entry(key)
    }

    /// Removes a key from the map, returning the value if the key was previously in the
    /// map.
    ///
    /// This is equivalent to [`swap_remove`](Self::swap_remove). Because iteration order
    /// is randomized, the swap-remove reordering is unobservable.
    pub fn remove<Q>(&mut self, key: &Q) -> Option<V>
    where
        Q: ?Sized + Hash + Equivalent<K>,
    {
        self.swap_remove(key)
    }

    /// Removes a key from the map, returning the stored key and value if the
    /// key was previously in the map.
    pub fn remove_entry<Q>(&mut self, key: &Q) -> Option<(K, V)>
    where
        Q: ?Sized + Hash + Equivalent<K>,
    {
        self.swap_remove_entry(key)
    }

    /// Removes a key from the map using swap-remove.
    pub fn swap_remove<Q>(&mut self, key: &Q) -> Option<V>
    where
        Q: ?Sized + Hash + Equivalent<K>,
    {
        self.inner.swap_remove(key)
    }

    /// Removes a key from the map using swap-remove, returning both key and value.
    pub fn swap_remove_entry<Q>(&mut self, key: &Q) -> Option<(K, V)>
    where
        Q: ?Sized + Hash + Equivalent<K>,
    {
        self.inner.swap_remove_entry(key)
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
    Q: ?Sized + Hash + Equivalent<K>,
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
    Q: ?Sized + Hash + Equivalent<K>,
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

impl<'a, K, V, S> IntoIterator for &'a mut RandMap<K, V, S> {
    type Item = (&'a K, &'a mut V);
    type IntoIter = RandIterMut<'a, K, V, S>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter_mut()
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::{collections::BTreeSet, iter::once};

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
    fn into_iter_visits_all_elements() {
        let map: RandMap<i32, i32> = (0..50).map(|i| (i, i)).collect();
        let keys: BTreeSet<i32> = map.into_iter().map(|(k, _)| k).collect();
        let expected: BTreeSet<i32> = (0..50).collect();
        assert_eq!(keys, expected);
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
    fn empty_iteration() {
        let map = RandMap::<i32, i32>::default();
        assert_eq!(map.iter().count(), 0);
        assert_eq!(map.keys().count(), 0);
        assert_eq!(map.values().count(), 0);
    }

    #[test]
    fn single_element() {
        let map: RandMap<i32, i32> = once((42, 100)).collect();
        let items: Vec<_> = map.iter().collect();
        assert_eq!(items, [(&42, &100)]);
    }

    #[test]
    fn from_iterator_and_extend() {
        let mut map: RandMap<i32, i32> = [(1, 10), (2, 20)].into_iter().collect();
        assert_eq!(map.get(&1), Some(&10));
        assert_eq!(map.get(&2), Some(&20));

        map.extend([(3, 30), (4, 40)]);
        assert_eq!(map.len(), 4);
        assert_eq!(map.get(&3), Some(&30));
        assert_eq!(map.get(&4), Some(&40));
    }

    #[test]
    fn exact_size_iterator() {
        let map: RandMap<i32, i32> = (0..10).map(|i| (i, i)).collect();
        let mut iter = map.iter();
        assert_eq!(iter.len(), 10);
        iter.next();
        assert_eq!(iter.len(), 9);
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
        let map: RandMap<i32, i32> = once((1, 10)).collect();
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
    fn into_iter_no_extra_alloc() {
        let map: RandMap<i32, i32> = (0..100).map(|i| (i, i * 10)).collect();
        let mut keys: Vec<i32> = map.into_iter().map(|(k, _)| k).collect();
        keys.sort();
        let expected: Vec<i32> = (0..100).collect();
        assert_eq!(keys, expected);
    }

    #[test]
    fn into_iter_exact_size() {
        let map: RandMap<i32, i32> = (0..10).map(|i| (i, i)).collect();
        let mut iter = map.into_iter();
        assert_eq!(iter.len(), 10);
        iter.next();
        assert_eq!(iter.len(), 9);
    }

    #[test]
    fn with_capacity() {
        let map: RandMap<i32, i32> = RandMap::with_capacity(100);
        assert!(map.is_empty());
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
    fn get_mut() {
        let mut map: RandMap<&str, i32> = [("a", 1), ("b", 2)].into_iter().collect();

        *map.get_mut("a").unwrap() = 10;
        assert_eq!(map["a"], 10);
        assert!(map.get_mut("z").is_none());
    }

    #[test]
    fn retain() {
        let mut map: RandMap<i32, i32> = (0..10).map(|i| (i, i * 10)).collect();

        map.retain(|_, v| *v >= 50);
        assert_eq!(map.len(), 5);
        for i in 0..5 {
            assert!(!map.contains_key(&i));
        }
        for i in 5..10 {
            assert_eq!(map[&i], i * 10);
        }
    }

    #[test]
    fn iter_mut_visits_all_elements() {
        let mut map: RandMap<i32, i32> = (0..100).map(|i| (i, i * 10)).collect();
        let keys: BTreeSet<i32> = map.iter_mut().map(|(k, _)| *k).collect();
        let expected: BTreeSet<i32> = (0..100).collect();
        assert_eq!(keys, expected);
    }

    #[test]
    fn iter_mut_can_mutate_values() {
        let mut map: RandMap<i32, i32> = (0..10).map(|i| (i, i)).collect();

        for (_, v) in map.iter_mut() {
            *v *= 2;
        }

        for i in 0..10 {
            assert_eq!(map[&i], i * 2);
        }
    }

    #[test]
    fn iter_mut_randomized() {
        let mut map: RandMap<i32, i32> = (0..20).map(|i| (i, i)).collect();

        let order1: Vec<i32> = map.iter_mut().map(|(k, _)| *k).collect();
        let mut any_differ = false;
        for _ in 0..10 {
            let order_n: Vec<i32> = map.iter_mut().map(|(k, _)| *k).collect();
            if order_n != order1 {
                any_differ = true;
                break;
            }
        }
        assert!(any_differ, "iter_mut order should differ between calls");
    }

    #[test]
    fn iter_mut_exact_size() {
        let mut map: RandMap<i32, i32> = (0..10).map(|i| (i, i)).collect();
        let mut iter = map.iter_mut();
        assert_eq!(iter.len(), 10);
        iter.next();
        assert_eq!(iter.len(), 9);
    }

    #[test]
    fn iter_mut_empty() {
        let mut map = RandMap::<i32, i32>::default();
        assert_eq!(map.iter_mut().count(), 0);
    }

    #[test]
    fn iter_mut_single_element() {
        let mut map: RandMap<i32, i32> = once((42, 100)).collect();
        let items: Vec<_> = map.iter_mut().map(|(k, v)| (*k, *v)).collect();
        assert_eq!(items, [(42, 100)]);
    }

    #[test]
    fn into_iter_mut_ref() {
        let mut map: RandMap<i32, i32> = (0..10).map(|i| (i, i)).collect();
        for (_, v) in &mut map {
            *v += 100;
        }
        for i in 0..10 {
            assert_eq!(map[&i], i + 100);
        }
    }
}
