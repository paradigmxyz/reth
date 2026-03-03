use crate::Nibbles;
use alloc::{sync::Arc, vec::Vec};
use alloy_primitives::map::{B256Map, B256Set};

/// Maximum number of keys for the `Small` variant (linear scan / binary search).
/// Sets larger than this threshold use the path-compressed bitmap trie.
const SMALL_SET_THRESHOLD: usize = 16;

/// Collection of mutable prefix sets.
#[derive(Clone, Default, Debug)]
pub struct TriePrefixSetsMut {
    /// A set of account prefixes that have changed.
    pub account_prefix_set: PrefixSetMut,
    /// A map containing storage changes with the hashed address as key and a set of storage key
    /// prefixes as the value.
    pub storage_prefix_sets: B256Map<PrefixSetMut>,
    /// A set of hashed addresses of destroyed accounts.
    pub destroyed_accounts: B256Set,
}

impl TriePrefixSetsMut {
    /// Returns `true` if all prefix sets are empty.
    pub fn is_empty(&self) -> bool {
        self.account_prefix_set.is_empty() &&
            self.storage_prefix_sets.is_empty() &&
            self.destroyed_accounts.is_empty()
    }

    /// Extends prefix sets with contents of another prefix set.
    pub fn extend(&mut self, other: Self) {
        self.account_prefix_set.extend(other.account_prefix_set);
        for (hashed_address, prefix_set) in other.storage_prefix_sets {
            self.storage_prefix_sets.entry(hashed_address).or_default().extend(prefix_set);
        }
        self.destroyed_accounts.extend(other.destroyed_accounts);
    }

    /// Returns a `TriePrefixSets` with the same elements as these sets.
    ///
    /// If not yet sorted, the elements will be sorted and deduplicated.
    pub fn freeze(self) -> TriePrefixSets {
        TriePrefixSets {
            account_prefix_set: self.account_prefix_set.freeze(),
            storage_prefix_sets: self
                .storage_prefix_sets
                .into_iter()
                .map(|(hashed_address, prefix_set)| (hashed_address, prefix_set.freeze()))
                .collect(),
            destroyed_accounts: self.destroyed_accounts,
        }
    }

    /// Clears the prefix sets and destroyed accounts map.
    pub fn clear(&mut self) {
        self.destroyed_accounts.clear();
        self.storage_prefix_sets.clear();
        self.account_prefix_set.clear();
    }
}

/// Collection of trie prefix sets.
#[derive(Default, Debug, Clone)]
pub struct TriePrefixSets {
    /// A set of account prefixes that have changed.
    pub account_prefix_set: PrefixSet,
    /// A map containing storage changes with the hashed address as key and a set of storage key
    /// prefixes as the value.
    pub storage_prefix_sets: B256Map<PrefixSet>,
    /// A set of hashed addresses of destroyed accounts.
    pub destroyed_accounts: B256Set,
}

/// A container for efficiently storing and checking for the presence of key prefixes.
///
/// This data structure stores a set of `Nibbles` and provides methods to insert
/// new elements and check whether any existing element has a given prefix.
///
/// Internally, this implementation stores keys in an unsorted `Vec<Nibbles>` together with an
/// `all` flag. The `all` flag indicates that every entry should be considered changed and that
/// individual keys can be ignored.
///
/// Sorting and deduplication do not happen during insertion or membership checks on this mutable
/// structure. Instead, keys are sorted and deduplicated when converting into the immutable
/// `PrefixSet` via `freeze()`.
///
/// The frozen `PrefixSet` uses a size-specialized strategy:
/// - 0 keys: empty set (instant false)
/// - 1 key: inline single-key comparison (no allocation)
/// - 2–16 keys: sorted slice with binary search
/// - 17+ keys: path-compressed bitmap trie with O(L) lookups
///
/// # Examples
///
/// ```
/// use reth_trie_common::{prefix_set::PrefixSetMut, Nibbles};
///
/// let mut prefix_set_mut = PrefixSetMut::default();
/// prefix_set_mut.insert(Nibbles::from_nibbles_unchecked(&[0xa, 0xb]));
/// prefix_set_mut.insert(Nibbles::from_nibbles_unchecked(&[0xa, 0xb, 0xc]));
/// let prefix_set = prefix_set_mut.freeze();
/// assert!(prefix_set.contains(&Nibbles::from_nibbles_unchecked([0xa, 0xb])));
/// assert!(prefix_set.contains(&Nibbles::from_nibbles_unchecked([0xa, 0xb, 0xc])));
/// ```
#[derive(PartialEq, Eq, Clone, Default, Debug)]
pub struct PrefixSetMut {
    /// Flag indicating that any entry should be considered changed.
    /// If set, the keys will be discarded.
    all: bool,
    keys: Vec<Nibbles>,
}

