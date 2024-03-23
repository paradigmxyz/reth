use core::hash::BuildHasher;
use std::{
    collections::HashMap,
    fmt,
    fmt::Debug,
    hash::{DefaultHasher, Hash, Hasher},
    mem::replace,
};

use derive_more::{Deref, DerefMut};
use itertools::Itertools;
use schnellru::{ByLength, Limiter, RandomState};

type Index = usize;

const NULL: usize = usize::MAX;
const HEAD: usize = 0;
const TAIL: usize = 1;
const OFFSET: usize = 2;
const VEC_GROWTH_CAP: usize = 65536;

#[derive(Debug, Clone)]
struct Node {
    pub(crate) prev: Index,
    pub(crate) next: Index,
    pub(crate) data: u64,
}

impl Node {
    fn new(data: u64) -> Self {
        Self { prev: NULL, next: NULL, data }
    }
}

#[derive(Debug, Clone)]
struct Nodes {
    head: Node,
    tail: Node,
    nodes: Vec<Node>,
}

impl Nodes {
    fn with_capacity(capacity: usize) -> Self {
        Self {
            head: Node { prev: NULL, next: TAIL, data: 0 },
            tail: Node { prev: HEAD, next: NULL, data: 0 },
            nodes: Vec::with_capacity(capacity),
        }
    }

    fn new_node(&mut self, data: u64) -> Index {
        // vector doubles its capacity when it reaches the limit, which can cause memory waste.
        // Since the capacity is already large, double the capacity is not necessary.
        // So, we rework the expansion to 10% instead.
        if self.nodes.capacity() > VEC_GROWTH_CAP && self.nodes.capacity() - self.nodes.len() < 2 {
            self.nodes.reserve_exact(self.nodes.capacity() / 10);
        }
        self.nodes.push(Node::new(data));
        self.nodes.len() - 1 + OFFSET
    }

    #[allow(dead_code)]
    fn len(&self) -> usize {
        self.nodes.len()
    }

    #[allow(dead_code)]
    fn tail(&self) -> &Node {
        &self.tail
    }

    #[allow(dead_code)]
    fn head(&self) -> &Node {
        &self.head
    }
}

impl std::ops::Index<usize> for Nodes {
    type Output = Node;

    fn index(&self, index: usize) -> &Self::Output {
        match index {
            HEAD => &self.head,
            TAIL => &self.tail,
            _ => &self.nodes[index - OFFSET],
        }
    }
}

impl std::ops::IndexMut<usize> for Nodes {
    fn index_mut(&mut self, index: usize) -> &mut Self::Output {
        match index {
            HEAD => &mut self.head,
            TAIL => &mut self.tail,
            _ => &mut self.nodes[index - OFFSET],
        }
    }
}

/// Doubly linked list with a fixed capacity.
#[derive(Debug, Clone)]
pub struct LinkedList {
    nodes: Nodes,
    /// To keep track of free memory slots to reuse.
    free: Vec<Index>,
}

impl LinkedList {
    /// Creates a new linked list with the given capacity.
    pub fn with_capacity(capacity: usize) -> Self {
        Self { nodes: Nodes::with_capacity(capacity), free: vec![] }
    }

    /// Insert a new node at the head of the list.
    pub fn push_head(&mut self, data: u64) -> Index {
        let node_index = self.new_node(data);
        self.insert_after(node_index, HEAD);
        node_index
    }

    /// Remove the node at the tail and return its data.
    pub fn pop_tail(&mut self) -> Option<u64> {
        self.remove(self.nodes.tail.prev)
    }

    /// Remove the node at `index` and return its data.
    ///
    /// @notice: If data is not removed, it can cause memory leaks.
    pub fn remove(&mut self, index: Index) -> Option<u64> {
        assert!(index != HEAD && index != TAIL);

        let node = &mut self.nodes[index];
        // clear the nodes
        let prev = replace(&mut node.prev, NULL);
        let next = replace(&mut node.next, NULL);
        let data = node.data;
        self.nodes[prev].next = next;
        self.nodes[next].prev = prev;
        self.free.push(index);
        Some(data)
    }

    /// Creates a new node with the given data.
    ///
    /// If there are free slots, the node will reuse one of them.
    fn new_node(&mut self, data: u64) -> Index {
        if let Some(index) = self.free.pop() {
            self.nodes[index].data = data;
            index
        } else {
            self.nodes.new_node(data)
        }
    }

    /// Insert the `node_index` after the node at `at`.
    fn insert_after(&mut self, node_index: Index, at: Index) {
        assert!(at != TAIL && node_index != at);

        let next = replace(&mut self.nodes[at].next, node_index);
        let node = &mut self.nodes[node_index];
        node.prev = at;
        node.next = next;
        self.nodes[next].prev = node_index;
    }

    fn next(&self, index: Index) -> Index {
        self.nodes[index].next
    }

