//! A length-limited LRU map tailored to reth's caches.
//!
//! Entries live in a `Vec` of nodes that form a doubly linked list from most to least recently
//! used. A [`HashTable`] maps keys to node indices, so hashbrown can grow or rehash it freely
//! without invalidating the links. Nodes freed by removals are reused by later inserts, and the
//! node `Vec` never grows past the length limit.
//!
//! Insertions that evict return the evicted entry, so callers that need eviction feedback don't
//! need a second lookup.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![no_std]

extern crate alloc;

use alloc::vec::Vec;
use alloy_primitives::map::DefaultHashBuilder;
use core::{
    borrow::Borrow,
    fmt,
    hash::{BuildHasher, Hash},
    iter::FusedIterator,
    mem::{self, MaybeUninit},
};
use hashbrown::HashTable;

/// Marks the end of the list and an empty free list.
const NONE: u32 = u32::MAX;

/// Hash table capacity per node slot.
///
/// The table holds only `u32` indices, so a sparse table costs a few bytes per entry and keeps
/// probe sequences short and tombstone rehashes rare under eviction churn.
const TABLE_SLACK: usize = 2;

/// A hash map holding at most `max_length` entries that evicts the least recently used entry.
///
/// [`get`](Self::get) and the insert methods mark an entry as most recently used, the `peek`
/// methods don't.
pub struct LruMap<K, V, S = DefaultHashBuilder> {
    /// Indices of the linked nodes, keyed by the hash of their key.
    table: HashTable<u32>,
    nodes: Vec<Node<K, V>>,
    hasher: S,
    max_length: u32,
    len: u32,
    newest: u32,
    oldest: u32,
    /// Head of the vacant nodes, chained through [`Node::newer`].
    free: u32,
}

impl<K, V> LruMap<K, V> {
    /// Creates an empty map that holds at most `max_length` entries, using the default hasher.
    /// Does not allocate.
    pub fn new(max_length: u32) -> Self {
        Self::with_hasher(max_length, DefaultHashBuilder::default())
    }
}

impl<K, V, S> LruMap<K, V, S> {
    /// Creates an empty map that holds at most `max_length` entries, using `hasher` to hash keys.
    /// Does not allocate.
    pub const fn with_hasher(max_length: u32, hasher: S) -> Self {
        Self {
            table: HashTable::new(),
            nodes: Vec::new(),
            hasher,
            max_length,
            len: 0,
            newest: NONE,
            oldest: NONE,
            free: NONE,
        }
    }

    /// Returns the maximum number of entries.
    pub const fn max_length(&self) -> u32 {
        self.max_length
    }

    /// Returns the number of entries.
    pub const fn len(&self) -> usize {
        self.len as usize
    }

    /// Returns `true` if the map holds no entries.
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Returns the number of heap bytes allocated by the map, excluding heap memory owned by keys
    /// and values.
    pub fn memory_usage(&self) -> usize {
        self.table.allocation_size() + self.nodes.capacity() * size_of::<Node<K, V>>()
    }

    /// Returns the least recently used entry without changing its position.
    pub fn peek_oldest(&self) -> Option<(&K, &V)> {
        (self.oldest != NONE).then(|| self.entry(self.oldest))
    }

    /// Returns the most recently used entry without changing its position.
    pub fn peek_newest(&self) -> Option<(&K, &V)> {
        (self.newest != NONE).then(|| self.entry(self.newest))
    }

