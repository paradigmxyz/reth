use alloy_primitives::map::{Entry, HashMap};
use reth_trie_common::Nibbles;
use reth_trie_sparse::SparseNode;

/// Maximum suffix length (relative to the base path) for which nodes are stored
/// in the flat array instead of the hashmap.
const SHORT_PATH_MAX_LEN: usize = 3;

/// Total number of slots in the flat array.
/// This is `sum(16^i for i in 0..=SHORT_PATH_MAX_LEN)` = 1 + 16 + 256 + 4096 = 4369.
const SHORT_ARRAY_LEN: usize = short_offset(SHORT_PATH_MAX_LEN + 1);

/// Computes the starting offset for suffix of given length.
/// `offset(L) = (16^L - 1) / 15`
const fn short_offset(len: usize) -> usize {
    match len {
        0 => 0,
        1 => 1,
        2 => 17,
        3 => 273,
        4 => 4369,
        _ => unreachable!(),
    }
}

/// Converts a nibble path suffix of length `0..=SHORT_PATH_MAX_LEN` to a flat array index.
///
/// The suffix is treated as a base-16 number, added to the offset for its length tier.
fn suffix_to_index(path: &Nibbles, base_len: usize) -> usize {
    let suffix_len = path.len() - base_len;
    debug_assert!(suffix_len <= SHORT_PATH_MAX_LEN);
    let mut v: usize = 0;
    for i in base_len..path.len() {
        v = (v << 4) | (path.get_unchecked(i) as usize);
    }
    short_offset(suffix_len) + v
}

/// Reconstructs a nibble path from a flat array index and the base path.
fn index_to_path(index: usize, base: &Nibbles) -> Nibbles {
    let (suffix_len, remainder) = if index < 1 {
        (0, 0)
    } else if index < 17 {
        (1, index - 1)
    } else if index < 273 {
        (2, index - 17)
    } else {
        (3, index - 273)
    };

    let mut path = *base;
    for i in (0..suffix_len).rev() {
        let nibble = (remainder >> (i * 4)) & 0xF;
        path.push_unchecked(nibble as u8);
    }
    path
}

/// A hybrid node storage that uses a pre-allocated flat array for short paths
/// and falls back to a [`HashMap`] for longer paths.
///
/// Nodes with paths whose suffix relative to `base` is at most [`SHORT_PATH_MAX_LEN`] nibbles
/// are stored in a flat array indexed by converting the suffix to an integer. This avoids
/// expensive hashmap lookups for the most frequently accessed near-root nodes.
#[derive(Clone, Debug, Default)]
pub(crate) struct SparseSubtrieNodes {
    base: Nibbles,
    /// When `false`, the flat array is disabled and all nodes go to the hashmap.
    use_short: bool,
    short: Option<Vec<Option<SparseNode>>>,
    short_len: usize,
    long: HashMap<Nibbles, SparseNode>,
}

impl PartialEq for SparseSubtrieNodes {
    fn eq(&self, other: &Self) -> bool {
        if self.len() != other.len() {
            return false;
        }
        self.iter().all(|(path, node)| other.get(&path) == Some(node))
    }
}

impl Eq for SparseSubtrieNodes {}

impl SparseSubtrieNodes {
    /// Creates a new empty node storage with the given base path.
    /// When `use_short` is true, short paths are stored in a flat array for fast lookups.
    pub(crate) fn new(base: Nibbles, use_short: bool) -> Self {
        Self { base, use_short, ..Default::default() }
    }

    /// Creates node storage with a single entry.
    pub(crate) fn from_single(path: Nibbles, node: SparseNode, use_short: bool) -> Self {
        let mut nodes = Self::new(path, use_short);
        nodes.insert(path, node);
        nodes
    }

    /// Sets the base path, re-indexing all existing entries.
    pub(crate) fn set_base(&mut self, base: Nibbles) {
        if self.base == base {
            return;
        }

        if self.is_empty() {
            self.base = base;
            return;
        }

        let old_base = self.base;
        self.base = base;

        let old_short = self.short.take();
        let old_short_len = self.short_len;
        self.short_len = 0;

        let old_long = core::mem::take(&mut self.long);
        let old_long_len = old_long.len();

        if let Some(old_short) = old_short {
            for (i, slot) in old_short.into_iter().enumerate() {
                if let Some(node) = slot {
                    let path = index_to_path(i, &old_base);
                    self.insert(path, node);
                }
            }
        }

        for (path, node) in old_long {
            self.insert(path, node);
        }

        debug_assert_eq!(self.short_len + self.long.len(), old_short_len + old_long_len);
    }