impl<I> From<I> for PrefixSetMut
where
    I: IntoIterator<Item = Nibbles>,
{
    fn from(value: I) -> Self {
        Self { all: false, keys: value.into_iter().collect() }
    }
}

impl PrefixSetMut {
    /// Create [`PrefixSetMut`] with pre-allocated capacity.
    pub fn with_capacity(capacity: usize) -> Self {
        Self { all: false, keys: Vec::with_capacity(capacity) }
    }

    /// Create [`PrefixSetMut`] that considers all key changed.
    pub const fn all() -> Self {
        Self { all: true, keys: Vec::new() }
    }

    /// Inserts the given `nibbles` into the set.
    pub fn insert(&mut self, nibbles: Nibbles) {
        self.keys.push(nibbles);
    }

    /// Extend prefix set with contents of another prefix set.
    pub fn extend(&mut self, other: Self) {
        self.all |= other.all;
        self.keys.extend(other.keys);
    }

    /// Extend prefix set keys with contents of provided iterator.
    pub fn extend_keys<I>(&mut self, keys: I)
    where
        I: IntoIterator<Item = Nibbles>,
    {
        self.keys.extend(keys);
    }

    /// Returns the number of elements in the set.
    pub const fn len(&self) -> usize {
        self.keys.len()
    }

    /// Returns `true` if the set is empty and `all` flag is not set.
    pub const fn is_empty(&self) -> bool {
        !self.all && self.keys.is_empty()
    }

    /// Clears the inner vec for reuse, setting `all` to `false`.
    pub fn clear(&mut self) {
        self.all = false;
        self.keys.clear();
    }

    /// Returns a `PrefixSet` with the same elements as this set.
    ///
    /// If not yet sorted, the elements will be sorted and deduplicated.
    pub fn freeze(mut self) -> PrefixSet {
        if self.all {
            return PrefixSet { inner: PrefixSetInner::All };
        }

        self.keys.sort_unstable();
        self.keys.dedup();

        let inner = match self.keys.len() {
            0 => PrefixSetInner::Empty,
            1 => PrefixSetInner::One(self.keys.pop().unwrap()),
            n if n <= SMALL_SET_THRESHOLD => {
                self.keys.shrink_to_fit();
                PrefixSetInner::Small(Arc::new(self.keys))
            }
            _ => {
                self.keys.shrink_to_fit();
                let trie = CompactTrie::build(&self.keys);
                PrefixSetInner::Trie { keys: Arc::new(self.keys), trie }
            }
        };

        PrefixSet { inner }
    }
}

/// Size-specialized representation of an immutable prefix set.
#[derive(Debug, Clone)]
enum PrefixSetInner {
    /// No keys.
    Empty,
    /// Every prefix matches (the `all` flag).
    All,
    /// Exactly one key — direct inline comparison, no heap allocation.
    One(Nibbles),
    /// 2–16 keys — sorted slice with binary search.
    Small(Arc<Vec<Nibbles>>),
    /// 17+ keys — path-compressed bitmap trie for O(L) lookups,
    /// with sorted keys retained for iteration.
    Trie { keys: Arc<Vec<Nibbles>>, trie: CompactTrie },
}

