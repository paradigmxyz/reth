//! Stable node and blinded-child handles and their backing slot tables.

mod slots;

use super::ArenaSparseNode;
use alloc::vec::Vec;
use core::{
    fmt,
    ops::{Index as IndexOps, IndexMut},
};
use reth_trie_common::RlpNode;
use slots::{Slots, MAX_SLOTS};

/// Bits per word of a [`BlindedMarks`] bitmap.
const WORD_BITS: usize = u64::BITS as usize;

/// Arena of trie nodes addressed by an [`Index`].
///
/// Removing a node for reuse leaves an [`ArenaSparseNode::Free`] tombstone and puts its index
/// on a LIFO free list. Indices have no generation counter: a reused index refers to the
/// new occupant and must not be retained across removal and reinsertion.
///
/// Snapshot copies can share slot values, but recycle indices independently. A mutable borrow
/// copies a shared value; use an immutable borrow for reads and discard to recycle a slot
/// without copying its value.
#[derive(Debug, Clone, Default)]
pub(super) struct NodeArena {
    nodes: Slots<ArenaSparseNode>,
    free: Vec<Index>,
    blinded: Slots<Option<RlpNode>>,
    blinded_free: Vec<u32>,
}

impl NodeArena {
    /// Creates an empty arena.
    pub(super) const fn new() -> Self {
        Self {
            nodes: Slots::new(),
            free: Vec::new(),
            blinded: Slots::new(),
            blinded_free: Vec::new(),
        }
    }

    /// Creates an empty arena with room for `capacity` nodes.
    pub(super) fn with_capacity(capacity: usize) -> Self {
        Self { nodes: Slots::with_capacity(capacity), ..Self::new() }
    }

    /// Inserts a node, reusing a free slot when one is available, and returns its index.
    ///
    /// # Panics
    ///
    /// Panics if `node` is a free-slot tombstone or the node table is full.
    pub(super) fn insert(&mut self, node: ArenaSparseNode) -> Index {
        assert!(is_occupied(&node), "cannot insert a free arena slot");
        if let Some(idx) = self.free.pop() {
            self.nodes.set(idx.get(), node);
            return idx;
        }
        // Slots::push enforces the same index bound as Index::new.
        Index(self.nodes.push(node) as u32)
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
        let node = self.nodes.replace(idx.get(), ArenaSparseNode::Free);
        assert!(is_occupied(&node), "drained a free arena slot");
        node
    }

    /// Removes and returns the node at `idx`, recycling its slot.
    /// Returns `None` if the slot is out of bounds or free.
    pub(super) fn remove(&mut self, idx: Index) -> Option<ArenaSparseNode> {
        let node = self.nodes.replace_if(idx.get(), ArenaSparseNode::Free, is_occupied)?;
        self.free.push(idx);
        Some(node)
    }

    /// Recycles an occupied node slot without cloning its value out of shared storage.
    ///
    /// # Panics
    ///
    /// Panics if the slot is out of bounds or free.
    pub(super) fn discard(&mut self, idx: Index) {
        assert!(
            self.nodes.set_if(idx.get(), ArenaSparseNode::Free, is_occupied),
            "cannot discard an unoccupied arena slot"
        );
        self.free.push(idx);
    }

    /// Returns the node at `idx`, or `None` if the slot is out of bounds or free.
    pub(super) fn get(&self, idx: Index) -> Option<&ArenaSparseNode> {
        self.nodes.get(idx.get()).filter(|node| is_occupied(node))
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
            .filter(|(_, node)| is_occupied(node))
            .map(|(idx, node)| (Index(idx as u32), node))
    }

    /// Calls `predicate` for occupied slots in index order, then passes each accepted slot
    /// to `visit` before examining the next one. The visitor must leave the slot occupied.
    pub(super) fn for_each_mut_if(
        &mut self,
        mut predicate: impl FnMut(&ArenaSparseNode) -> bool,
        mut visit: impl FnMut(Index, &mut ArenaSparseNode),
    ) {
        self.nodes.for_each_mut_if(
            |node| is_occupied(node) && predicate(node),
            |index, node| visit(Index(index as u32), node),
        );
    }

