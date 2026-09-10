use super::ArenaSparseNode;
use alloc::vec::Vec;
use core::{
    fmt,
    ops::{Index as IndexOps, IndexMut},
};
use reth_trie_common::RlpNode;

/// A reference to a node inside a [`NodeArena`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(super) struct Index(u32);

impl Index {
    /// Returns the index as a `usize`, suitable for indexing into the arena's backing vector.
    pub(super) const fn get(self) -> usize {
        self.0 as usize
    }
}

/// Arena of trie nodes addressed by a [`Index`].
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
        let idx = Index(self.nodes.len() as u32);
        self.nodes.push(node);
        idx
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

    /// Returns `true` if `idx` refers to an occupied slot.
    pub(super) fn contains_key(&self, idx: Index) -> bool {
        self.get(idx).is_some()
    }

    /// Returns the number of occupied slots.
    pub(super) const fn len(&self) -> usize {
        self.nodes.len() - self.free.len()
    }

    /// Returns `true` if the arena holds no occupied slots.
    pub(super) const fn is_empty(&self) -> bool {
        self.len() == 0
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

    /// Stores the RLP of an unrevealed child and returns the child slot referencing it.
    pub(super) fn insert_blinded(&mut self, rlp: RlpNode) -> BranchChild {
        let slot = if let Some(slot) = self.blinded_free.pop() {
            self.blinded[slot as usize] = rlp;
            slot
        } else {
            self.blinded.push(rlp);
            (self.blinded.len() - 1) as u32
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
}

impl IndexOps<Index> for NodeArena {
    type Output = ArenaSparseNode;

    #[inline]
    fn index(&self, idx: Index) -> &Self::Output {
        let node = &self.nodes[idx.get()];
        debug_assert!(!matches!(node, ArenaSparseNode::Free), "indexed a free arena slot");
        node
    }
}

impl IndexMut<Index> for NodeArena {
    #[inline]
    fn index_mut(&mut self, idx: Index) -> &mut Self::Output {
        let node = &mut self.nodes[idx.get()];
        debug_assert!(!matches!(node, ArenaSparseNode::Free), "indexed a free arena slot");
        node
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
    pub(super) const fn revealed(idx: Index) -> Self {
        debug_assert!(idx.0 & Self::BLINDED == 0, "arena index overflows the blinded tag bit");
        Self(idx.0)
    }

    /// Returns a child referencing the blinded RLP at `slot`.
    const fn blinded(slot: u32) -> Self {
        debug_assert!(slot & Self::BLINDED == 0, "blinded slot overflows the tag bit");
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

#[cfg(test)]
mod tests {
    use super::*;
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
}