    #[allow(dead_code)]
    fn prev(&self, index: Index) -> Index {
        self.nodes[index].prev
    }
}

#[derive(Debug, Clone)]
pub struct LruNode<T> {
    data: T,
    #[allow(dead_code)]
    list_index: usize,
}

// notice: least recently used is at the tail
#[derive(Debug, Clone)]
pub struct LruCache<T>
where
    T: Hash,
{
    lookup_table: HashMap<u64, Box<LruNode<T>>>,
    order: LinkedList,
    capacity: usize,
}

impl<T> LruCache<T>
where
    T: Hash,
{
    /// Creates a new LRU cache with the given capacity.
    pub fn new(capacity: usize) -> Self {
        Self::with_capacity(capacity)
    }

    /// Creates a new LRU cache with the given capacity.
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            lookup_table: HashMap::with_capacity(capacity),
            order: LinkedList::with_capacity(capacity),
            capacity,
        }
    }

    /// Inserts a data into the cache.
    ///
    /// If capacity is reached, the given length will be enforced
    /// and the oldest element will be removed.
    ///
    /// Returns `true` if the data was inserted, `false` if the data already existed.
    pub fn insert(&mut self, data: T) -> bool {
        let (success, _evicted) = self.insert_and_get_evicted(data);
        success
    }

    /// Same as [`Self::insert`] but returns a tuple, where the second index is the evicted data,
    /// if one was evicted.
    pub fn insert_and_get_evicted(&mut self, data: T) -> (bool, Option<T>) {
        // Check if the data already exists
        let key = Self::key_from_data(&data);
        if self.lookup_table.contains_key(&key) {
            return (false, None);
        }

        let mut old_val = None;
        if self.lookup_table.len() >= self.capacity {
            old_val = self.remove_lru();
        }

        // insert
        let list_index = self.order.push_head(key);
        self.lookup_table.insert(key, Box::new(LruNode { data, list_index }));
        (true, old_val)
    }

    /// Returns `true` if the cache contains the given data.
    pub fn contains(&self, data: &T) -> bool {
        let key = Self::key_from_data(data);
        self.lookup_table.contains_key(&key)
    }

    fn key_from_data(data: &T) -> u64 {
        let mut hasher = DefaultHasher::new();
        data.hash(&mut hasher);
        hasher.finish()
    }

    /// Removes data from the cache.
    pub fn remove(&mut self, data: &T) -> bool {
        let key = Self::key_from_data(data);
        let node = self.lookup_table.remove(&key);
        node.is_some()
    }

    /// Removes the least recently used item from the cache and returns it.
    /// @notice: This function doesn't check if the cache is empty.
    fn remove_lru(&mut self) -> Option<T> {
        let key = self.order.pop_tail()?;
        let node = self.lookup_table.remove(&key)?;
        Some(node.data)
    }

    /// Returns the number of elements in the cache.
    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.lookup_table.len()
    }

    /// Returns `true` if the cache is empty.
    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.lookup_table.is_empty()
    }

    /// Returns an iterator over all cached entries in LRU order.
    pub fn iter(&self) -> impl Iterator<Item = &T> {
        // loop through the linked list
        if self.lookup_table.is_empty() {
            return vec![].into_iter();
        }

        let mut res = vec![];
        let mut node_index = self.order.next(HEAD);
        while node_index != TAIL {
            let k = self.order.nodes[node_index].data;
            if let Some(data) = self.lookup_table.get(&k) {
                res.push(&data.data);
            }
            node_index = self.order.next(node_index);
        }
        res.into_iter()
    }
}

impl<T> Extend<T> for LruCache<T>
where
    T: Hash,
{
    fn extend<I: IntoIterator<Item = T>>(&mut self, iter: I) {
        for data in iter {
            self.insert(data);
        }
    }
}

/// Wrapper of [`schnellru::LruMap`] that implements [`Debug`].
#[derive(Deref, DerefMut, Default)]
pub struct LruMap<K, V, L = ByLength, S = RandomState>(schnellru::LruMap<K, V, L, S>)
where
    K: Hash + PartialEq,
    L: Limiter<K, V>,
    S: BuildHasher;

impl<K, V, L, S> Debug for LruMap<K, V, L, S>
where
    K: Hash + PartialEq + fmt::Display,
    V: Debug,
    L: Limiter<K, V> + Debug,
    S: BuildHasher,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut debug_struct = f.debug_struct("LruMap");

        debug_struct.field("limiter", self.limiter());

        debug_struct.field(
            "res_fn_iter",
            &format_args!(
                "Iter: {{{} }}",
                self.iter().map(|(k, v)| format!(" {k}: {v:?}")).format(",")
            ),
        );

        debug_struct.finish()
    }
}