    /// Stores the RLP of an unrevealed child and returns a [`BranchChild`] referencing it.
    pub(super) fn insert_blinded(&mut self, rlp: RlpNode) -> BranchChild {
        let slot = if let Some(slot) = self.blinded_free.pop() {
            self.blinded.set(slot as usize, Some(rlp));
            slot
        } else {
            self.blinded.push(Some(rlp)) as u32
        };
        BranchChild::blinded(slot)
    }

    /// Returns the RLP of a blinded child.
    ///
    /// # Panics
    ///
    /// Panics if `child` is revealed or its slot is out of bounds or free.
    pub(super) fn blinded(&self, child: BranchChild) -> &RlpNode {
        self.blinded[child.blinded_slot().expect("child is revealed") as usize]
            .as_ref()
            .expect("indexed a free blinded slot")
    }

    /// Takes the RLP of a blinded child, recycling its slot.
    ///
    /// # Panics
    ///
    /// Panics if `child` is revealed or its slot is out of bounds or free.
    pub(super) fn take_blinded(&mut self, child: BranchChild) -> RlpNode {
        let slot = child.blinded_slot().expect("child is revealed");
        let rlp = self
            .blinded
            .replace_if(slot as usize, None, Option::is_some)
            .flatten()
            .expect("cannot take an unoccupied blinded slot");
        self.blinded_free.push(slot);
        rlp
    }

    /// Recycles a blinded slot without cloning its RLP out of shared storage.
    ///
    /// # Panics
    ///
    /// Panics if `child` is revealed or its slot is out of bounds or free.
    pub(super) fn discard_blinded(&mut self, child: BranchChild) {
        let slot = child.blinded_slot().expect("child is revealed");
        assert!(
            self.blinded.set_if(slot as usize, None, Option::is_some),
            "cannot discard an unoccupied blinded slot"
        );
        self.blinded_free.push(slot);
    }

    /// Releases unused node capacity.
    ///
    /// # Panics
    ///
    /// Panics if the node table is shared.
    pub(super) fn shrink_nodes_to_fit(&mut self) {
        let mut nodes = self.nodes.exclusive_slots().expect("cannot shrink a shared node table");
        let len = nodes.len();
        nodes.shrink_to(len);
    }

    /// Takes over `src`'s side table of blinded children, so that blinded [`BranchChild`]s copied
    /// out of `src` stay valid without their RLP being moved one slot at a time.
    ///
    /// The copy must record every child it carries over in the returned marks and finish with
    /// [`Self::sweep_blinded`], which reclaims unreferenced slots and may rewrite the blinded
    /// children stored in this arena. Blinded handles must not be retained across the sweep.
    ///
    /// # Panics
    ///
    /// Panics if the destination table is not empty or the source table is shared.
    pub(super) fn adopt_blinded(&mut self, src: &mut Self) -> BlindedMarks {
        assert!(self.blinded.is_empty(), "adopting into a non-empty side table");
        assert!(!src.blinded.is_shared(), "cannot adopt a shared blinded table");
        self.blinded = core::mem::take(&mut src.blinded);
        self.blinded_free = core::mem::take(&mut src.blinded_free);
        BlindedMarks { words: alloc::vec![0; self.blinded.len().div_ceil(WORD_BITS)] }
    }

