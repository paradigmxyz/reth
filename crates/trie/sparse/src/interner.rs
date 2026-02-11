//! Interned node storage for the parallel sparse trie.

use alloy_primitives::map::HashMap;
use core::ops;
pub use inturn;
use inturn::{CopyInterner, Symbol};
use reth_trie_common::Nibbles;
use std::sync::Arc;

use crate::SparseNode;

/// A shared path interner backed by [`CopyInterner<Nibbles>`].
///
/// This is a thread-safe, lock-free interner that deduplicates [`Nibbles`] paths across the entire
/// [`crate::ParallelSparseTrie`]. Each unique path is assigned a compact [`Symbol`] index.
/// The interner never frees symbols; it only grows. Node storage lifecycle is managed separately
/// by each [`SparseNodeInterner`] via its `HashMap<Symbol, SparseNode>`.
pub type SharedPathInterner = Arc<CopyInterner<Nibbles>>;

/// Creates a new shared path interner.
pub fn shared_path_interner() -> SharedPathInterner {
    Arc::new(CopyInterner::new())
}

/// An interned node store for [`SparseNode`]s keyed by [`Nibbles`] path.
///
/// Paths are interned into compact [`Symbol`]s via a [`SharedPathInterner`] that is shared across
/// the entire [`crate::ParallelSparseTrie`] (upper subtrie + all 256 lower subtries).
/// Nodes are stored in a `HashMap<Symbol, SparseNode>` per subtrie.
pub struct SparseNodeInterner {
    /// Shared path interner.
    interner: SharedPathInterner,
    /// Nodes stored by interned symbol.
    nodes: HashMap<Symbol, SparseNode>,
}

impl core::fmt::Debug for SparseNodeInterner {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_map().entries(self.iter()).finish()
    }
}

impl Clone for SparseNodeInterner {
    fn clone(&self) -> Self {
        Self { interner: Arc::clone(&self.interner), nodes: self.nodes.clone() }
    }
}

impl PartialEq for SparseNodeInterner {
    fn eq(&self, other: &Self) -> bool {
        if self.nodes.len() != other.nodes.len() {
            return false;
        }
        for (path, node) in self.iter() {
            match other.get(path) {
                Some(other_node) if node == other_node => {}
                _ => return false,
            }
        }
        true
    }
}

impl Eq for SparseNodeInterner {}

impl SparseNodeInterner {
    /// Creates an empty interner backed by the given shared path interner.
    pub fn new(interner: SharedPathInterner) -> Self {
        Self { interner, nodes: HashMap::default() }
    }

    /// Returns a reference to the shared path interner.
    pub const fn interner(&self) -> &SharedPathInterner {
        &self.interner
    }

    /// Returns the number of live entries.
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// Returns `true` if there are no live entries.
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Reserves capacity for at least `additional` more entries.
    pub fn reserve(&mut self, additional: usize) {
        self.nodes.reserve(additional);
    }

    /// Shrinks the capacity of the underlying storage.
    pub fn shrink_to(&mut self, min_capacity: usize) {
        self.nodes.shrink_to(min_capacity);
    }

    /// Gets a reference to the node at the given path.
    pub fn get(&self, path: &Nibbles) -> Option<&SparseNode> {
        let sym = self.interner.intern(path);
        self.nodes.get(&sym)
    }

    /// Gets a mutable reference to the node at the given path.
    pub fn get_mut(&mut self, path: &Nibbles) -> Option<&mut SparseNode> {
        let sym = self.interner.intern(path);
        self.nodes.get_mut(&sym)
    }

    /// Returns `true` if the path has a live node.
    pub fn contains_key(&self, path: &Nibbles) -> bool {
        let sym = self.interner.intern(path);
        self.nodes.contains_key(&sym)
    }

    /// Inserts a node at the given path, returning the previous node if any.
    pub fn insert(&mut self, path: Nibbles, node: SparseNode) -> Option<SparseNode> {
        let sym = self.interner.intern(&path);
        self.nodes.insert(sym, node)
    }

