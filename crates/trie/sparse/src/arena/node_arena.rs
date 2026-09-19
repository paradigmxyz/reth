use super::ArenaSparseNode;
use alloc::vec::Vec;
use core::{
    fmt,
    ops::{Index as IndexOps, IndexMut},
};
use reth_trie_common::RlpNode;

/// Bits per word of a [`BlindedMarks`] bitmap.
const WORD_BITS: usize = u64::BITS as usize;

/// Arena of trie nodes addressed by an [`Index`].
///
/// Nodes live in a flat vector; [`NodeArena::remove`] leaves an [`ArenaSparseNode::Free`] tombstone
/// behind and pushes the slot onto a LIFO free list. Unlike a slotmap there are no generation
/// counters, so an index handed out again refers to the new occupant: an index must not be reused
/// across a removal unless the caller knows nothing was inserted in between.
#[derive(Debug, Clone, Default)]
pub(super) struct NodeArena {
    nodes: Vec<ArenaSparseNode>,
    free: Vec<Index>,
    blinded: Vec<RlpNode>,
    blinded_free: Vec<u32>,
}

impl NodeArena {
    /// Creates an empty arena.
    pub(super) const fn new() -> Self {
        Self { nodes: Vec::new(), free: Vec::new(), blinded: Vec::new(), blinded_free: Vec::new() }
    }

    /// Creates an empty arena with room for `capacity` nodes.
    pub(super) fn with_capacity(capacity: usize) -> Self {
        Self { nodes: Vec::with_capacity(capacity), ..Self::new() }
    }

    /// Inserts a node, reusing a free slot when one is available, and returns its index.
    pub(super) fn insert(&mut self, node: ArenaSparseNode) -> Index {
        if let Some(idx) = self.free.pop() {
            self.nodes[idx.get()] = node;
            return idx;
        }
        let idx = Index::new(self.nodes.len());
        self.nodes.push(node);
        idx
    }

    /// Removes the node at `idx` without recycling its slot.
    ///
    /// For an arena that is being emptied into a fresh one — pruning and compaction both do that
    /// and then drop the source — maintaining the free list is wasted work. [`Self::len`] is no
    /// longer accurate afterwards.
    ///
    /// # Panics
    ///
    /// Panics if the slot is out of bounds or free.
    pub(super) fn drain_node(&mut self, idx: Index) -> ArenaSparseNode {
        let node = core::mem::replace(&mut self.nodes[idx.get()], ArenaSparseNode::Free);
        assert!(!matches!(node, ArenaSparseNode::Free), "drained a free arena slot");
        node
    }

    /// Removes the node at `idx`, returning it if the slot was occupied.
    pub(super) fn remove(&mut self, idx: Index) -> Option<ArenaSparseNode> {
        let slot = self.nodes.get_mut(idx.get())?;
        if matches!(slot, ArenaSparseNode::Free) {
            return None;
        }
        let node = core::mem::replace(slot, ArenaSparseNode::Free);
        self.free.push(idx);
        Some(node)
    }

    /// Returns the node at `idx`, or `None` if the slot is out of bounds or free.
    pub(super) fn get(&self, idx: Index) -> Option<&ArenaSparseNode> {
        match self.nodes.get(idx.get()) {
            Some(ArenaSparseNode::Free) | None => None,
            Some(node) => Some(node),
        }
    }

    /// Returns `true` if `idx` refers to an occupied slot, or `false` if it is out of bounds or
    /// free.
    pub(super) fn contains_key(&self, idx: Index) -> bool {
        self.get(idx).is_some()
    }

    /// Returns the number of occupied slots.
    pub(super) const fn len(&self) -> usize {
        self.nodes.len() - self.free.len()
    }