    /// Reclaims unmarked slots in the side table adopted by [`Self::adopt_blinded`].
    /// Sparse tables are compacted and their branch child references rewritten, so a live tail
    /// slot cannot retain an arbitrarily large table. Other tables keep their slot indices.
    ///
    /// # Panics
    ///
    /// Panics if the arena still shares storage or the marks reference an out-of-bounds slot.
    pub(super) fn sweep_blinded(&mut self, marks: BlindedMarks) {
        let words = marks.words;
        let end = words
            .iter()
            .rposition(|word| *word != 0)
            .map_or(0, |idx| idx * WORD_BITS + WORD_BITS - words[idx].leading_zeros() as usize);
        assert!(self.can_compact(), "cannot sweep a shared arena");
        assert!(end <= self.blinded.len(), "marked slot outside the side table");
        let mut blinded =
            self.blinded.exclusive_slots().expect("cannot sweep a shared blinded table");
        blinded.truncate(end);
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
                    blinded.swap(next, slot);
                    next += 1;
                    marked &= marked - 1;
                }
            }
            blinded.truncate(next);
            let mut nodes = self.nodes.exclusive_slots().expect("cannot sweep a shared node table");
            for node in &mut *nodes {
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
        } else {
            // Push high to low so that `insert_blinded` hands the lowest slot out first and the
            // tail of the table keeps draining.
            let word_count = words.len();
            for (idx, word) in words.iter().enumerate().rev() {
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
                    let slot = idx as u32 * u64::BITS + bit;
                    blinded[slot as usize] = None;
                    self.blinded_free.push(slot);
                }
            }
        }
        if let Some(capacity) = retained_capacity(blinded.len(), blinded.capacity()) {
            blinded.shrink_to(capacity);
        }
        if let Some(capacity) =
            retained_capacity(self.blinded_free.len(), self.blinded_free.capacity())
        {
            self.blinded_free.shrink_to(capacity);
        }
    }

    /// Shares unchanged slot values in this arena and any contained lower arenas.
    pub(super) fn snapshot(&mut self) -> Self {
        // Prepare subtries before sharing their enclosing slots. Later copying one of those
        // slots then shares the lower arena instead of cloning all of its nodes.
        self.for_each_mut_if(
            |node| matches!(node, ArenaSparseNode::Subtrie(subtrie) if !subtrie.arena.is_snapshot_ready()),
            |_, node| {
                let ArenaSparseNode::Subtrie(subtrie) = node else { unreachable!() };
                subtrie.arena.prepare_snapshot();
            },
        );
        self.prepare_snapshot();
        self.clone()
    }

    /// Prepares this arena's tables without traversing any subtries.
    fn prepare_snapshot(&mut self) {
        self.nodes.prepare_snapshot();
        self.blinded.prepare_snapshot();
    }

    /// Returns whether both slot tables are ready for a snapshot. A table may be ready even
    /// if no other arena currently shares it.
    const fn is_snapshot_ready(&self) -> bool {
        self.nodes.is_layered() && self.blinded.is_layered()
    }

    /// Returns whether compaction can move values from both slot tables.
    pub(super) fn can_compact(&self) -> bool {
        !self.nodes_mutation_copies() && !self.blinded.is_shared()
    }

    /// Returns whether taking or mutating nodes can require copying their current values.
    pub(super) fn nodes_mutation_copies(&self) -> bool {
        self.nodes.is_shared()
    }

    /// Copies both tables, including vacant slots, without sharing their storage.
    /// `clone_node` controls how nested subtries are copied.
    pub(super) fn deep_clone(
        &self,
        clone_node: impl FnMut(&ArenaSparseNode) -> ArenaSparseNode,
    ) -> Self {
        Self {
            nodes: self.nodes.deep_clone(clone_node),
            free: self.free.clone(),
            blinded: self.blinded.deep_clone(Clone::clone),
            blinded_free: self.blinded_free.clone(),
        }
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
    const BLINDED: u32 = MAX_SLOTS as u32;

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

const fn is_occupied(node: &ArenaSparseNode) -> bool {
    !matches!(node, ArenaSparseNode::Free)
}

/// Returns the capacity to retain after a large contraction.
fn retained_capacity(len: usize, capacity: usize) -> Option<usize> {
    // Shrink at quarter-full to avoid reallocating on small prunes. Empty vectors release all
    // capacity; non-empty vectors retain room for twice their current length.
    (len <= capacity / 4).then(|| len * 2)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arena::{ArenaSparseNodeBranch, ArenaSparseNodeState};
    use alloy_primitives::B256;
    use std::panic::{catch_unwind, AssertUnwindSafe};

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
            branch_masks: Default::default(),
        })
    }

    #[test]
    fn snapshot_readiness_covers_both_tables() {
        let mut original = NodeArena::new();
        let child = original.insert_blinded(RlpNode::default());
        original.prepare_snapshot();
        let mut snapshot = original.clone();
        assert!(snapshot.is_snapshot_ready());
        assert!(!snapshot.can_compact());

        drop(original);
        assert!(snapshot.can_compact());
        assert!(snapshot.is_snapshot_ready());
        assert_eq!(snapshot.take_blinded(child), RlpNode::default());
        assert!(snapshot.nodes.is_layered());
        assert!(!snapshot.blinded.is_layered());
        assert!(!snapshot.is_snapshot_ready());

        snapshot.prepare_snapshot();
        assert!(snapshot.is_snapshot_ready());
        assert!(snapshot.can_compact());
    }

    #[test]
    fn deep_clone_preserves_slots_without_sharing_arenas() {
        let mut source = NodeArena::new();
        let recycled = source.insert(ArenaSparseNode::TakenSubtrie);
        let live = source.insert(ArenaSparseNode::TakenSubtrie);
        source.discard(recycled);
        let recycled_blinded = source.insert_blinded(RlpNode::default());
        let live_blinded = source.insert_blinded(RlpNode::word_rlp(&B256::repeat_byte(1)));
        source.discard_blinded(recycled_blinded);
        source.prepare_snapshot();

        let mut copy = source.deep_clone(Clone::clone);
        assert!(source.can_compact());
        assert!(!copy.is_snapshot_ready());
        assert!(copy.contains_key(live));
        assert_eq!(copy.blinded(live_blinded), source.blinded(live_blinded));
        assert_eq!(copy.insert(ArenaSparseNode::TakenSubtrie), recycled);
        assert_eq!(copy.insert_blinded(RlpNode::default()), recycled_blinded);
        assert!(!source.contains_key(recycled));
        assert_eq!(source.len(), 1);
    }

    #[test]
    fn discarding_an_unoccupied_slot_does_not_change_recycling() {
        let mut original = NodeArena::new();
        let node = original.insert(ArenaSparseNode::TakenSubtrie);
        let child = original.insert_blinded(RlpNode::default());
        original.prepare_snapshot();
        let mut snapshot = original.clone();

        snapshot.discard(node);
        snapshot.discard_blinded(child);
        let node_error = catch_unwind(AssertUnwindSafe(|| snapshot.discard(node))).unwrap_err();
        assert_eq!(
            node_error.downcast_ref::<&str>(),
            Some(&"cannot discard an unoccupied arena slot")
        );
        let child_error =
            catch_unwind(AssertUnwindSafe(|| snapshot.discard_blinded(child))).unwrap_err();
        assert_eq!(
            child_error.downcast_ref::<&str>(),
            Some(&"cannot discard an unoccupied blinded slot")
        );

        assert_eq!(snapshot.len(), 0);
        assert!(snapshot.remove(node).is_none());
        assert_eq!(snapshot.insert(ArenaSparseNode::TakenSubtrie), node);
        assert_ne!(snapshot.insert(ArenaSparseNode::TakenSubtrie), node);
        assert_eq!(snapshot.insert_blinded(RlpNode::default()), child);
        assert_ne!(snapshot.insert_blinded(RlpNode::default()), child);
        assert!(original.contains_key(node));
        assert_eq!(original.blinded(child), &RlpNode::default());
    }

    #[test]
    fn mutation_predicate_only_sees_occupied_slots() {
        let mut original = NodeArena::new();
        let first = original.insert(ArenaSparseNode::TakenSubtrie);
        let second = original.insert(ArenaSparseNode::TakenSubtrie);
        original.discard(first);
        original.prepare_snapshot();
        let mut snapshot = original.clone();

        let mut seen = 0;
        let mut visited = Vec::new();
        snapshot.for_each_mut_if(
            |node| {
                assert!(is_occupied(node));
                seen += 1;
                true
            },
            |index, _| visited.push(index),
        );
        assert_eq!(seen, 1);
        assert_eq!(visited, [second]);
    }
}