    /// Returns true if the path is short enough to be stored in the flat array.
    fn is_short(&self, path: &Nibbles) -> bool {
        debug_assert!(
            path.len() >= self.base.len(),
            "path {path:?} is shorter than base {:?}",
            self.base
        );
        self.use_short && path.len() - self.base.len() <= SHORT_PATH_MAX_LEN
    }

    /// Returns the total number of nodes stored.
    pub(crate) fn len(&self) -> usize {
        self.short_len + self.long.len()
    }

    /// Returns true if no nodes are stored.
    pub(crate) fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Looks up a node by its full path.
    pub(crate) fn get(&self, path: &Nibbles) -> Option<&SparseNode> {
        if self.is_short(path) {
            self.short.as_ref()?.get(suffix_to_index(path, self.base.len()))?.as_ref()
        } else {
            self.long.get(path)
        }
    }

    /// Looks up a node mutably by its full path.
    pub(crate) fn get_mut(&mut self, path: &Nibbles) -> Option<&mut SparseNode> {
        if self.is_short(path) {
            self.short.as_mut()?.get_mut(suffix_to_index(path, self.base.len()))?.as_mut()
        } else {
            self.long.get_mut(path)
        }
    }

    /// Inserts a node at the given path, returning any previously stored node.
    pub(crate) fn insert(&mut self, path: Nibbles, node: SparseNode) -> Option<SparseNode> {
        if self.is_short(&path) {
            let idx = suffix_to_index(&path, self.base.len());
            let short = self.short.get_or_insert_with(|| {
                let mut v = Vec::with_capacity(SHORT_ARRAY_LEN);
                v.resize_with(SHORT_ARRAY_LEN, || None);
                v
            });
            let slot = &mut short[idx];
            let prev = slot.take();
            if prev.is_none() {
                self.short_len += 1;
            }
            *slot = Some(node);
            prev
        } else {
            self.long.insert(path, node)
        }
    }

    /// Removes and returns the node at the given path.
    pub(crate) fn remove(&mut self, path: &Nibbles) -> Option<SparseNode> {
        if self.is_short(path) {
            let slot = &mut self.short.as_mut()?[suffix_to_index(path, self.base.len())];
            let prev = slot.take();
            if prev.is_some() {
                self.short_len -= 1;
            }
            prev
        } else {
            self.long.remove(path)
        }
    }

    /// Returns an entry for the given path, allowing in-place manipulation.
    pub(crate) fn entry(&mut self, path: Nibbles) -> SparseSubtrieNodesEntry<'_> {
        if self.is_short(&path) {
            let idx = suffix_to_index(&path, self.base.len());
            let short = self.short.get_or_insert_with(|| {
                let mut v = Vec::with_capacity(SHORT_ARRAY_LEN);
                v.resize_with(SHORT_ARRAY_LEN, || None);
                v
            });
            if short[idx].is_some() {
                SparseSubtrieNodesEntry::ShortOccupied(ShortOccupiedEntry {
                    slot: &mut short[idx],
                    len: &mut self.short_len,
                    key: path,
                })
            } else {
                SparseSubtrieNodesEntry::ShortVacant(ShortVacantEntry {
                    slot: &mut short[idx],
                    len: &mut self.short_len,
                    key: path,
                })
            }
        } else {
            SparseSubtrieNodesEntry::Long(self.long.entry(path))
        }
    }

    /// Reserves capacity for at least `additional` more elements in the long storage.
    pub(crate) fn reserve(&mut self, additional: usize) {
        self.long.reserve(additional);
    }

    /// Shrinks the capacity of the long storage.
    pub(crate) fn shrink_to(&mut self, min_capacity: usize) {
        self.long.shrink_to(min_capacity);
    }

    /// Clears all nodes from both short and long storage.
    pub(crate) fn clear(&mut self) {
        if self.short_len > 0 {
            if let Some(short) = self.short.as_mut() {
                short.fill(None);
            }
            self.short_len = 0;
        }
        self.long.clear();
    }

    /// Retains only nodes for which the predicate returns `true`.
    pub(crate) fn retain(&mut self, mut f: impl FnMut(&Nibbles, &mut SparseNode) -> bool) {
        if let Some(short) = self.short.as_mut() {
            let base = self.base;
            let base_len = base.len();
            for (i, slot) in short.iter_mut().enumerate() {
                if let Some(node) = slot {
                    let path = index_to_path(i, &base);
                    debug_assert_eq!(suffix_to_index(&path, base_len), i);
                    if !f(&path, node) {
                        *slot = None;
                        self.short_len -= 1;
                    }
                }
            }
        }
        self.long.retain(f);
    }

    /// Returns an iterator over all `(path, node)` pairs.
    pub(crate) fn iter(&self) -> SparseSubtrieNodesIter<'_> {
        SparseSubtrieNodesIter {
            base: &self.base,
            short: self.short.as_deref().unwrap_or(&[]),
            short_idx: 0,
            long: self.long.iter(),
        }
    }

    /// Returns a heuristic for the in-memory size of this node storage in bytes.
    pub(crate) fn memory_size(&self) -> usize {
        let mut size = core::mem::size_of::<Self>();

        if let Some(short) = &self.short {
            size += short.capacity() * core::mem::size_of::<Option<SparseNode>>();
            for node in short.iter().flatten() {
                size += node.memory_size();
            }
        }

        for (path, node) in &self.long {
            size += core::mem::size_of::<Nibbles>();
            size += path.len();
            size += node.memory_size();
        }

        size
    }
}