    /// Iterates over the occupied slots.
    pub(super) fn iter(&self) -> impl Iterator<Item = (Index, &ArenaSparseNode)> + '_ {
        self.nodes
            .iter()
            .enumerate()
            .filter(|(_, node)| !matches!(node, ArenaSparseNode::Free))
            .map(|(idx, node)| (Index(idx as u32), node))
    }

    /// Iterates mutably over the occupied slots.
    pub(super) fn iter_mut(&mut self) -> impl Iterator<Item = (Index, &mut ArenaSparseNode)> + '_ {
        self.nodes
            .iter_mut()
            .enumerate()
            .filter(|(_, node)| !matches!(node, ArenaSparseNode::Free))
            .map(|(idx, node)| (Index(idx as u32), node))
    }

    /// Stores the RLP of an unrevealed child and returns a [`BranchChild`] referencing it.
    pub(super) fn insert_blinded(&mut self, rlp: RlpNode) -> BranchChild {
        let slot = if let Some(slot) = self.blinded_free.pop() {
            self.blinded[slot as usize] = rlp;
            slot
        } else {
            let slot = self.blinded.len();
            assert!(slot < BranchChild::BLINDED as usize, "blinded slot overflows the tag bit");
            self.blinded.push(rlp);
            slot as u32
        };
        BranchChild::blinded(slot)
    }

    /// Returns the RLP of a blinded child.
    ///
    /// # Panics
    ///
    /// Panics if `child` is revealed.
    pub(super) fn blinded(&self, child: BranchChild) -> &RlpNode {
        &self.blinded[child.blinded_slot().expect("child is revealed") as usize]
    }

    /// Takes the RLP of a blinded child, releasing its slot.
    ///
    /// # Panics
    ///
    /// Panics if `child` is revealed.
    pub(super) fn take_blinded(&mut self, child: BranchChild) -> RlpNode {
        let slot = child.blinded_slot().expect("child is revealed");
        self.blinded_free.push(slot);
        core::mem::take(&mut self.blinded[slot as usize])
    }

    /// Releases the node vector's unused capacity.
    pub(super) fn shrink_nodes_to_fit(&mut self) {
        self.nodes.shrink_to_fit();
    }

    /// Takes over `src`'s side table of blinded children, so that blinded [`BranchChild`]s copied
    /// out of `src` stay valid without their RLP being moved one slot at a time.
    ///
    /// The copy must record every child it carries over in the returned marks and finish with
    /// [`Self::sweep_blinded`], which reclaims unreferenced slots and may rewrite the blinded
    /// children stored in this arena. Blinded handles must not be retained across the sweep.
    pub(super) fn adopt_blinded(&mut self, src: &mut Self) -> BlindedMarks {
        debug_assert!(self.blinded.is_empty(), "adopting into a non-empty side table");
        self.blinded = core::mem::take(&mut src.blinded);
        self.blinded_free = core::mem::take(&mut src.blinded_free);
        BlindedMarks { words: alloc::vec![0; self.blinded.len().div_ceil(WORD_BITS)] }
    }

    /// Reclaims unmarked slots in the side table adopted by [`Self::adopt_blinded`].
    /// Sparse tables are compacted and their branch child references rewritten, so a live tail
    /// slot cannot retain an arbitrarily large table. Other tables keep their slot indices.
    pub(super) fn sweep_blinded(&mut self, marks: BlindedMarks) {
        let words = marks.words;
        let end = words
            .iter()
            .rposition(|word| *word != 0)
            .map_or(0, |idx| idx * WORD_BITS + WORD_BITS - words[idx].leading_zeros() as usize);
        debug_assert!(end <= self.blinded.len(), "marked slot outside the side table");
        self.blinded.truncate(end);
        self.blinded_free.clear();

        let words = &words[..end.div_ceil(WORD_BITS)];
        let live_count = words.iter().map(|word| word.count_ones() as usize).sum::<usize>();
        if end > 0 && live_count <= end / 4 {
            // A slot's new index is the number of marked slots before it. Store one prefix
            // count per bitmap word instead of allocating a mapping for every old slot.
            let mut offsets = Vec::with_capacity(words.len());
            let mut next = 0;
            for (idx, &word) in words.iter().enumerate() {
                offsets.push(next as u32);
                let mut marked = word;
                while marked != 0 {
                    let slot = idx * WORD_BITS + marked.trailing_zeros() as usize;
                    self.blinded.swap(next, slot);
                    next += 1;
                    marked &= marked - 1;
                }
            }
            self.blinded.truncate(next);
            for (_, node) in self.iter_mut() {
                if let ArenaSparseNode::Branch(branch) = node {
                    for child in &mut branch.children {
                        if let Some(slot) = child.blinded_slot() {
                            let word = slot as usize / WORD_BITS;
                            let bit = slot as usize % WORD_BITS;
                            debug_assert_ne!(words[word] & (1 << bit), 0, "unmarked child");
                            let preceding = words[word] & ((1 << bit) - 1);
                            *child = BranchChild::blinded(offsets[word] + preceding.count_ones());
                        }
                    }
                }
            }
            shrink_excess_capacity(&mut self.blinded);
            shrink_excess_capacity(&mut self.blinded_free);
            return;
        }

        // Push high to low so that `insert_blinded` hands the lowest slot out first and the tail
        // of the table keeps draining.
        let word_count = end.div_ceil(WORD_BITS);
        for (idx, word) in words[..word_count].iter().enumerate().rev() {
            // The last word covers only the slots below `end`.
            let covered = if idx + 1 == word_count && end % WORD_BITS != 0 {
                (1 << (end % WORD_BITS)) - 1
            } else {
                u64::MAX
            };
            let mut unmarked = !word & covered;
            while unmarked != 0 {
                let bit = u64::BITS - 1 - unmarked.leading_zeros();
                unmarked ^= 1 << bit;
                self.blinded_free.push(idx as u32 * u64::BITS + bit);
            }
        }

        shrink_excess_capacity(&mut self.blinded);
        shrink_excess_capacity(&mut self.blinded_free);
    }
}