    /// Iterates from the most to the least recently used entry.
    pub const fn iter(&self) -> Iter<'_, K, V, S> {
        Iter { map: self, next: self.newest, remaining: self.len() }
    }

    /// Removes all entries, keeping the allocated memory.
    pub fn clear(&mut self) {
        self.table.clear();
        let newest = mem::replace(&mut self.newest, NONE);
        self.oldest = NONE;
        self.free = NONE;
        self.len = 0;
        // The map is empty before any entry is dropped, so a panicking drop only leaks the rest.
        let mut nodes = mem::take(&mut self.nodes);
        drop_linked(&mut nodes, newest);
        nodes.clear();
        self.nodes = nodes;
    }

    #[inline]
    fn node(&self, index: u32) -> &Node<K, V> {
        debug_assert!((index as usize) < self.nodes.len());
        // SAFETY: indices stored in the table, the list and the free list are always in bounds.
        unsafe { self.nodes.get_unchecked(index as usize) }
    }

    #[inline]
    fn node_mut(&mut self, index: u32) -> &mut Node<K, V> {
        debug_assert!((index as usize) < self.nodes.len());
        // SAFETY: see `node`.
        unsafe { self.nodes.get_unchecked_mut(index as usize) }
    }

    /// Returns the entry of the linked node at `index`.
    #[inline]
    fn entry(&self, index: u32) -> (&K, &V) {
        let node = self.node(index);
        // SAFETY: linked nodes are initialized.
        unsafe { (node.key.assume_init_ref(), node.value.assume_init_ref()) }
    }

    #[inline]
    fn unlink(&mut self, index: u32) {
        let Node { older, newer, .. } = *self.node(index);
        if older == NONE {
            self.oldest = newer
        } else {
            self.node_mut(older).newer = newer
        }
        if newer == NONE {
            self.newest = older
        } else {
            self.node_mut(newer).older = older
        }
    }

    #[inline]
    fn link_newest(&mut self, index: u32) {
        let prev = self.newest;
        let node = self.node_mut(index);
        node.older = prev;
        node.newer = NONE;
        if prev == NONE {
            self.oldest = index
        } else {
            self.node_mut(prev).newer = index
        }
        self.newest = index;
    }

    #[inline]
    fn promote(&mut self, index: u32) {
        if self.newest != index {
            self.unlink(index);
            self.link_newest(index);
        }
    }
}