/// An entry in [`SparseSubtrieNodes`], similar to [`Entry`] for `HashMap`.
pub(crate) enum SparseSubtrieNodesEntry<'a> {
    ShortOccupied(ShortOccupiedEntry<'a>),
    ShortVacant(ShortVacantEntry<'a>),
    Long(Entry<'a, Nibbles, SparseNode>),
}

pub(crate) struct ShortOccupiedEntry<'a> {
    slot: &'a mut Option<SparseNode>,
    #[allow(dead_code)]
    len: &'a mut usize,
    key: Nibbles,
}

pub(crate) struct ShortVacantEntry<'a> {
    slot: &'a mut Option<SparseNode>,
    len: &'a mut usize,
    key: Nibbles,
}

/// An occupied entry in [`SparseSubtrieNodes`].
pub(crate) enum OccupiedEntry<'a> {
    Short(ShortOccupiedEntry<'a>),
    Long(alloy_primitives::map::OccupiedEntry<'a, Nibbles, SparseNode>),
}

/// A vacant entry in [`SparseSubtrieNodes`].
pub(crate) enum VacantEntry<'a> {
    Short(ShortVacantEntry<'a>),
    Long(alloy_primitives::map::VacantEntry<'a, Nibbles, SparseNode>),
}

impl<'a> SparseSubtrieNodesEntry<'a> {
    /// Converts this entry into occupied/vacant enum variants matching [`Entry`] semantics.
    pub(crate) const fn split(self) -> Result<OccupiedEntry<'a>, VacantEntry<'a>> {
        match self {
            Self::ShortOccupied(e) => Ok(OccupiedEntry::Short(e)),
            Self::ShortVacant(e) => Err(VacantEntry::Short(e)),
            Self::Long(Entry::Occupied(e)) => Ok(OccupiedEntry::Long(e)),
            Self::Long(Entry::Vacant(e)) => Err(VacantEntry::Long(e)),
        }
    }
}

impl<'a> OccupiedEntry<'a> {
    /// Gets a reference to the key.
    pub(crate) fn key(&self) -> &Nibbles {
        match self {
            Self::Short(e) => &e.key,
            Self::Long(e) => e.key(),
        }
    }

    /// Gets a reference to the value.
    pub(crate) fn get(&self) -> &SparseNode {
        match self {
            Self::Short(e) => e.slot.as_ref().unwrap(),
            Self::Long(e) => e.get(),
        }
    }

    /// Replaces the value, returning the old value.
    pub(crate) fn insert(&mut self, value: SparseNode) -> SparseNode {
        match self {
            Self::Short(e) => e.slot.replace(value).unwrap(),
            Self::Long(e) => e.insert(value),
        }
    }

    /// Takes ownership of the value, removing it from the map.
    #[allow(dead_code)]
    pub(crate) fn remove(self) -> SparseNode {
        match self {
            Self::Short(e) => {
                *e.len -= 1;
                e.slot.take().unwrap()
            }
            Self::Long(e) => e.remove(),
        }
    }
}

impl<'a> VacantEntry<'a> {
    /// Gets a reference to the key.
    pub(crate) fn key(&self) -> &Nibbles {
        match self {
            Self::Short(e) => &e.key,
            Self::Long(e) => e.key(),
        }
    }

    /// Inserts a value into the vacant entry and returns a mutable reference to it.
    pub(crate) fn insert(self, value: SparseNode) -> &'a mut SparseNode {
        match self {
            Self::Short(e) => {
                *e.len += 1;
                *e.slot = Some(value);
                e.slot.as_mut().unwrap()
            }
            Self::Long(e) => e.insert(value),
        }
    }
}

/// Iterator over `(Nibbles, &SparseNode)` pairs in [`SparseSubtrieNodes`].
pub(crate) struct SparseSubtrieNodesIter<'a> {
    base: &'a Nibbles,
    short: &'a [Option<SparseNode>],
    short_idx: usize,
    long: alloy_primitives::map::hash_map::Iter<'a, Nibbles, SparseNode>,
}