    /// Removes the node at the given path, returning it if it existed.
    pub fn remove(&mut self, path: &Nibbles) -> Option<SparseNode> {
        let sym = self.interner.intern(path);
        self.nodes.remove(&sym)
    }

    /// Provides entry API similar to [`HashMap::entry`].
    pub fn entry(&mut self, path: Nibbles) -> Entry<'_> {
        let sym = self.interner.intern(&path);
        match self.nodes.entry(sym) {
            alloy_primitives::map::Entry::Occupied(e) => {
                Entry::Occupied(OccupiedEntry { path, entry: e })
            }
            alloy_primitives::map::Entry::Vacant(e) => {
                Entry::Vacant(VacantEntry { path, entry: e })
            }
        }
    }

    /// Retains only the entries for which the predicate returns `true`.
    pub fn retain(&mut self, mut f: impl FnMut(&Nibbles, &mut SparseNode) -> bool) {
        let interner = &self.interner;
        self.nodes.retain(|sym, node| {
            let path = interner.resolve(*sym);
            f(path, node)
        });
    }

    /// Clears all entries, keeping allocations.
    pub fn clear(&mut self) {
        self.nodes.clear();
    }

    /// Iterates over all live `(path, node)` pairs.
    pub fn iter(&self) -> impl Iterator<Item = (&Nibbles, &SparseNode)> {
        self.nodes.iter().map(|(sym, node)| (self.interner.resolve(*sym), node))
    }

    /// Returns a heuristic for the in-memory size in bytes.
    pub fn memory_size(&self) -> usize {
        let mut size = core::mem::size_of::<Self>();
        for (path, node) in self.iter() {
            size += core::mem::size_of::<Symbol>() + core::mem::size_of::<SparseNode>();
            size += path.len();
            size += node.memory_size();
        }
        size
    }
}

/// Entry API for [`SparseNodeInterner`].
#[allow(missing_debug_implementations)]
pub enum Entry<'a> {
    /// An occupied entry.
    Occupied(OccupiedEntry<'a>),
    /// A vacant entry.
    Vacant(VacantEntry<'a>),
}

/// An occupied entry in the [`SparseNodeInterner`].
#[allow(missing_debug_implementations)]
pub struct OccupiedEntry<'a> {
    path: Nibbles,
    entry: alloy_primitives::map::OccupiedEntry<'a, Symbol, SparseNode>,
}

impl<'a> OccupiedEntry<'a> {
    /// Returns a reference to the key.
    pub const fn key(&self) -> &Nibbles {
        &self.path
    }

    /// Returns a reference to the value.
    pub fn get(&self) -> &SparseNode {
        self.entry.get()
    }

    /// Returns a mutable reference to the value.
    pub fn get_mut(&mut self) -> &mut SparseNode {
        self.entry.get_mut()
    }

    /// Replaces the value, returning the old one.
    pub fn insert(&mut self, node: SparseNode) -> SparseNode {
        self.entry.insert(node)
    }

    /// Converts into a mutable reference to the value.
    pub fn into_mut(self) -> &'a mut SparseNode {
        self.entry.into_mut()
    }
}

impl ops::Deref for OccupiedEntry<'_> {
    type Target = SparseNode;

    fn deref(&self) -> &Self::Target {
        self.entry.get()
    }
}

impl ops::DerefMut for OccupiedEntry<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.entry.get_mut()
    }
}

/// A vacant entry in the [`SparseNodeInterner`].
#[allow(missing_debug_implementations)]
pub struct VacantEntry<'a> {
    path: Nibbles,
    entry: alloy_primitives::map::VacantEntry<'a, Symbol, SparseNode>,
}

impl<'a> VacantEntry<'a> {
    /// Returns a reference to the key.
    pub const fn key(&self) -> &Nibbles {
        &self.path
    }

    /// Inserts a value and returns a mutable reference to it.
    pub fn insert(self, node: SparseNode) -> &'a mut SparseNode {
        self.entry.insert(node)
    }
}