/// A node in the path-compressed bitmap trie.
///
/// Each node stores a 16-bit bitmap indicating which nibble children (0-15) exist,
/// and an edge label (skip segment) representing collapsed single-child chains.
#[derive(Debug, Clone)]
struct TrieNode {
    /// 16-bit bitmap: bit `n` is set if a child for nibble `n` exists.
    bitmap: u16,
    /// Number of nibbles in the skip segment (edge label from parent to this node).
    skip_len: u16,
    /// Index of the first child node in the flat `nodes` array. The child for nibble
    /// `n` is at `child_base + popcount(bitmap & ((1 << n) - 1))`.
    child_base: u32,
    /// Byte offset into the shared `skip_data` buffer where this node's skip nibbles begin.
    skip_offset: u32,
}

impl Default for TrieNode {
    fn default() -> Self {
        Self { bitmap: 0, skip_len: 0, child_base: 0, skip_offset: 0 }
    }
}

/// Path-compressed bitmap trie (Patricia-style) for fast prefix containment queries.
///
/// Nodes are stored in a flat array with children of each node placed contiguously.
/// Navigation uses popcount on 16-bit bitmaps to compute child indices in O(1).
/// Single-child chains are collapsed into skip segments stored in a shared buffer.
#[derive(Debug, Clone, Default)]
struct CompactTrie {
    /// Flat array of trie nodes. Node 0 is the root.
    nodes: Vec<TrieNode>,
    /// Shared buffer of skip nibble values (each 0-15 stored as `u8`).
    skip_data: Vec<u8>,
}

impl CompactTrie {
    /// Build a path-compressed bitmap trie from sorted, deduplicated keys.
    fn build(keys: &[Nibbles]) -> Self {
        debug_assert!(!keys.is_empty());
        let mut trie = Self { nodes: Vec::with_capacity(keys.len() * 2), skip_data: Vec::new() };
        // Reserve root node
        trie.nodes.push(TrieNode::default());
        trie.build_node(0, keys, 0);
        trie.nodes.shrink_to_fit();
        trie.skip_data.shrink_to_fit();
        trie
    }

    /// Recursively build a trie node and its descendants.
    ///
    /// `node_idx` is a pre-allocated slot in `self.nodes`.
    /// `keys` is a sorted slice of keys that share the same prefix up to `depth`.
    ///
    /// Uses fixed-size stack arrays (max 16 children per nibble) to avoid heap
    /// allocation during the recursive build.
    fn build_node(&mut self, node_idx: usize, keys: &[Nibbles], depth: usize) {
        // Compute the common prefix (skip segment) shared by all keys beyond `depth`
        let skip_len = common_prefix_len(keys, depth);
        let skip_offset = self.skip_data.len() as u32;

        // Store skip nibbles into the shared buffer
        if skip_len > 0 {
            let first_key = &keys[0];
            for i in 0..skip_len {
                self.skip_data.push(first_key.get_unchecked(depth + i));
            }
        }

        let new_depth = depth + skip_len;

        // Group keys by their nibble at `new_depth` using stack-allocated arrays
        // (max 16 children for nibbles 0-15, zero heap allocation)
        let mut bitmap = 0u16;
        let mut group_starts = [0u32; 16];
        let mut group_ends = [0u32; 16];

        let mut i = 0;
        while i < keys.len() {
            if keys[i].len() <= new_depth {
                // Key is fully consumed by the skip segment — no child contribution
                i += 1;
                continue;
            }
            let nibble = keys[i].get_unchecked(new_depth) as usize;
            bitmap |= 1u16 << nibble;
            group_starts[nibble] = i as u32;
            while i < keys.len() &&
                keys[i].len() > new_depth &&
                keys[i].get_unchecked(new_depth) as usize == nibble
            {
                i += 1;
            }
            group_ends[nibble] = i as u32;
        }

        // Reserve contiguous child slots so siblings are adjacent in memory
        let num_children = bitmap.count_ones() as usize;
        let child_base = self.nodes.len() as u32;
        self.nodes.resize_with(self.nodes.len() + num_children, TrieNode::default);

        // Write this node's data
        self.nodes[node_idx] =
            TrieNode { bitmap, skip_len: skip_len as u16, child_base, skip_offset };

        // Recurse into each child group in nibble order (iterate set bits)
        let mut child_rank = 0;
        let mut remaining = bitmap;
        while remaining != 0 {
            let nibble = remaining.trailing_zeros() as usize;
            remaining &= remaining - 1; // clear lowest set bit
            let start = group_starts[nibble] as usize;
            let end = group_ends[nibble] as usize;
            self.build_node(child_base as usize + child_rank, &keys[start..end], new_depth + 1);
            child_rank += 1;
        }
    }