impl<'a> Iterator for SparseSubtrieNodesIter<'a> {
    type Item = (Nibbles, &'a SparseNode);

    fn next(&mut self) -> Option<Self::Item> {
        while self.short_idx < self.short.len() {
            let idx = self.short_idx;
            self.short_idx += 1;
            if let Some(node) = &self.short[idx] {
                return Some((index_to_path(idx, self.base), node));
            }
        }
        self.long.next().map(|(path, node)| (*path, node))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_suffix_to_index_roundtrip() {
        let base = Nibbles::from_nibbles([0x1, 0x2]);
        let base_len = base.len();

        // Empty suffix (path == base)
        let path = base;
        assert_eq!(suffix_to_index(&path, base_len), 0);
        assert_eq!(index_to_path(0, &base), path);

        // Single nibble suffix
        for n in 0..16u8 {
            let mut p = base;
            p.push_unchecked(n);
            let idx = suffix_to_index(&p, base_len);
            assert_eq!(idx, 1 + n as usize);
            assert_eq!(index_to_path(idx, &base), p);
        }

        // Two nibble suffix
        for n0 in 0..16u8 {
            for n1 in 0..16u8 {
                let mut p = base;
                p.push_unchecked(n0);
                p.push_unchecked(n1);
                let idx = suffix_to_index(&p, base_len);
                assert_eq!(idx, 17 + (n0 as usize) * 16 + n1 as usize);
                assert_eq!(index_to_path(idx, &base), p);
            }
        }

        // Three nibble suffix
        let mut p = base;
        p.push_unchecked(0xA);
        p.push_unchecked(0xB);
        p.push_unchecked(0xC);
        let idx = suffix_to_index(&p, base_len);
        assert_eq!(idx, 273 + 0xA * 256 + 0xB * 16 + 0xC);
        assert_eq!(index_to_path(idx, &base), p);
    }

    #[test]
    fn test_insert_get_remove() {
        let base = Nibbles::default();
        let mut nodes = SparseSubtrieNodes::new(base, true);

        // Insert at root (short)
        nodes.insert(base, SparseNode::Empty);
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes.get(&base), Some(&SparseNode::Empty));

        // Insert at short path
        let mut short_path = base;
        short_path.push_unchecked(0x5);
        nodes.insert(short_path, SparseNode::new_branch(alloy_trie::TrieMask::new(0)));
        assert_eq!(nodes.len(), 2);
        assert!(nodes.get(&short_path).is_some());

        // Insert at long path (> 3 nibbles)
        let long_path = Nibbles::from_nibbles([0x1, 0x2, 0x3, 0x4]);
        nodes.insert(long_path, SparseNode::Empty);
        assert_eq!(nodes.len(), 3);
        assert_eq!(nodes.get(&long_path), Some(&SparseNode::Empty));

        // Remove
        assert!(nodes.remove(&short_path).is_some());
        assert_eq!(nodes.len(), 2);
        assert!(nodes.get(&short_path).is_none());
    }

    #[test]
    fn test_entry_api() {
        let base = Nibbles::default();
        let mut nodes = SparseSubtrieNodes::new(base, true);

        let path = Nibbles::from_nibbles([0x1]);
        match nodes.entry(path).split() {
            Ok(_) => panic!("expected vacant"),
            Err(vacant) => {
                vacant.insert(SparseNode::Empty);
            }
        }
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes.get(&path), Some(&SparseNode::Empty));

        match nodes.entry(path).split() {
            Ok(mut occupied) => {
                assert_eq!(occupied.key(), &path);
                assert_eq!(occupied.get(), &SparseNode::Empty);
                occupied.insert(SparseNode::new_branch(alloy_trie::TrieMask::new(0xFF)));
            }
            Err(_) => panic!("expected occupied"),
        }
    }

    #[test]
    fn test_iter_and_retain() {
        let base = Nibbles::default();
        let mut nodes = SparseSubtrieNodes::new(base, true);

        nodes.insert(Nibbles::default(), SparseNode::Empty);
        nodes.insert(Nibbles::from_nibbles([0x1]), SparseNode::Empty);
        nodes.insert(Nibbles::from_nibbles([0x1, 0x2, 0x3, 0x4]), SparseNode::Empty);
        assert_eq!(nodes.len(), 3);

        let collected: Vec<_> = nodes.iter().collect();
        assert_eq!(collected.len(), 3);

        nodes.retain(|path, _| path.len() < 2);
        assert_eq!(nodes.len(), 2);
    }

    #[test]
    fn test_clear() {
        let base = Nibbles::default();
        let mut nodes = SparseSubtrieNodes::new(base, true);

        nodes.insert(Nibbles::default(), SparseNode::Empty);
        nodes.insert(Nibbles::from_nibbles([0x1]), SparseNode::Empty);
        assert_eq!(nodes.len(), 2);

        nodes.clear();
        assert!(nodes.is_empty());
    }
}