impl<K, V, S> LruMap<K, V, S>
where
    K: Hash + Eq,
    S: BuildHasher,
{
    /// Returns the value for `key` and marks it as the most recently used.
    #[inline]
    pub fn get<Q>(&mut self, key: &Q) -> Option<&mut V>
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        let index = self.find(key)?;
        self.promote(index);
        // SAFETY: `find` only returns linked nodes, which are initialized.
        Some(unsafe { self.node_mut(index).value.assume_init_mut() })
    }

    /// Returns the value for `key` without changing its position.
    #[inline]
    pub fn peek<Q>(&self, key: &Q) -> Option<&V>
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        self.find(key).map(|index| self.entry(index).1)
    }

    /// Returns a mutable reference to the value for `key` without changing its position.
    #[inline]
    pub fn peek_mut<Q>(&mut self, key: &Q) -> Option<&mut V>
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        let index = self.find(key)?;
        // SAFETY: `find` only returns linked nodes, which are initialized.
        Some(unsafe { self.node_mut(index).value.assume_init_mut() })
    }

    /// Returns `true` if the map contains `key`, without changing its position.
    #[inline]
    pub fn contains<Q>(&self, key: &Q) -> bool
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        self.find(key).is_some()
    }

    /// Inserts `value` for `key`, or replaces the current value, and marks `key` as the most
    /// recently used.
    ///
    /// Returns the least recently used entry if it was evicted to make room. If `max_length` is
    /// zero, nothing is stored and the given entry is returned instead.
    #[inline]
    pub fn insert(&mut self, key: K, value: V) -> Option<(K, V)> {
        let hash = self.hasher.hash_one(&key);
        if let Some(index) = self.find_hashed(hash, &key) {
            self.promote(index);
            // SAFETY: `find_hashed` only returns linked nodes, which are initialized.
            *unsafe { self.node_mut(index).value.assume_init_mut() } = value;
            return None
        }
        self.insert_new(hash, key, value).1
    }

    /// Returns the value for `key` and marks it as the most recently used, first inserting the
    /// value returned by `f` if `key` is absent.
    ///
    /// Returns `None` only if `max_length` is zero.
    #[inline]
    pub fn get_or_insert(&mut self, key: K, f: impl FnOnce() -> V) -> Option<&mut V> {
        let hash = self.hasher.hash_one(&key);
        let index = if let Some(index) = self.find_hashed(hash, &key) {
            self.promote(index);
            index
        } else {
            let index = self.insert_new(hash, key, f()).0;
            if index == NONE {
                return None
            }
            index
        };
        // SAFETY: both branches yield a linked node, which is initialized.
        Some(unsafe { self.node_mut(index).value.assume_init_mut() })
    }

    /// Removes `key` and returns its value.
    #[inline]
    pub fn remove<Q>(&mut self, key: &Q) -> Option<V>
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        let index = self.find(key)?;
        Some(self.take(index).1)
    }

    /// Removes and returns the least recently used entry.
    #[inline]
    pub fn pop_oldest(&mut self) -> Option<(K, V)> {
        (self.oldest != NONE).then(|| self.take(self.oldest))
    }

    #[inline]
    fn find<Q>(&self, key: &Q) -> Option<u32>
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        self.find_hashed(self.hasher.hash_one(key), key)
    }

    #[inline]
    fn find_hashed<Q>(&self, hash: u64, key: &Q) -> Option<u32>
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        let nodes = &self.nodes;
        self.table
            .find(hash, |&index| {
                // SAFETY: the table only holds indices of linked nodes, which are initialized.
                let node = unsafe { nodes.get_unchecked(index as usize) };
                unsafe { node.key.assume_init_ref() }.borrow() == key
            })
            .copied()
    }

    /// Removes the linked node at `index` from the table, list and length, moves its entry out
    /// and puts the node on the free list.
    fn take(&mut self, index: u32) -> (K, V) {
        // Hashing may panic, so it happens before any state changes.
        // SAFETY: the node is linked, so its key is initialized.
        let hash = self.hasher.hash_one(unsafe { self.node(index).key.assume_init_ref() });
        match self.table.find_entry(hash, |&i| i == index) {
            Ok(slot) => {
                slot.remove();
            }
            // Only reachable with a `Hash` impl that is not deterministic. The table must never
            // point at a vacant node, so fall back to a full scan.
            Err(_) => self.table.retain(|&mut i| i != index),
        }
        self.unlink(index);
        let free = self.free;
        let node = self.node_mut(index);
        node.newer = free;
        // SAFETY: the node was linked, so both are initialized. It is vacant from here on.
        let entry = unsafe { (node.key.assume_init_read(), node.value.assume_init_read()) };
        self.free = index;
        self.len -= 1;
        entry
    }

    /// Inserts an absent key, evicting the oldest entry if the map is full.
    ///
    /// Returns the index of the new node, or [`NONE`] with the given entry if `max_length` is
    /// zero, and the evicted entry.
    ///
    /// Every linked node is initialized and counted in `len` whenever user code (hashing,
    /// equality, drops) can panic. The table may then miss an index, which only causes lookup
    /// misses, but never holds the index of a vacant node.
    fn insert_new(&mut self, hash: u64, key: K, value: V) -> (u32, Option<(K, V)>) {
        if self.max_length == 0 {
            return (NONE, Some((key, value)))
        }
        let evicted = (self.len >= self.max_length).then(|| self.take(self.oldest));
        let index = if self.free == NONE {
            if self.nodes.len() == self.nodes.capacity() {
                self.grow();
            }
            self.nodes.push(Node {
                key: MaybeUninit::new(key),
                value: MaybeUninit::new(value),
                older: NONE,
                newer: NONE,
            });
            self.nodes.len() as u32 - 1
        } else {
            let index = self.free;
            let node = self.node_mut(index);
            let next = node.newer;
            node.key.write(key);
            node.value.write(value);
            self.free = next;
            index
        };
        self.link_newest(index);
        self.len += 1;
        let (nodes, hasher) = (&self.nodes, &self.hasher);
        self.table.insert_unique(hash, index, |&i| hash_node(hasher, nodes, i));
        (index, evicted)
    }

    /// Grows the node `Vec` geometrically up to `max_length` and keeps the table sparse.
    #[cold]
    fn grow(&mut self) {
        let target =
            (self.nodes.len() * 2).clamp(4.min(self.max_length as usize), self.max_length as usize);
        self.nodes.reserve_exact(target - self.nodes.len());
        let (nodes, hasher) = (&self.nodes, &self.hasher);
        let additional = target.saturating_mul(TABLE_SLACK).saturating_sub(self.table.len());
        self.table.reserve(additional, |&i| hash_node(hasher, nodes, i));
    }
}