    /// Check if any key in the trie starts with the given prefix.
    ///
    /// Traverses the trie following the prefix nibbles. At each node:
    /// 1. Match the skip segment (compressed single-child chain)
    /// 2. If prefix is consumed during or after the skip, return `true` (subtree exists)
    /// 3. Check the 16-bit bitmap for the next nibble
    /// 4. Use popcount to navigate to the child
    #[inline]
    fn contains(&self, prefix: &Nibbles) -> bool {
        let prefix_len = prefix.len();
        let mut node_idx = 0usize;
        let mut nib_pos = 0usize;

        loop {
            let node = unsafe { self.nodes.get_unchecked(node_idx) };

            // 1. Match skip segment
            let skip_end = node.skip_offset as usize + node.skip_len as usize;
            let mut skip_pos = node.skip_offset as usize;
            while skip_pos < skip_end {
                if nib_pos >= prefix_len {
                    return true; // prefix consumed within skip → subtree exists
                }
                if unsafe { *self.skip_data.get_unchecked(skip_pos) } !=
                    prefix.get_unchecked(nib_pos)
                {
                    return false; // mismatch
                }
                nib_pos += 1;
                skip_pos += 1;
            }

            // 2. Prefix fully consumed after skip?
            if nib_pos >= prefix_len {
                return true;
            }

            // 3. Check bitmap for the next nibble
            let nibble = prefix.get_unchecked(nib_pos);
            let bit = 1u16 << nibble;
            if node.bitmap & bit == 0 {
                return false; // no child for this nibble
            }

            // 4. Popcount navigation to child
            let rank = (node.bitmap & (bit - 1)).count_ones() as usize;
            node_idx = node.child_base as usize + rank;
            nib_pos += 1;
        }
    }
}

/// Compute the length of the common prefix shared by all keys starting at `start`.
///
/// For a single key, returns its remaining length. For sorted keys, only the first
/// and last need to be compared (intermediate keys are lexicographically between them).
fn common_prefix_len(keys: &[Nibbles], start: usize) -> usize {
    if keys.len() <= 1 {
        return keys.first().map(|k| k.len().saturating_sub(start)).unwrap_or(0);
    }
    let first = &keys[0];
    let last = &keys[keys.len() - 1];
    let max_len = first.len().min(last.len());
    if start >= max_len {
        return 0;
    }
    let mut len = 0;
    while start + len < max_len &&
        first.get_unchecked(start + len) == last.get_unchecked(start + len)
    {
        len += 1;
    }
    len
}

/// A sorted prefix set with size-specialized storage for fast lookups.
///
/// Constructed via [`PrefixSetMut::freeze`]. Uses an enum-based strategy:
/// - **Empty/All**: instant return
/// - **One**: single inline key, direct prefix comparison
/// - **Small** (2–16 keys): binary search on sorted slice
/// - **Trie** (17+ keys): path-compressed bitmap trie, O(L) per query
#[derive(Debug, Clone)]
pub struct PrefixSet {
    inner: PrefixSetInner,
}