impl<K, V> LruMap<K, V>
where
    K: Hash + PartialEq,
{
    /// Returns a new cache with default limiter and hash builder.
    pub fn new(max_length: u32) -> Self {
        LruMap(schnellru::LruMap::new(ByLength::new(max_length)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // linkedlist test
    #[test]
    fn test_linked_list_sanity() {
        let mut list = LinkedList::with_capacity(8);
        assert_eq!(list.nodes.head.next, TAIL);

        let index = list.push_head(10);
        assert_eq!(list.nodes[index].data, 10);

        let tail = list.pop_tail();
        assert_eq!(tail, Some(10));
    }

    #[test]
    fn test_linked_list_push_head() {
        let mut list = LinkedList::with_capacity(8);

        let index = list.push_head(10);
        assert_eq!(list.nodes[index].data, 10);
        assert_eq!(list.nodes.head.next, index);
        assert_eq!(list.nodes[index].next, TAIL);

        let index2 = list.push_head(20);
        assert_eq!(list.nodes[index2].data, 20);
        assert_eq!(list.nodes.head.next, index2);
        assert_eq!(list.nodes[index2].next, index);
    }

    #[test]
    fn test_linked_list_pop_tail() {
        let mut list = LinkedList::with_capacity(8);

        let index = list.push_head(10);
        let index2 = list.push_head(20);
        assert_eq!(list.nodes.head.next, index2);
        assert_eq!(list.nodes[index2].next, index);

        let tail = list.pop_tail();
        assert_eq!(tail, Some(10));
        assert_eq!(list.nodes.head.next, index2);
    }

    #[test]
    fn test_linked_list_remove() {
        let mut list = LinkedList::with_capacity(8);

        let index = list.push_head(10);
        let index2 = list.push_head(20);
        assert_eq!(list.nodes.head.next, index2);
        assert_eq!(list.nodes[index2].next, index);

        let removed = list.remove(index2);
        assert_eq!(removed, Some(20));
        assert_eq!(list.nodes.head.next, index);
    }

    #[test]
    fn test_sanity() {
        let mut cache = LruCache::with_capacity(8);
        assert_eq!(cache.lookup_table.len(), 0);
        assert!(cache.is_empty());

        cache.insert("hash");
        assert_eq!(cache.lookup_table.len(), 1);
        assert!(cache.contains(&"hash"));
        assert!(!cache.is_empty());
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn test_insert() {
        let mut cache = LruCache::with_capacity(8);
        assert_eq!(cache.lookup_table.len(), 0);

        cache.insert("hash");
        assert_eq!(cache.lookup_table.len(), 1);
    }

    #[test]
    fn test_insert_and_get_evicted() {
        let mut cache = LruCache::with_capacity(8);
        assert_eq!(cache.lookup_table.len(), 0);

        cache.insert("hash");
        assert_eq!(cache.lookup_table.len(), 1);

        let (success, evicted) = cache.insert_and_get_evicted("hash");
        assert!(!success);
        assert_eq!(evicted, None);
    }

    #[test]
    fn test_contains() {
        let mut cache = LruCache::with_capacity(8);
        assert_eq!(cache.lookup_table.len(), 0);

        cache.insert("hash");
        assert_eq!(cache.lookup_table.len(), 1);

        let contains = cache.contains(&"hash");
        assert!(contains);
    }

    #[test]
    fn test_remove_lru() {
        let mut cache = LruCache::with_capacity(8);
        assert_eq!(cache.lookup_table.len(), 0);

        cache.insert("hash");
        assert_eq!(cache.lookup_table.len(), 1);

        let removed = cache.remove_lru();
        assert_eq!(removed, Some("hash"));
    }

    #[test]
    fn test_extend() {
        let mut cache = LruCache::with_capacity(8);
        assert_eq!(cache.lookup_table.len(), 0);

        cache.extend(vec!["hash", "hash2"]);
        assert_eq!(cache.lookup_table.len(), 2);
    }

    #[test]
    fn test_iter() {
        let mut cache = LruCache::with_capacity(8);

        cache.insert("first");
        cache.insert("second");
        cache.insert("third");

        let iter = cache.iter();
        let res = iter.collect::<Vec<_>>();
        assert_eq!(res, vec![&"third", &"second", &"first"]);
    }

    #[test]
    #[allow(dead_code)]
    fn test_debug_impl_lru_map() {
        use derive_more::Display;

        #[derive(Debug, Hash, PartialEq, Eq, Display)]
        struct Key(i8);

        #[derive(Debug)]
        struct Value(i8);

        let mut cache = LruMap::new(2);
        let key_1 = Key(1);
        let value_1 = Value(11);
        cache.insert(key_1, value_1);
        let key_2 = Key(2);
        let value_2 = Value(22);
        cache.insert(key_2, value_2);

        assert_eq!("LruMap { limiter: ByLength { max_length: 2 }, res_fn_iter: Iter: { 2: Value(22), 1: Value(11) } }", format!("{cache:?}"))
    }
}