impl<K, V, S: Default> Default for LruMap<K, V, S> {
    /// Returns an empty map with `max_length` zero, which stores nothing.
    fn default() -> Self {
        Self::with_hasher(0, S::default())
    }
}

impl<K, V, S> Drop for LruMap<K, V, S> {
    fn drop(&mut self) {
        drop_linked(&mut self.nodes, self.newest);
    }
}

impl<K, V, S> fmt::Debug for LruMap<K, V, S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LruMap")
            .field("len", &self.len)
            .field("max_length", &self.max_length)
            .finish_non_exhaustive()
    }
}

impl<'a, K, V, S> IntoIterator for &'a LruMap<K, V, S> {
    type Item = (&'a K, &'a V);
    type IntoIter = Iter<'a, K, V, S>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

/// Iterator over the entries of an [`LruMap`], from most to least recently used.
#[derive(Debug)]
pub struct Iter<'a, K, V, S> {
    map: &'a LruMap<K, V, S>,
    next: u32,
    remaining: usize,
}

impl<'a, K, V, S> Iterator for Iter<'a, K, V, S> {
    type Item = (&'a K, &'a V);

    fn next(&mut self) -> Option<Self::Item> {
        if self.next == NONE {
            return None
        }
        let map = self.map;
        let entry = map.entry(self.next);
        self.next = map.node(self.next).older;
        self.remaining -= 1;
        Some(entry)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.remaining, Some(self.remaining))
    }
}

impl<K, V, S> ExactSizeIterator for Iter<'_, K, V, S> {}

impl<K, V, S> FusedIterator for Iter<'_, K, V, S> {}

/// A list node. `key` and `value` are initialized iff the node is linked into the LRU list.
struct Node<K, V> {
    key: MaybeUninit<K>,
    value: MaybeUninit<V>,
    /// Next less recently used node, or [`NONE`].
    older: u32,
    /// Next more recently used node, or the next vacant node for vacant nodes, or [`NONE`].
    newer: u32,
}

/// Drops the entries of the list starting at `newest`.
///
/// Callers must not treat these nodes as linked afterwards.
fn drop_linked<K, V>(nodes: &mut [Node<K, V>], newest: u32) {
    if !mem::needs_drop::<(K, V)>() {
        return
    }
    let mut index = newest;
    while index != NONE {
        let node = &mut nodes[index as usize];
        index = node.older;
        // SAFETY: linked nodes are initialized, and the caller stops treating them as linked, so
        // each is dropped exactly once.
        unsafe {
            node.key.assume_init_drop();
            node.value.assume_init_drop();
        }
    }
}

