use alloy_primitives::B256;
use reth_trie_common::Nibbles;

use crate::proof_v2::increment_and_strip_trailing_zeros;

/// Target describes a proof target. For every proof target given, the
/// [`crate::proof_v2::ProofCalculator`] will calculate and return all nodes whose path is a prefix
/// of the target's `key`.
#[derive(Debug, Copy, Clone)]
pub struct Target {
    pub(crate) key: Nibbles,
    /// The lower bound of the range of trie nodes which can be retained by this target. In other
    /// words, the shortest trie node path which can be retained by this target.
    pub(crate) lower_bound: Nibbles,
}

impl Target {
    /// Returns a new [`Target`] which matches all trie nodes whose path is a prefix of this key.
    pub fn new(key: B256) -> Self {
        // SAFETY: key is a B256 and so is exactly 32-bytes.
        let key = unsafe { Nibbles::unpack_unchecked(key.as_slice()) };
        Self { key, lower_bound: Nibbles::new() }
    }

    /// Only match trie nodes whose path is at least this long.
    ///
    /// # Panics
    ///
    /// This method panics if `min_len` is greater than 64.
    pub fn with_min_len(mut self, min_len: u8) -> Self {
        debug_assert!(min_len <= 64);
        self.lower_bound = self.key;
        self.lower_bound.truncate(64 - min_len as usize);
        self
    }

    /// Returns the exclusive upper bound of the range of possible trie nodes which can be retained
    /// by this target, or None for unbounded.
    fn upper_bound(&self) -> Option<Nibbles> {
        increment_and_strip_trailing_zeros(&self.lower_bound)
    }
}

impl From<B256> for Target {
    fn from(key: B256) -> Self {
        Self::new(key)
    }
}

/// Describes a set of targets which all apply to a single sub-trie, ie a section of the overall
/// trie whose nodes all share a prefix.
pub(crate) struct SubTrieTargets<'a> {
    /// The lower bound of the sub-trie, ie the prefix which all nodes in the sub-trie share.
    pub(crate) lower_bound: Nibbles,
    /// The exclusive upper bound of the sub-trie, or None if unbounded. This will be the first
    /// path in lexicographical order which is not contained by the sub-trie.
    pub(crate) upper_bound: Option<Nibbles>,
    /// The targets belonging to this sub-trie. These will be sorted by their `key` field,
    /// lexicographically.
    pub(crate) targets: &'a [Target],
}

/// Given a set of [`Target`]s, returns an iterator over those same [`Target`]s chunked by the
/// sub-tries they apply to within the overall trie.
pub fn sub_trie_targets<'a>(targets: &'a mut [Target]) -> impl Iterator<Item = SubTrieTargets<'a>> {
    // First sort lexicographically by lower bound. We will use this for chunking targets into
    // contiguous sections in the next steps based on their bounds.
    targets.sort_unstable_by_key(|target| target.lower_bound);

    // We now chunk targets, such that each chunk contains all targets belonging to the same
    // sub-trie. We are taking advantage of the following properties:
    //
    // - The first target in the chunk has the lowest lower bound (see previous sorting step).
    //
    // - The first target in the chunk's upper bound will therefore be the highest upper bound, and
    //   the upper bound of the whole chunk.
    //      - For example, given a chunk with lower bounds [0x2, 0x2f, 0x2fa], the upper bounds will
    //        be [0x3, 0x3, 0x2fb]. Note that no target could match a trie node with path equal to
    //        or greater than 0x3.
    //
    // - If a target's lower bound does not lie within the bounds of the current chunk, then that
    //   target must be the first target of the next chunk, covering a separate sub-trie.
    //      - Example: given lower bounds of [0x2, 0x2fa, 0x4c, 0x4ce, 0x4e], we would end up with
    //        the following chunks:
    //          - [0x2, 0x2a]  w/ upper bound 0x3
    //          - [0x4c 0x4ce] w/ upper bound 0x4d
    //          - [0x4e]       w/ upper bound 0x4f
    let mut upper_bound = targets.first().and_then(|t| t.upper_bound());
    let target_chunks = targets.chunk_by_mut(move |_, next| {
        if let Some(some_upper_bound) = upper_bound {
            let same_chunk = next.lower_bound < some_upper_bound;
            if !same_chunk {
                upper_bound = next.upper_bound();
            }
            same_chunk
        } else {
            true
        }
    });

    // Map the chunks to the return type. Within each chunk we want targets to be sorted by their
    // key, as that will be the order they are checked by the `ProofCalculator`.
    target_chunks.map(|target_chunk| {
        let lower_bound = target_chunk[0].lower_bound;
        let upper_bound = target_chunk[0].upper_bound();
        target_chunk.sort_unstable_by_key(|target| target.key);
        SubTrieTargets { lower_bound, upper_bound, targets: target_chunk }
    })
}