impl Default for PrefixSet {
    fn default() -> Self {
        Self { inner: PrefixSetInner::Empty }
    }
}

impl PrefixSet {
    /// Returns `true` if any of the keys in the set has the given prefix.
    #[inline]
    pub fn contains(&self, prefix: &Nibbles) -> bool {
        match &self.inner {
            PrefixSetInner::Empty => false,
            PrefixSetInner::All => true,
            PrefixSetInner::One(key) => prefix.is_empty() || key.starts_with(prefix),
            PrefixSetInner::Small(keys) => {
                if prefix.is_empty() {
                    return true;
                }
                // Binary search: all keys with a given prefix form a contiguous sorted range,
                // so the first key >= prefix either starts with it or nothing does.
                let idx = keys.partition_point(|k| k < prefix);
                idx < keys.len() && keys[idx].starts_with(prefix)
            }
            PrefixSetInner::Trie { trie, .. } => {
                if prefix.is_empty() {
                    return true;
                }
                trie.contains(prefix)
            }
        }
    }

    /// Returns an iterator over reference to _all_ nibbles regardless of cursor position.
    pub fn iter(&self) -> core::slice::Iter<'_, Nibbles> {
        self.keys_slice().iter()
    }

    /// Returns true if every entry should be considered changed.
    pub const fn all(&self) -> bool {
        matches!(self.inner, PrefixSetInner::All)
    }

    /// Returns the number of elements in the set.
    pub fn len(&self) -> usize {
        self.keys_slice().len()
    }

    /// Returns `true` if the set is empty and `all` flag is not set.
    pub fn is_empty(&self) -> bool {
        matches!(self.inner, PrefixSetInner::Empty)
    }

    /// Returns a slice of all keys in sorted order.
    fn keys_slice(&self) -> &[Nibbles] {
        match &self.inner {
            PrefixSetInner::Empty | PrefixSetInner::All => &[],
            PrefixSetInner::One(key) => core::slice::from_ref(key),
            PrefixSetInner::Small(keys) | PrefixSetInner::Trie { keys, .. } => keys,
        }
    }
}

impl<'a> IntoIterator for &'a PrefixSet {
    type Item = &'a Nibbles;
    type IntoIter = core::slice::Iter<'a, Nibbles>;
    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_contains_with_multiple_inserts_and_duplicates() {
        let mut prefix_set_mut = PrefixSetMut::default();
        prefix_set_mut.insert(Nibbles::from_nibbles([1, 2, 3]));
        prefix_set_mut.insert(Nibbles::from_nibbles([1, 2, 4]));
        prefix_set_mut.insert(Nibbles::from_nibbles([4, 5, 6]));
        prefix_set_mut.insert(Nibbles::from_nibbles([1, 2, 3])); // Duplicate

        let prefix_set = prefix_set_mut.freeze();
        assert!(prefix_set.contains(&Nibbles::from_nibbles_unchecked([1, 2])));
        assert!(prefix_set.contains(&Nibbles::from_nibbles_unchecked([4, 5])));
        assert!(!prefix_set.contains(&Nibbles::from_nibbles_unchecked([7, 8])));
        assert_eq!(prefix_set.len(), 3); // Length should be 3 (excluding duplicate)
    }

    #[test]
    fn test_freeze_shrinks_capacity() {
        let mut prefix_set_mut = PrefixSetMut::default();
        prefix_set_mut.insert(Nibbles::from_nibbles([1, 2, 3]));
        prefix_set_mut.insert(Nibbles::from_nibbles([1, 2, 4]));
        prefix_set_mut.insert(Nibbles::from_nibbles([4, 5, 6]));
        prefix_set_mut.insert(Nibbles::from_nibbles([1, 2, 3])); // Duplicate

        assert_eq!(prefix_set_mut.keys.len(), 4); // Length is 4 (before deduplication)
        assert_eq!(prefix_set_mut.keys.capacity(), 4); // Capacity is 4 (before deduplication)

        let prefix_set = prefix_set_mut.freeze();
        assert!(prefix_set.contains(&Nibbles::from_nibbles_unchecked([1, 2])));
        assert!(prefix_set.contains(&Nibbles::from_nibbles_unchecked([4, 5])));
        assert!(!prefix_set.contains(&Nibbles::from_nibbles_unchecked([7, 8])));
        assert_eq!(prefix_set.len(), 3); // 3 after dedup
                                         // With Small variant: keys are in Arc<Vec> with shrunk capacity
        if let PrefixSetInner::Small(ref keys) = prefix_set.inner {
            assert_eq!(keys.capacity(), 3);
        } else {
            panic!("expected Small variant for 3 keys");
        }
    }