/// Hashes the key of the linked node at `index`, for rehashing the table.
#[inline]
fn hash_node<K: Hash, V, S: BuildHasher>(hasher: &S, nodes: &[Node<K, V>], index: u32) -> u64 {
    // SAFETY: the table only holds indices of linked nodes, which are initialized.
    hasher.hash_one(unsafe { nodes.get_unchecked(index as usize).key.assume_init_ref() })
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::{
        string::{String, ToString},
        sync::Arc,
        vec,
    };

    fn keys<K: Copy, V, S>(map: &LruMap<K, V, S>) -> Vec<K> {
        map.iter().map(|(k, _)| *k).collect()
    }

    #[test]
    fn evicts_least_recently_used() {
        let mut map = LruMap::<u64, u64>::new(3);
        assert_eq!(map.insert(1, 10), None);
        assert_eq!(map.insert(2, 20), None);
        assert_eq!(map.insert(3, 30), None);
        assert_eq!(map.get(&1), Some(&mut 10));
        assert_eq!(map.peek(&2), Some(&20));
        assert_eq!(map.insert(4, 40), Some((2, 20)));
        assert_eq!(keys(&map), [4, 1, 3]);
        assert_eq!(map.peek_oldest(), Some((&3, &30)));
        assert_eq!(map.peek_newest(), Some((&4, &40)));
        assert_eq!(map.insert(3, 31), None);
        assert_eq!(keys(&map), [3, 4, 1]);
        assert_eq!(map.remove(&4), Some(40));
        assert_eq!(map.get_or_insert(5, || 50), Some(&mut 50));
        assert_eq!(map.get_or_insert(1, || 0), Some(&mut 10));
        assert_eq!(keys(&map), [1, 5, 3]);
        assert_eq!(map.pop_oldest(), Some((3, 31)));
        assert_eq!(map.len(), 2);
        assert_eq!(map.iter().len(), 2);
    }

    #[test]
    fn zero_length_stores_nothing() {
        let mut map = LruMap::<u64, u64>::default();
        assert_eq!(map.insert(1, 1), Some((1, 1)));
        assert_eq!(map.get_or_insert(1, || 1), None);
        assert!(map.is_empty());
        assert_eq!(map.memory_usage(), 0);
    }

    #[test]
    fn borrowed_lookups() {
        let mut map = LruMap::<String, u64>::new(2);
        map.insert("a".to_string(), 1);
        assert_eq!(map.get("a"), Some(&mut 1));
        assert!(map.contains("a"));
        assert_eq!(map.peek_mut("a"), Some(&mut 1));
        assert_eq!(map.remove("a"), Some(1));
        assert!(!map.contains("a"));
    }

    #[test]
    fn never_grows_past_max_length() {
        let mut map = LruMap::<u64, ()>::new(5);
        for i in 0..100 {
            map.insert(i, ());
            if i % 7 == 6 {
                map.remove(&(i - 1));
            }
        }
        assert_eq!(map.nodes.capacity(), 5);
        assert_eq!(map.len(), 5);
    }

    /// Every value is dropped exactly once across evictions, removals, replacements, clears and
    /// drop.
    #[test]
    fn drops_each_value_once() {
        let token = Arc::new(());
        {
            let mut map = LruMap::<u64, Arc<()>>::new(4);
            for i in 0..64u64 {
                map.insert(i % 9, Arc::clone(&token));
                if i % 3 == 0 {
                    map.remove(&(i % 7));
                }
                if i % 5 == 0 {
                    map.pop_oldest();
                }
                map.get_or_insert(i % 11, || Arc::clone(&token));
                assert_eq!(Arc::strong_count(&token), 1 + map.len());
                if i == 32 {
                    map.clear();
                    assert_eq!(Arc::strong_count(&token), 1);
                }
            }
        }
        assert_eq!(Arc::strong_count(&token), 1);
    }

    /// Replays a random trace against a `Vec` model, newest first.
    #[test]
    fn matches_model() {
        let steps = if cfg!(miri) { 2_000 } else { 100_000 };
        for max_length in [1u32, 2, 3, 7, 64, 333] {
            let mut map = LruMap::<u64, u64>::new(max_length);
            let mut model: Vec<(u64, u64)> = vec![];
            let mut x = 0x9e37_79b9_7f4a_7c15u64;
            for step in 0..steps {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                let key = x % (max_length as u64 * 3 + 1);
                let position = model.iter().position(|e| e.0 == key);
                match x >> 61 {
                    0..=2 => {
                        let expected = match position {
                            Some(p) => {
                                model.remove(p);
                                None
                            }
                            None if model.len() == max_length as usize => model.pop(),
                            None => None,
                        };
                        model.insert(0, (key, step));
                        assert_eq!(map.insert(key, step), expected);
                    }
                    3 | 4 => {
                        let expected = position.map(|p| {
                            let entry = model.remove(p);
                            model.insert(0, entry);
                            entry.1
                        });
                        assert_eq!(map.get(&key).copied(), expected);
                    }
                    5 => assert_eq!(map.remove(&key), position.map(|p| model.remove(p).1)),
                    6 => assert_eq!(map.pop_oldest(), model.pop()),
                    _ => assert_eq!(map.peek(&key).copied(), position.map(|p| model[p].1)),
                }
                assert_eq!(map.len(), model.len());
                if step % 997 == 0 {
                    assert_eq!(map.iter().map(|(k, v)| (*k, *v)).collect::<Vec<_>>(), model);
                }
            }
        }
    }
}