impl IndexOps<Index> for NodeArena {
    type Output = ArenaSparseNode;

    #[inline]
    fn index(&self, idx: Index) -> &Self::Output {
        let node = &self.nodes[idx.get()];
        assert!(!matches!(node, ArenaSparseNode::Free), "indexed a free arena slot");
        node
    }
}

impl IndexMut<Index> for NodeArena {
    #[inline]
    fn index_mut(&mut self, idx: Index) -> &mut Self::Output {
        let node = &mut self.nodes[idx.get()];
        assert!(!matches!(node, ArenaSparseNode::Free), "indexed a free arena slot");
        node
    }
}

/// A reference to a node inside a [`NodeArena`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(super) struct Index(u32);

impl Index {
    /// Creates an index that fits below the blinded-child tag bit.
    ///
    /// # Panics
    ///
    /// Panics if `index` is at least 2^31.
    pub(super) const fn new(index: usize) -> Self {
        assert!(index < BranchChild::BLINDED as usize, "arena index overflows the blinded tag bit");
        Self(index as u32)
    }

    /// Returns the index as a `usize`, suitable for indexing into the arena's backing vector.
    pub(super) const fn get(self) -> usize {
        self.0 as usize
    }
}

/// The slots of a side table of blinded children that a copy into a fresh arena carried over.
///
/// See [`NodeArena::adopt_blinded`].
#[must_use = "adopted blinded children must be marked and swept"]
pub(super) struct BlindedMarks {
    words: Vec<u64>,
}

impl BlindedMarks {
    /// Records that `child`'s slot is still referenced.
    ///
    /// # Panics
    ///
    /// Panics if `child` is revealed.
    pub(super) fn mark(&mut self, child: BranchChild) {
        let slot = child.blinded_slot().expect("child is revealed") as usize;
        let idx = slot / WORD_BITS;
        if idx >= self.words.len() {
            self.words.resize(idx + 1, 0);
        }
        self.words[idx] |= 1 << (slot % WORD_BITS);
    }
}

/// A reference from a branch node to one of its children.
///
/// Packs the two cases into a single `u32`: a revealed child holds an [`Index`] into the arena's
/// nodes, a blinded child a slot in the arena's side table of unrevealed RLP. The top bit
/// discriminates, so both fit in four bytes and a branch's children stay dense and small.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct BranchChild(u32);

impl BranchChild {
    /// Tag bit marking a child as blinded.
    const BLINDED: u32 = 1 << 31;

    /// Returns a child referencing the revealed node at `idx`.
    ///
    /// # Panics
    ///
    /// Panics if `idx` is at least 2^31.
    pub(super) const fn revealed(idx: Index) -> Self {
        assert!(idx.0 & Self::BLINDED == 0, "arena index overflows the blinded tag bit");
        Self(idx.0)
    }