    #[test]
    fn test_freeze_shrinks_existing_capacity() {
        let mut prefix_set_mut = PrefixSetMut::with_capacity(101);
        prefix_set_mut.insert(Nibbles::from_nibbles([1, 2, 3]));
        prefix_set_mut.insert(Nibbles::from_nibbles([1, 2, 4]));
        prefix_set_mut.insert(Nibbles::from_nibbles([4, 5, 6]));
        prefix_set_mut.insert(Nibbles::from_nibbles([1, 2, 3])); // Duplicate

        assert_eq!(prefix_set_mut.keys.len(), 4);
        assert_eq!(prefix_set_mut.keys.capacity(), 101);

        let prefix_set = prefix_set_mut.freeze();
        assert!(prefix_set.contains(&Nibbles::from_nibbles_unchecked([1, 2])));
        assert!(prefix_set.contains(&Nibbles::from_nibbles_unchecked([4, 5])));
        assert!(!prefix_set.contains(&Nibbles::from_nibbles_unchecked([7, 8])));
        assert_eq!(prefix_set.len(), 3);
        if let PrefixSetInner::Small(ref keys) = prefix_set.inner {
            assert_eq!(keys.capacity(), 3);
        } else {
            panic!("expected Small variant for 3 keys");
        }
    }

    #[test]
    fn test_prefix_set_all_extend() {
        let mut prefix_set_mut = PrefixSetMut::default();
        prefix_set_mut.extend(PrefixSetMut::all());
        assert!(prefix_set_mut.all);
    }

    #[test]
    fn test_contains_exact_match() {
        let mut prefix_set_mut = PrefixSetMut::default();
        prefix_set_mut.insert(Nibbles::from_nibbles([0xa, 0xb, 0xc]));
        let prefix_set = prefix_set_mut.freeze();

        assert!(matches!(prefix_set.inner, PrefixSetInner::One(_)));
        assert!(prefix_set.contains(&Nibbles::from_nibbles_unchecked([0xa, 0xb, 0xc])));
        assert!(prefix_set.contains(&Nibbles::from_nibbles_unchecked([0xa, 0xb])));
        assert!(prefix_set.contains(&Nibbles::from_nibbles_unchecked([0xa])));
        assert!(prefix_set.contains(&Nibbles::default()));
        assert!(!prefix_set.contains(&Nibbles::from_nibbles_unchecked([0xa, 0xb, 0xc, 0xd])));
        assert!(!prefix_set.contains(&Nibbles::from_nibbles_unchecked([0xa, 0xc])));
    }

    #[test]
    fn test_contains_shared_prefix() {
        let mut prefix_set_mut = PrefixSetMut::default();
        prefix_set_mut.insert(Nibbles::from_nibbles([1, 2, 3, 4]));
        prefix_set_mut.insert(Nibbles::from_nibbles([1, 2, 5, 6]));
        let prefix_set = prefix_set_mut.freeze();

        assert!(prefix_set.contains(&Nibbles::from_nibbles_unchecked([1, 2])));
        assert!(prefix_set.contains(&Nibbles::from_nibbles_unchecked([1])));
        assert!(!prefix_set.contains(&Nibbles::from_nibbles_unchecked([1, 3])));
        assert!(prefix_set.contains(&Nibbles::from_nibbles_unchecked([1, 2, 3])));
        assert!(prefix_set.contains(&Nibbles::from_nibbles_unchecked([1, 2, 5])));
        assert!(!prefix_set.contains(&Nibbles::from_nibbles_unchecked([1, 2, 6])));
    }

    #[test]
    fn test_contains_variable_length_keys() {
        let mut prefix_set_mut = PrefixSetMut::default();
        prefix_set_mut.insert(Nibbles::from_nibbles([1, 2]));
        prefix_set_mut.insert(Nibbles::from_nibbles([1, 2, 3, 4]));
        let prefix_set = prefix_set_mut.freeze();

        assert!(prefix_set.contains(&Nibbles::from_nibbles_unchecked([1, 2])));
        assert!(prefix_set.contains(&Nibbles::from_nibbles_unchecked([1])));
        assert!(prefix_set.contains(&Nibbles::from_nibbles_unchecked([1, 2, 3])));
        assert!(prefix_set.contains(&Nibbles::from_nibbles_unchecked([1, 2, 3, 4])));
        assert!(!prefix_set.contains(&Nibbles::from_nibbles_unchecked([1, 2, 5])));
        assert!(!prefix_set.contains(&Nibbles::from_nibbles_unchecked([1, 2, 3, 4, 5])));
    }

    #[test]
    fn test_single_key() {
        let mut prefix_set_mut = PrefixSetMut::default();
        prefix_set_mut.insert(Nibbles::from_nibbles([0xa, 0xb, 0xc, 0xd]));
        let prefix_set = prefix_set_mut.freeze();

        assert!(matches!(prefix_set.inner, PrefixSetInner::One(_)));
        assert!(prefix_set.contains(&Nibbles::from_nibbles_unchecked([0xa])));
        assert!(prefix_set.contains(&Nibbles::from_nibbles_unchecked([0xa, 0xb, 0xc, 0xd])));
        assert!(!prefix_set.contains(&Nibbles::from_nibbles_unchecked([0xa, 0xb, 0xc, 0xe])));
        assert!(!prefix_set.contains(&Nibbles::from_nibbles_unchecked([0xb])));
    }

    #[test]
    fn test_empty_set() {
        let prefix_set_mut = PrefixSetMut::default();
        let prefix_set = prefix_set_mut.freeze();

        assert!(matches!(prefix_set.inner, PrefixSetInner::Empty));
        assert!(!prefix_set.contains(&Nibbles::default()));
        assert!(!prefix_set.contains(&Nibbles::from_nibbles_unchecked([1])));
    }

    #[test]
    fn test_all_flag() {
        let prefix_set = PrefixSetMut::all().freeze();

        assert!(matches!(prefix_set.inner, PrefixSetInner::All));
        assert!(prefix_set.contains(&Nibbles::default()));
        assert!(prefix_set.contains(&Nibbles::from_nibbles_unchecked([1, 2, 3])));
        assert!(prefix_set.all());
    }

    #[test]
    fn test_many_keys_all_nibbles() {
        let mut prefix_set_mut = PrefixSetMut::default();
        for n in 0u8..16 {
            prefix_set_mut.insert(Nibbles::from_nibbles([n, 0, 0]));
        }
        let prefix_set = prefix_set_mut.freeze();

        for n in 0u8..16 {
            assert!(prefix_set.contains(&Nibbles::from_nibbles_unchecked([n])));
        }
        assert_eq!(prefix_set.len(), 16);
    }

    #[test]
    fn test_iter_returns_sorted_keys() {
        let mut prefix_set_mut = PrefixSetMut::default();
        prefix_set_mut.insert(Nibbles::from_nibbles([4, 5, 6]));
        prefix_set_mut.insert(Nibbles::from_nibbles([1, 2, 3]));
        prefix_set_mut.insert(Nibbles::from_nibbles([7, 8, 9]));
        let prefix_set = prefix_set_mut.freeze();

        let keys: Vec<_> = prefix_set.iter().collect();
        assert_eq!(keys.len(), 3);
        assert!(keys[0] < keys[1]);
        assert!(keys[1] < keys[2]);
    }