    /// Returns a child referencing the blinded RLP at `slot`.
    ///
    /// # Panics
    ///
    /// Panics if `slot` is at least 2^31.
    const fn blinded(slot: u32) -> Self {
        assert!(slot & Self::BLINDED == 0, "blinded slot overflows the tag bit");
        Self(slot | Self::BLINDED)
    }

    /// Returns `true` if this child reference is blinded (not yet revealed in the arena).
    pub(super) const fn is_blinded(self) -> bool {
        self.0 & Self::BLINDED != 0
    }

    /// Returns the arena index of a revealed child, or `None` if it is blinded.
    pub(super) const fn revealed_index(self) -> Option<Index> {
        if self.is_blinded() {
            None
        } else {
            Some(Index(self.0))
        }
    }

    /// Returns the side-table slot of a blinded child, or `None` if it is revealed.
    const fn blinded_slot(self) -> Option<u32> {
        if self.is_blinded() {
            Some(self.0 & !Self::BLINDED)
        } else {
            None
        }
    }
}

impl fmt::Debug for BranchChild {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.blinded_slot() {
            Some(slot) => write!(f, "Blinded({slot})"),
            None => write!(f, "Revealed({})", self.0),
        }
    }
}

/// Reclaims capacity after a large contraction while leaving room for subsequent growth.
fn shrink_excess_capacity<T>(vec: &mut Vec<T>) {
    // Shrink at quarter-full to avoid reallocating on small prunes. Empty vectors release all
    // capacity; non-empty vectors retain room for twice their current length.
    if vec.len() <= vec.capacity() / 4 {
        vec.shrink_to(vec.len() * 2);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arena::{ArenaSparseNodeBranch, ArenaSparseNodeState};
    use alloy_primitives::B256;

    #[test]
    fn removed_slots_are_recycled() {
        let mut arena = NodeArena::new();
        let first = arena.insert(ArenaSparseNode::TakenSubtrie);
        let second = arena.insert(ArenaSparseNode::TakenSubtrie);
        assert_eq!(arena.len(), 2);

        assert!(arena.remove(first).is_some());
        assert!(arena.get(first).is_none());
        assert!(!arena.contains_key(first));
        assert!(arena.remove(first).is_none());
        assert_eq!(arena.len(), 1);

        let third = arena.insert(ArenaSparseNode::TakenSubtrie);
        assert_eq!(third, first, "the freed slot is handed out again");
        assert_ne!(third, second);
        assert_eq!(arena.len(), 2);
        assert_eq!(arena.iter().count(), 2);
    }

    #[test]
    fn contains_key_excludes_free_slots() {
        let mut arena = NodeArena::new();
        let removed = arena.insert(ArenaSparseNode::TakenSubtrie);
        let drained = arena.insert(ArenaSparseNode::TakenSubtrie);
        assert!(arena.contains_key(removed));
        assert!(arena.contains_key(drained));
        assert!(!arena.contains_key(Index::new(2)));

        arena.remove(removed);
        arena.drain_node(drained);
        assert!(!arena.contains_key(removed));
        assert!(!arena.contains_key(drained));
    }

    #[test]
    #[should_panic(expected = "indexed a free arena slot")]
    fn indexing_a_free_slot_panics() {
        let mut arena = NodeArena::new();
        let idx = arena.insert(ArenaSparseNode::TakenSubtrie);
        arena.remove(idx);
        let _ = &arena[idx];
    }

    #[test]
    #[should_panic(expected = "indexed a free arena slot")]
    fn mutably_indexing_a_free_slot_panics() {
        let mut arena = NodeArena::new();
        let idx = arena.insert(ArenaSparseNode::TakenSubtrie);
        arena.remove(idx);
        arena[idx] = ArenaSparseNode::TakenSubtrie;
    }

    #[test]
    #[should_panic(expected = "drained a free arena slot")]
    fn draining_a_free_slot_panics() {
        let mut arena = NodeArena::new();
        let idx = arena.insert(ArenaSparseNode::TakenSubtrie);
        arena.drain_node(idx);
        arena.drain_node(idx);
    }

    #[test]
    fn branch_child_preserves_largest_index() {
        let index = Index::new((1 << 31) - 1);
        assert_eq!(BranchChild::revealed(index).revealed_index(), Some(index));
        assert_eq!(BranchChild::blinded(index.0).blinded_slot(), Some(index.0));
    }

    #[test]
    #[should_panic(expected = "arena index overflows the blinded tag bit")]
    fn revealed_child_rejects_blinded_tag_bit() {
        BranchChild::revealed(Index(1 << 31));
    }

    #[test]
    #[should_panic(expected = "blinded slot overflows the tag bit")]
    fn blinded_child_rejects_blinded_tag_bit() {
        BranchChild::blinded(1 << 31);
    }

    #[test]
    #[should_panic(expected = "arena index overflows the blinded tag bit")]
    fn arena_index_rejects_blinded_tag_bit() {
        Index::new(1 << 31);
    }

    #[test]
    fn blinded_slots_are_recycled() {
        let mut arena = NodeArena::new();
        let rlp = RlpNode::word_rlp(&B256::repeat_byte(1));
        let child = arena.insert_blinded(rlp.clone());
        assert!(child.is_blinded());
        assert_eq!(child.revealed_index(), None);
        assert_eq!(arena.blinded(child), &rlp);

        assert_eq!(arena.take_blinded(child), rlp);
        let other = RlpNode::word_rlp(&B256::repeat_byte(2));
        assert_eq!(arena.insert_blinded(other.clone()), child, "the freed slot is reused");
        assert_eq!(arena.blinded(child), &other);
    }

    #[test]
    fn sweeping_an_adopted_table_keeps_marked_slots() {
        let mut src = NodeArena::new();
        // More than one word of the mark bitmap.
        let children: Vec<_> = (0..200u8)
            .map(|byte| src.insert_blinded(RlpNode::word_rlp(&B256::repeat_byte(byte))))
            .collect();

        let mut dst = NodeArena::new();
        let mut marks = dst.adopt_blinded(&mut src);
        for child in children.iter().step_by(3) {
            marks.mark(*child);
        }
        dst.sweep_blinded(marks);

        let last_marked = (children.len() - 1) / 3 * 3;
        assert_eq!(dst.blinded.len(), last_marked + 1, "the unmarked tail is dropped");

        // Every unmarked slot below the tail is handed out again, lowest first.
        let expected: Vec<u32> = (0..=last_marked as u32).filter(|slot| slot % 3 != 0).collect();
        let handed_out: Vec<u32> = expected
            .iter()
            .map(|_| dst.insert_blinded(RlpNode::default()).blinded_slot().unwrap())
            .collect();
        assert_eq!(handed_out, expected);

        for (byte, child) in (0..200u8).step_by(3).zip(children.iter().step_by(3)) {
            assert_eq!(dst.blinded(*child), &RlpNode::word_rlp(&B256::repeat_byte(byte)));
        }
    }

    #[test]
    fn sweeping_releases_excess_side_table_capacity() {
        let mut src = NodeArena::new();
        let rlp = RlpNode::word_rlp(&B256::repeat_byte(1));
        let children: Vec<_> = (0..4096).map(|_| src.insert_blinded(rlp.clone())).collect();
        for child in &children[2048..] {
            src.take_blinded(*child);
        }

        let mut dst = NodeArena::new();
        let mut marks = dst.adopt_blinded(&mut src);
        for child in children[..64].iter().step_by(2) {
            marks.mark(*child);
        }
        dst.sweep_blinded(marks);

        assert!(dst.blinded.capacity() <= 128, "release the discarded RLP capacity");
        assert!(dst.blinded_free.capacity() <= 64, "release the discarded free-list capacity");
        for child in children[..64].iter().step_by(2) {
            assert_eq!(dst.blinded(*child), &rlp);
        }
        for slot in (1..63).step_by(2) {
            assert_eq!(dst.insert_blinded(rlp.clone()).blinded_slot(), Some(slot));
        }

        let mut empty = NodeArena::new();
        let marks = empty.adopt_blinded(&mut dst);
        empty.sweep_blinded(marks);
        assert_eq!(empty.blinded.capacity(), 0);
        assert_eq!(empty.blinded_free.capacity(), 0);
    }

    #[test]
    fn sweeping_keeps_capacity_for_small_contractions() {
        let mut src = NodeArena::new();
        let children: Vec<_> = (0..128).map(|_| src.insert_blinded(RlpNode::default())).collect();
        let capacity = src.blinded.capacity();
        let mut dst = NodeArena::new();
        let mut marks = dst.adopt_blinded(&mut src);
        for child in &children[..96] {
            marks.mark(*child);
        }
        dst.sweep_blinded(marks);
        assert_eq!(dst.blinded.capacity(), capacity);
    }

    #[test]
    fn sweeping_compacts_a_live_replacement_at_the_tail() {
        let mut src = NodeArena::new();
        for _ in 0..65_536 {
            src.insert_blinded(RlpNode::default());
        }
        let mut dst = NodeArena::new();
        let mut marks = dst.adopt_blinded(&mut src);
        let rlp = RlpNode::word_rlp(&B256::repeat_byte(1));
        let replacement = dst.insert_blinded(rlp.clone());
        assert_eq!(replacement.blinded_slot(), Some(65_536));
        let root = dst.insert(branch([replacement]));
        marks.mark(replacement);
        dst.sweep_blinded(marks);

        let child = dst[root].branch_ref().children[0];
        assert_eq!(dst.blinded(child), &rlp);
        assert_eq!(child.blinded_slot(), Some(0));
        assert_eq!(dst.blinded.len(), 1);
        assert!(dst.blinded.capacity() <= 2);
        assert_eq!(dst.blinded_free.capacity(), 0);

        // Subsequent sweeps visit only the compacted table, not the old high-water mark.
        let mut next = NodeArena::new();
        let mut marks = next.adopt_blinded(&mut dst);
        assert_eq!(marks.words.len(), 1);
        marks.mark(child);
        next.insert(dst.drain_node(root));
        next.sweep_blinded(marks);
        assert_eq!(next.blinded(child), &rlp);
        assert_eq!(next.blinded.len(), 1);
    }

    #[test]
    fn sweeping_rewrites_sparse_slots_across_bitmap_words() {
        let mut src = NodeArena::new();
        let children: Vec<_> = (0..200u8)
            .map(|byte| src.insert_blinded(RlpNode::word_rlp(&B256::repeat_byte(byte))))
            .collect();
        let mut dst = NodeArena::new();
        let mut marks = dst.adopt_blinded(&mut src);
        let kept = [199, 64, 0, 130, 63];
        for slot in kept {
            marks.mark(children[slot]);
        }
        let leaf = dst.insert(ArenaSparseNode::TakenSubtrie);
        let root = dst.insert(branch(
            kept.map(|slot| children[slot]).into_iter().chain([BranchChild::revealed(leaf)]),
        ));
        dst.sweep_blinded(marks);

        assert_eq!(dst.blinded.len(), kept.len());
        assert!(dst.blinded.capacity() <= kept.len() * 2);
        assert_eq!(dst.blinded_free.capacity(), 0);
        let branch = dst[root].branch_ref();
        for (pos, slot) in kept.into_iter().enumerate() {
            assert_eq!(
                dst.blinded(branch.children[pos]),
                &RlpNode::word_rlp(&B256::repeat_byte(slot as u8))
            );
        }
        assert_eq!(branch.children[kept.len()].revealed_index(), Some(leaf));
        assert!(dst.contains_key(leaf));
        assert_eq!(dst.insert_blinded(RlpNode::default()).blinded_slot(), Some(5));
    }

    fn branch(children: impl IntoIterator<Item = BranchChild>) -> ArenaSparseNode {
        let children: smallvec::SmallVec<_> = children.into_iter().collect();
        ArenaSparseNode::Branch(ArenaSparseNodeBranch {
            state_mask: alloy_trie::TrieMask::new((1 << children.len()) - 1),
            children,
            state: ArenaSparseNodeState::Revealed,
            short_key: Default::default(),
            branch_masks: Default::default(),
        })
    }
}