    #[test]
    fn test_variant_selection() {
        // 0 keys → Empty
        let ps = PrefixSetMut::default().freeze();
        assert!(matches!(ps.inner, PrefixSetInner::Empty));

        // 1 key → One
        let mut psm = PrefixSetMut::default();
        psm.insert(Nibbles::from_nibbles([1]));
        let ps = psm.freeze();
        assert!(matches!(ps.inner, PrefixSetInner::One(_)));

        // 2 keys → Small
        let mut psm = PrefixSetMut::default();
        psm.insert(Nibbles::from_nibbles([1]));
        psm.insert(Nibbles::from_nibbles([2]));
        let ps = psm.freeze();
        assert!(matches!(ps.inner, PrefixSetInner::Small(_)));

        // SMALL_SET_THRESHOLD keys → Small
        let mut psm = PrefixSetMut::default();
        for i in 0..SMALL_SET_THRESHOLD {
            psm.insert(Nibbles::from_nibbles([i as u8 % 16, i as u8 / 16, 0]));
        }
        let ps = psm.freeze();
        assert!(matches!(ps.inner, PrefixSetInner::Small(_)));

        // SMALL_SET_THRESHOLD + 1 keys → Trie
        let mut psm = PrefixSetMut::default();
        for i in 0..=SMALL_SET_THRESHOLD {
            psm.insert(Nibbles::from_nibbles([i as u8 % 16, i as u8 / 16, 0]));
        }
        let ps = psm.freeze();
        assert!(matches!(ps.inner, PrefixSetInner::Trie { .. }));
    }

    #[test]
    fn test_trie_variant_contains() {
        // Force Trie variant with > SMALL_SET_THRESHOLD keys using two-nibble encoding
        let mut psm = PrefixSetMut::default();
        let mut inserted = Vec::new();
        for i in 0..=SMALL_SET_THRESHOLD {
            let lo = i as u8 % 16;
            let hi = i as u8 / 16;
            psm.insert(Nibbles::from_nibbles([lo, hi, lo]));
            inserted.push((lo, hi));
        }
        let ps = psm.freeze();
        assert!(matches!(ps.inner, PrefixSetInner::Trie { .. }));

        for &(lo, hi) in &inserted {
            assert!(ps.contains(&Nibbles::from_nibbles_unchecked([lo])));
            assert!(ps.contains(&Nibbles::from_nibbles_unchecked([lo, hi])));
            assert!(ps.contains(&Nibbles::from_nibbles_unchecked([lo, hi, lo])));
        }
        // Non-existent
        assert!(!ps.contains(&Nibbles::from_nibbles_unchecked([0, 0, 1])));
    }

    #[test]
    fn test_one_key_iter() {
        let mut psm = PrefixSetMut::default();
        psm.insert(Nibbles::from_nibbles([1, 2, 3]));
        let ps = psm.freeze();

        let keys: Vec<_> = ps.iter().collect();
        assert_eq!(keys.len(), 1);
        assert_eq!(*keys[0], Nibbles::from_nibbles([1, 2, 3]));
    }

    #[test]
    fn test_clone_and_iterate() {
        let mut psm = PrefixSetMut::default();
        psm.insert(Nibbles::from_nibbles([1, 2]));
        psm.insert(Nibbles::from_nibbles([3, 4]));
        let ps = psm.freeze();
        let ps2 = ps.clone();

        // Both should iterate identically
        let k1: Vec<_> = ps.iter().collect();
        let k2: Vec<_> = ps2.iter().collect();
        assert_eq!(k1, k2);

        // Both should answer contains identically
        assert!(ps2.contains(&Nibbles::from_nibbles_unchecked([1])));
        assert!(!ps2.contains(&Nibbles::from_nibbles_unchecked([2])));
    }
}
