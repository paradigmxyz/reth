use super::{
    state::{InMemoryStateBlockCounts, InMemoryStateStats},
    BlockState, ExecutedBlock, NewCanonicalChain,
};
use alloy_consensus::BlockHeader;
use alloy_eips::BlockNumHash;
use alloy_primitives::{
    map::{B256Map, B256Set},
    BlockNumber, B256,
};
use reth_primitives_traits::NodePrimitives;
use std::{cmp::Reverse, collections::BTreeMap, sync::Arc};

/// All executed in-memory blocks, split into the canonical and the non-canonical section.
///
/// # Invariants
///
/// - **I1** Every tracked block is in exactly one section.
/// - **I2** The number index of the canonical section and its blocks describe the same blocks. The
///   number index and the parent index of the non-canonical section describe exactly its blocks and
///   hold no empty set.
/// - **I3** A canonical block links to the state tracked for its parent in the canonical section, a
///   non-canonical block to the state tracked for its parent in either section. Without such a
///   parent the link is `None`.
/// - **I4** The canonical section is one chain: the parent links of its head cover exactly the
///   canonical section.
/// - **I5** The pending hash, if set, names a tracked block.
///
/// # Parent links
///
/// The parent link of a [`BlockState`] is immutable, so that a state is one consistent snapshot
/// of a block and its ancestors. A block whose parent link has to change is therefore replaced by
/// a new state, and so is every tracked block built on it, because a block points at its parent.
/// Removing the oldest blocks of a chain replaces every tracked block above them. Each
/// replacement is one allocation, and their number is bounded by the number of tracked blocks.
#[derive(Debug, Default)]
pub(super) struct InMemoryBlocks<N: NodePrimitives> {
    /// The canonical chain above the persisted frontier.
    canonical: InMemoryCanonicalBlocks<N>,
    /// Executed blocks that are not canonical.
    non_canonical: InMemoryNonCanonicalBlocks<N>,
    /// Hash of the pending block.
    pending_hash: Option<B256>,
    /// Hashes of the blocks whose parent link may be stale, see [`Self::relink`].
    ///
    /// This is empty between operations and kept to reuse its allocation.
    relink_queue: Vec<B256>,
    /// Number of blocks removed so far.
    removed_count: u64,
}

impl<N: NodePrimitives> InMemoryBlocks<N> {
    /// Returns the state of the tracked block with the given hash, canonical or not.
    pub(super) fn get(&self, hash: &B256) -> Option<&Arc<BlockState<N>>> {
        self.canonical.get(hash).or_else(|| self.non_canonical.blocks.get(hash))
    }

    /// Returns `true` if a block with the given hash is tracked, canonical or not.
    pub(super) fn contains(&self, hash: &B256) -> bool {
        self.canonical.blocks.contains_key(hash) || self.non_canonical.blocks.contains_key(hash)
    }

    /// Returns the canonical section.
    pub(super) const fn canonical(&self) -> &InMemoryCanonicalBlocks<N> {
        &self.canonical
    }

    /// Returns the state of the pending block, if there is one.
    pub(super) fn pending_state(&self) -> Option<&Arc<BlockState<N>>> {
        self.pending_hash.and_then(|hash| self.get(&hash))
    }

    /// Returns the number of tracked blocks per section.
    pub(super) fn counts(&self) -> InMemoryStateBlockCounts {
        InMemoryStateBlockCounts {
            canonical: self.canonical.blocks.len(),
            non_canonical: self.non_canonical.blocks.len(),
        }
    }

    /// Returns the numbers the metrics of the in-memory state are recorded from.
    pub(super) fn stats(&self) -> InMemoryStateStats {
        InMemoryStateStats {
            earliest_canonical: self.canonical.numbers.first_key_value().map(|(number, _)| *number),
            latest_canonical: self.canonical.numbers.last_key_value().map(|(number, _)| *number),
            counts: self.counts(),
        }
    }

    /// Returns the number of blocks that were removed so far. It never decreases.
    pub(super) const fn removed_count(&self) -> u64 {
        self.removed_count
    }

    /// Inserts `block` into the non-canonical section unless it is already tracked.
    ///
    /// Does not change the pending block. Returns `true` if the block was inserted.
    pub(super) fn insert_executed(&mut self, block: ExecutedBlock<N>) -> bool {
        if self.contains(&block.recovered_block().hash()) {
            return false
        }
        let parent = self.linked_parent(&block.recovered_block().parent_hash(), false).cloned();
        let state = Arc::new(BlockState::with_parent(block, parent));
        // Blocks built on this one can already be tracked, for example if it was removed as
        // persisted before.
        self.queue_children_of(&state);
        self.non_canonical.insert(state);
        self.relink();
        true
    }

    /// Inserts `block` like [`Self::insert_executed`] and makes it the pending block, also if it
    /// is already tracked.
    ///
    /// Returns `true` if the block was inserted.
    pub(super) fn insert_pending(&mut self, block: ExecutedBlock<N>) -> bool {
        let hash = block.recovered_block().hash();
        let inserted = self.insert_executed(block);
        self.pending_hash = Some(hash);
        inserted
    }

    /// Makes the new blocks of `new_chain` the top of the canonical chain, see
    /// [`Self::set_canonical_chain`], and inserts the blocks a reorg replaced that are not tracked
    /// into the non-canonical section.
    pub(super) fn update_chain(&mut self, new_chain: NewCanonicalChain<N>) {
        match new_chain {
            NewCanonicalChain::Commit { new } => self.set_canonical_chain(new),
            NewCanonicalChain::Reorg { new, old } => {
                self.set_canonical_chain(new);
                // The tracked ones were moved to the non-canonical section above.
                for block in old {
                    self.insert_executed(block);
                }
            }
        }
    }

    /// Makes `new`, a chain of blocks from oldest to newest, the top of the canonical chain and
    /// clears the pending block.
    ///
    /// Afterwards the canonical section consists of the blocks of `new` and of the canonical
    /// blocks below the first of them that it builds on. Every other canonical block is moved to
    /// the non-canonical section. A block of `new` that is tracked as non-canonical is moved to
    /// the canonical section, one that is not tracked is inserted into it.
    pub(super) fn set_canonical_chain(&mut self, new: Vec<ExecutedBlock<N>>) {
        debug_assert!(
            new.windows(2).all(|blocks| {
                blocks[1].recovered_block().parent_hash() == blocks[0].recovered_block().hash()
            }),
            "the new canonical blocks are not a chain"
        );
        let Some(first) = new.first() else {
            debug_assert!(false, "the new canonical chain is empty");
            return
        };
        let first_number = first.block_number();

        // The canonical blocks below the first new block stay canonical if the new chain builds
        // on them. They are one chain, so it is enough to compare the highest of them.
        let builds_on_canonical = self
            .canonical
            .numbers
            .range(..first_number)
            .next_back()
            .is_none_or(|(_, hash)| *hash == first.recovered_block().parent_hash());
        debug_assert!(
            builds_on_canonical,
            "the new canonical chain does not build on the canonical blocks below it"
        );
        let first_demoted = if builds_on_canonical { first_number } else { BlockNumber::MIN };
        let demoted = self
            .canonical
            .numbers
            .range(first_demoted..)
            .map(|(_, hash)| *hash)
            .filter(|hash| !new.iter().any(|block| block.recovered_block().hash() == *hash))
            .collect::<Vec<_>>();
        for hash in demoted {
            let state = self.canonical.remove(&hash);
            self.queue_moved(&state);
            self.non_canonical.insert(state);
        }

        for block in new {
            let hash = block.recovered_block().hash();
            if self.canonical.blocks.contains_key(&hash) {
                continue
            }
            let state = if self.non_canonical.blocks.contains_key(&hash) {
                self.non_canonical.remove(&hash)
            } else {
                let parent =
                    self.linked_parent(&block.recovered_block().parent_hash(), true).cloned();
                Arc::new(BlockState::with_parent(block, parent))
            };
            self.queue_moved(&state);
            self.canonical.insert(state);
        }

        self.pending_hash = None;
        self.relink();
    }

    /// Moves every canonical block to the non-canonical section and clears the pending block.
    pub(super) fn demote_canonical_chain(&mut self) {
        let canonical = std::mem::take(&mut self.canonical);
        // Only the link of the oldest block can be stale: as a non-canonical block it links to a
        // non-canonical parent.
        self.relink_queue.extend(canonical.numbers.first_key_value().map(|(_, hash)| *hash));
        for state in canonical.blocks.into_values() {
            self.non_canonical.insert(state);
        }
        self.pending_hash = None;
        self.relink();
    }

    /// Removes the canonical blocks up to and including `remove_until` and returns their states,
    /// oldest first, or `None` if `persisted_hash` is neither a canonical block nor the block
    /// the canonical chain builds on.
    ///
    /// Clears the pending block if it was removed.
    pub(super) fn remove_canonical_until(
        &mut self,
        persisted_hash: B256,
        remove_until: BlockNumber,
    ) -> Option<Vec<Arc<BlockState<N>>>> {
        // If the persisted hash is not on the canonical chain, canonical blocks were not actually
        // persisted. This can happen if persisting takes a long time while a reorg is happening.
        if !self.canonical.contains_or_builds_on(&persisted_hash) {
            return None
        }
        let removed = self.canonical.remove_until(remove_until);
        self.finish_removal(&removed);
        Some(removed)
    }

    /// Removes the non-canonical blocks that can never become canonical once `finalized` is
    /// final, and returns their states: the non-canonical blocks below its height, the other
    /// non-canonical blocks at its height, and every block built on such a block at its height.
    ///
    /// Clears the pending block if it was removed.
    pub(super) fn prune_conflicting_with_finalized(
        &mut self,
        finalized: BlockNumHash,
    ) -> Vec<Arc<BlockState<N>>> {
        let removed = self.non_canonical.remove_conflicting_with(finalized);
        self.finish_removal(&removed);
        removed
    }

    /// Removes all blocks of both sections, clears the pending block and returns what was
    /// tracked.
    pub(super) fn clear(&mut self) -> Self {
        let InMemoryStateBlockCounts { canonical, non_canonical } = self.counts();
        let removed_count = self.removed_count + (canonical + non_canonical) as u64;
        std::mem::replace(self, Self { removed_count, ..Default::default() })
    }

    /// Asserts the invariants of this type.
    #[cfg(any(test, debug_assertions))]
    pub(super) fn assert_invariants(&self) {
        let Self { canonical, non_canonical, pending_hash, relink_queue, removed_count: _ } = self;

        // I1 and I2.
        assert_eq!(canonical.numbers.len(), canonical.blocks.len());
        for (number, hash) in &canonical.numbers {
            assert_eq!(canonical.blocks[hash].number(), *number);
            assert!(!non_canonical.blocks.contains_key(hash), "block {hash} is in both sections");
        }
        for (number, hashes) in &non_canonical.numbers {
            assert!(!hashes.is_empty(), "the number index holds an empty set");
            for hash in hashes {
                assert_eq!(non_canonical.blocks[hash].number(), *number);
            }
        }
        for (parent_hash, hashes) in &non_canonical.children {
            assert!(!hashes.is_empty(), "the parent index holds an empty set");
            for hash in hashes {
                assert_eq!(non_canonical.blocks[hash].parent_hash(), *parent_hash);
            }
        }
        for index in [
            non_canonical.numbers.values().map(B256Set::len).sum::<usize>(),
            non_canonical.children.values().map(B256Set::len).sum::<usize>(),
        ] {
            assert_eq!(index, non_canonical.blocks.len(), "an index misses non-canonical blocks");
        }

        // I3.
        assert!(relink_queue.is_empty(), "blocks are still queued for relinking");
        for state in canonical.blocks.values().chain(non_canonical.blocks.values()) {
            let is_canonical = canonical.blocks.contains_key(&state.hash());
            match (state.parent(), self.linked_parent(&state.parent_hash(), is_canonical)) {
                (None, None) => {}
                (Some(linked), Some(parent)) if Arc::ptr_eq(linked, parent) => {}
                (linked, parent) => panic!(
                    "block {} is linked to {:?}, but its tracked parent is {:?}",
                    state.hash(),
                    linked.map(|linked| linked.hash()),
                    parent.map(|parent| parent.hash()),
                ),
            }
        }

        // I4.
        assert_eq!(
            canonical.head().map_or(0, |head| head.chain().count()),
            canonical.blocks.len(),
            "the canonical section is not one chain"
        );

        // I5.
        assert!(
            pending_hash.is_none_or(|hash| self.contains(&hash)),
            "the pending block is not tracked"
        );
    }

    /// Returns the state a block in the given section links to as its parent.
    ///
    /// A canonical block only links to a canonical parent. A non-canonical block links to its
    /// parent in either section.
    fn linked_parent(&self, parent_hash: &B256, canonical: bool) -> Option<&Arc<BlockState<N>>> {
        if canonical {
            self.canonical.get(parent_hash)
        } else {
            self.get(parent_hash)
        }
    }

    /// Returns the hashes of all blocks, in either section, that are built on `parent`.
    fn children_of(&self, parent: &BlockState<N>) -> impl Iterator<Item = B256> + '_ {
        self.canonical.children_of(parent).chain(self.non_canonical.children_of(parent))
    }

    /// Queues the blocks built on `parent`, a block that was inserted or removed, for
    /// [`Self::relink`].
    fn queue_children_of(&mut self, parent: &BlockState<N>) {
        let mut queue = std::mem::take(&mut self.relink_queue);
        queue.extend(self.children_of(parent));
        self.relink_queue = queue;
    }

    /// Queues `state`, a block that changes section, and the blocks built on it for
    /// [`Self::relink`].
    fn queue_moved(&mut self, state: &BlockState<N>) {
        self.relink_queue.push(state.hash());
        self.queue_children_of(state);
    }

    /// Replaces every queued block whose parent link does not point at the state
    /// [`Self::linked_parent`] returns for it by a state with that link, and does the same for
    /// the blocks built on a replaced block.
    fn relink(&mut self) {
        let mut queue = std::mem::take(&mut self.relink_queue);
        // The oldest block is visited first, and the blocks built on a replaced block right
        // after it. Every block is then visited after its ancestors and replaced at most once.
        queue.sort_unstable_by_key(|hash| Reverse(self.get(hash).map(|state| state.number())));
        while let Some(hash) = queue.pop() {
            let canonical = self.canonical.blocks.contains_key(&hash);
            let Some(state) = self.get(&hash) else {
                debug_assert!(false, "block {hash} is queued for relinking but not tracked");
                continue
            };
            let parent = self.linked_parent(&state.parent_hash(), canonical);
            let linked = match (state.parent(), parent) {
                (None, None) => true,
                (Some(linked), Some(parent)) => Arc::ptr_eq(linked, parent),
                _ => false,
            };
            if linked {
                continue
            }
            let relinked = Arc::new(BlockState::with_parent(state.block(), parent.cloned()));
            queue.extend(self.children_of(&relinked));
            let section =
                if canonical { &mut self.canonical.blocks } else { &mut self.non_canonical.blocks };
            section.insert(hash, relinked);
        }
        self.relink_queue = queue;
    }

    /// Counts the `removed` blocks, clears the pending block if it is one of them, and relinks
    /// the blocks built on them.
    fn finish_removal(&mut self, removed: &[Arc<BlockState<N>>]) {
        self.removed_count += removed.len() as u64;
        if self.pending_hash.is_some_and(|hash| !self.contains(&hash)) {
            self.pending_hash = None;
        }
        for state in removed {
            self.queue_children_of(state);
        }
        self.relink();
    }
}

/// The canonical chain above the persisted frontier.
#[derive(Debug, Default)]
pub(super) struct InMemoryCanonicalBlocks<N: NodePrimitives> {
    /// Canonical blocks by hash.
    blocks: B256Map<Arc<BlockState<N>>>,
    /// Canonical block hashes by number.
    numbers: BTreeMap<BlockNumber, B256>,
}

impl<N: NodePrimitives> InMemoryCanonicalBlocks<N> {
    /// Returns the state of the canonical block with the given hash.
    pub(super) fn get(&self, hash: &B256) -> Option<&Arc<BlockState<N>>> {
        self.blocks.get(hash)
    }

    /// Returns the state of the canonical block with the given number.
    pub(super) fn by_number(&self, number: BlockNumber) -> Option<&Arc<BlockState<N>>> {
        self.numbers.get(&number).and_then(|hash| self.blocks.get(hash))
    }

    /// Returns the hash of the canonical block with the given number.
    pub(super) fn hash_by_number(&self, number: BlockNumber) -> Option<B256> {
        self.numbers.get(&number).copied()
    }

    /// Returns the state of the highest canonical block.
    pub(super) fn head(&self) -> Option<&Arc<BlockState<N>>> {
        self.numbers.last_key_value().and_then(|(_, hash)| self.blocks.get(hash))
    }

    /// Returns `true` if `hash` is a canonical in-memory block or the block the chain builds on.
    ///
    /// This is whether [`BlockState::chain_hashes`] of the head contains `hash`, answered from
    /// the indexes.
    fn contains_or_builds_on(&self, hash: &B256) -> bool {
        self.blocks.contains_key(hash) ||
            self.numbers
                .first_key_value()
                .and_then(|(_, first)| self.blocks.get(first))
                .is_some_and(|first| first.parent_hash() == *hash)
    }

    /// Returns the hash of the canonical block built on `parent`, if there is one.
    fn children_of(&self, parent: &BlockState<N>) -> impl Iterator<Item = B256> + '_ {
        let parent_hash = parent.hash();
        parent
            .number()
            .checked_add(1)
            .and_then(|number| self.by_number(number))
            .filter(|child| child.parent_hash() == parent_hash)
            .map(|child| child.hash())
            .into_iter()
    }

    /// Makes `state` the canonical block at its height, which must be vacant.
    fn insert(&mut self, state: Arc<BlockState<N>>) {
        let hash = state.hash();
        let displaced = self.numbers.insert(state.number(), hash);
        debug_assert!(displaced.is_none(), "canonical height {} is not vacant", state.number());
        self.blocks.insert(hash, state);
    }

    /// Removes the canonical block with the given hash and returns its state.
    ///
    /// # Panics
    ///
    /// If the block is not canonical.
    fn remove(&mut self, hash: &B256) -> Arc<BlockState<N>> {
        let Some(state) = self.blocks.remove(hash) else {
            unreachable!("block {hash} is not canonical")
        };
        let indexed = self.numbers.remove(&state.number());
        debug_assert_eq!(indexed, Some(*hash), "canonical block {hash} is not indexed by number");
        state
    }

    /// Removes the canonical blocks up to and including `number` and returns their states,
    /// oldest first.
    fn remove_until(&mut self, number: BlockNumber) -> Vec<Arc<BlockState<N>>> {
        let mut removed = Vec::new();
        while let Some(first) = self.numbers.first_entry() &&
            *first.key() <= number
        {
            let hash = first.remove();
            let Some(state) = self.blocks.remove(&hash) else {
                unreachable!("canonical block {hash} is indexed by number but not tracked")
            };
            removed.push(state);
        }
        removed
    }
}

/// Executed blocks that are not canonical.
///
/// These are blocks that were validated but are not (yet) part of the canonical chain: blocks
/// that were executed but not made canonical, blocks of forks, and blocks a reorg moved off the
/// canonical chain. They stay here until they become canonical or can no longer become canonical.
#[derive(Debug, Default)]
pub(super) struct InMemoryNonCanonicalBlocks<N: NodePrimitives> {
    /// Non-canonical blocks by hash.
    blocks: B256Map<Arc<BlockState<N>>>,
    /// Non-canonical block hashes by number. Forks can put several blocks at the same height.
    numbers: BTreeMap<BlockNumber, B256Set>,
    /// Non-canonical block hashes by parent hash. The parent can be non-canonical, canonical or
    /// not tracked.
    children: B256Map<B256Set>,
}

impl<N: NodePrimitives> InMemoryNonCanonicalBlocks<N> {
    /// Returns the hashes of the non-canonical blocks built on `parent`.
    fn children_of(&self, parent: &BlockState<N>) -> impl Iterator<Item = B256> + '_ {
        self.children.get(&parent.hash()).into_iter().flatten().copied()
    }

    /// Inserts `state`, the state of a block that is not tracked, and indexes it.
    fn insert(&mut self, state: Arc<BlockState<N>>) {
        let hash = state.hash();
        self.numbers.entry(state.number()).or_default().insert(hash);
        self.children.entry(state.parent_hash()).or_default().insert(hash);
        let replaced = self.blocks.insert(hash, state);
        debug_assert!(replaced.is_none(), "block {hash} is already non-canonical");
    }

    /// Removes the non-canonical block with the given hash from the blocks and the indexes, and
    /// returns its state.
    ///
    /// # Panics
    ///
    /// If the block is not non-canonical.
    fn remove(&mut self, hash: &B256) -> Arc<BlockState<N>> {
        let Some(state) = self.blocks.remove(hash) else {
            unreachable!("block {hash} is not non-canonical")
        };
        if remove_indexed(self.numbers.get_mut(&state.number()), hash) {
            self.numbers.remove(&state.number());
        }
        if remove_indexed(self.children.get_mut(&state.parent_hash()), hash) {
            self.children.remove(&state.parent_hash());
        }
        state
    }

    /// Removes the blocks that can never become canonical once `finalized` is final, and returns
    /// their states: the blocks below its height, the other blocks at its height, and every block
    /// built on such a block at its height.
    fn remove_conflicting_with(&mut self, finalized: BlockNumHash) -> Vec<Arc<BlockState<N>>> {
        let mut removed = Vec::new();
        while let Some(hash) = self
            .numbers
            .first_key_value()
            .filter(|(number, _)| **number < finalized.number)
            .and_then(|(_, hashes)| hashes.iter().next().copied())
        {
            removed.push(self.remove(&hash));
        }

        // The blocks removed from here on are the queue of blocks whose children are removed
        // next. The blocks below the finalized height are not part of it, because the finalized
        // block is built on one of them.
        let mut next = removed.len();
        while let Some(hash) = self
            .numbers
            .get(&finalized.number)
            .and_then(|hashes| hashes.iter().find(|hash| **hash != finalized.hash).copied())
        {
            removed.push(self.remove(&hash));
        }
        while next < removed.len() {
            let parent_hash = removed[next].hash();
            while let Some(hash) =
                self.children.get(&parent_hash).and_then(|hashes| hashes.iter().next().copied())
            {
                removed.push(self.remove(&hash));
            }
            next += 1;
        }
        removed
    }
}

/// Removes `hash` from `hashes`, the set an index of the non-canonical section holds for a block,
/// and returns `true` if the set is empty afterwards and has to be removed from the index.
fn remove_indexed(hashes: Option<&mut B256Set>, hash: &B256) -> bool {
    let Some(hashes) = hashes else {
        debug_assert!(false, "non-canonical block {hash} is not indexed");
        return false
    };
    let removed = hashes.remove(hash);
    debug_assert!(removed, "non-canonical block {hash} is not indexed");
    hashes.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::TestBlockBuilder;
    use reth_ethereum_primitives::EthPrimitives;

    fn hash(block: &ExecutedBlock) -> B256 {
        block.recovered_block().hash()
    }

    /// Returns the hashes the non-canonical section indexes as built on `parent`.
    fn non_canonical_children(
        blocks: &InMemoryBlocks<EthPrimitives>,
        parent: &ExecutedBlock,
    ) -> B256Set {
        blocks.non_canonical.children.get(&hash(parent)).cloned().unwrap_or_default()
    }

    /// Returns the hashes both sections index at the given height, the canonical one first.
    fn hashes_at(blocks: &InMemoryBlocks<EthPrimitives>, number: BlockNumber) -> Vec<B256> {
        let canonical = blocks.canonical.numbers.get(&number).into_iter();
        let non_canonical = blocks.non_canonical.numbers.get(&number).into_iter().flatten();
        canonical.chain(non_canonical).copied().collect()
    }

    #[test]
    fn linear_chain_is_indexed_by_parent_and_number() {
        let mut blocks = InMemoryBlocks::<EthPrimitives>::default();
        let chain = TestBlockBuilder::eth().get_executed_blocks(1..4).collect::<Vec<_>>();

        assert!(blocks.insert_executed(chain[0].clone()));
        assert!(blocks.insert_executed(chain[1].clone()));
        assert_eq!(
            non_canonical_children(&blocks, &chain[0]),
            B256Set::from_iter([hash(&chain[1])])
        );
        assert!(non_canonical_children(&blocks, &chain[1]).is_empty());

        assert!(blocks.insert_executed(chain[2].clone()));
        assert_eq!(
            non_canonical_children(&blocks, &chain[1]),
            B256Set::from_iter([hash(&chain[2])])
        );
        assert!(non_canonical_children(&blocks, &chain[2]).is_empty());

        // Executed blocks are not canonical until they are made canonical.
        assert_eq!(blocks.counts(), InMemoryStateBlockCounts { canonical: 0, non_canonical: 3 });
        for (number, block) in (1..).zip(&chain) {
            assert_eq!(hashes_at(&blocks, number), vec![hash(block)]);
        }
        assert_eq!(
            blocks.get(&hash(&chain[2])).unwrap().chain_hashes().collect::<Vec<_>>(),
            vec![
                hash(&chain[2]),
                hash(&chain[1]),
                hash(&chain[0]),
                chain[0].recovered_block().parent_hash()
            ]
        );
        blocks.assert_invariants();
    }

    #[test]
    fn forks_are_indexed_next_to_the_canonical_blocks_and_reorgs_swap_the_sections() {
        let mut builder = TestBlockBuilder::eth();
        let mut blocks = InMemoryBlocks::<EthPrimitives>::default();
        let chain = builder.get_executed_blocks(1..6).collect::<Vec<_>>();
        for block in &chain {
            blocks.insert_executed(block.clone());
        }
        blocks.set_canonical_chain(chain.clone());
        assert_eq!(blocks.counts(), InMemoryStateBlockCounts { canonical: 5, non_canonical: 0 });

        let fork_3 = builder.get_executed_block_with_number(3, hash(&chain[1]));
        let fork_4 = builder.get_executed_block_with_number(4, hash(&fork_3));
        let fork_5 = builder.get_executed_block_with_number(5, hash(&fork_4));
        for block in [&fork_3, &fork_4, &fork_5] {
            assert!(blocks.insert_executed(block.clone()));
        }
        assert_eq!(blocks.counts(), InMemoryStateBlockCounts { canonical: 5, non_canonical: 3 });
        // Two blocks at each forked height, the canonical one and the fork.
        assert_eq!(hashes_at(&blocks, 3), vec![hash(&chain[2]), hash(&fork_3)]);
        assert_eq!(hashes_at(&blocks, 4), vec![hash(&chain[3]), hash(&fork_4)]);
        assert_eq!(hashes_at(&blocks, 5), vec![hash(&chain[4]), hash(&fork_5)]);
        assert_eq!(non_canonical_children(&blocks, &chain[1]), B256Set::from_iter([hash(&fork_3)]));

        // Inserting a tracked block again changes nothing.
        assert!(!blocks.insert_executed(fork_4.clone()));
        assert_eq!(blocks.counts(), InMemoryStateBlockCounts { canonical: 5, non_canonical: 3 });
        assert_eq!(non_canonical_children(&blocks, &fork_3), B256Set::from_iter([hash(&fork_4)]));
        assert_eq!(non_canonical_children(&blocks, &fork_4), B256Set::from_iter([hash(&fork_5)]));
        assert_eq!(hashes_at(&blocks, 4).len(), 2);

        // Reorging to the fork moves the replaced canonical blocks to the non-canonical section.
        blocks.update_chain(NewCanonicalChain::Reorg {
            new: vec![fork_3.clone(), fork_4, fork_5.clone()],
            old: chain[2..].to_vec(),
        });
        assert_eq!(blocks.counts(), InMemoryStateBlockCounts { canonical: 5, non_canonical: 3 });
        assert_eq!(hashes_at(&blocks, 3), vec![hash(&fork_3), hash(&chain[2])]);
        assert_eq!(hashes_at(&blocks, 5), vec![hash(&fork_5), hash(&chain[4])]);
        assert!(blocks.canonical().get(&hash(&fork_3)).is_some());
        assert!(blocks.canonical().get(&hash(&chain[2])).is_none());
        assert!(blocks.contains(&hash(&chain[4])));
        assert_eq!(
            non_canonical_children(&blocks, &chain[1]),
            B256Set::from_iter([hash(&chain[2])])
        );
        assert!(non_canonical_children(&blocks, &fork_3).is_empty());
        blocks.assert_invariants();
    }

    #[test]
    fn removed_blocks_leave_the_indexes() {
        let mut builder = TestBlockBuilder::eth();
        let mut blocks = InMemoryBlocks::<EthPrimitives>::default();
        let chain = builder.get_executed_blocks(1..6).collect::<Vec<_>>();
        blocks.set_canonical_chain(chain.clone());
        let dead_fork = builder.get_executed_block_with_number(2, hash(&chain[0]));
        let dead_fork_child = builder.get_executed_block_with_number(3, hash(&dead_fork));
        let live_fork = builder.get_executed_block_with_number(4, hash(&chain[2]));
        for block in [&dead_fork, &dead_fork_child, &live_fork] {
            blocks.insert_executed(block.clone());
        }

        let persisted = chain[1].recovered_block().num_hash();
        let removed = blocks.remove_canonical_until(persisted.hash, persisted.number).unwrap();
        assert_eq!(removed.len(), 2);
        let removed = blocks.prune_conflicting_with_finalized(persisted);
        assert_eq!(
            removed.iter().map(|state| state.hash()).collect::<B256Set>(),
            B256Set::from_iter([hash(&dead_fork), hash(&dead_fork_child)])
        );
        assert_eq!(blocks.removed_count(), 4);

        assert!(hashes_at(&blocks, 1).is_empty());
        assert!(hashes_at(&blocks, 2).is_empty());
        assert_eq!(hashes_at(&blocks, 3), vec![hash(&chain[2])]);
        assert_eq!(hashes_at(&blocks, 4), vec![hash(&chain[3]), hash(&live_fork)]);
        assert_eq!(hashes_at(&blocks, 5), vec![hash(&chain[4])]);
        assert!(non_canonical_children(&blocks, &chain[0]).is_empty());
        assert_eq!(
            non_canonical_children(&blocks, &chain[2]),
            B256Set::from_iter([hash(&live_fork)])
        );

        // The remaining chains start right after the removed blocks.
        let head = blocks.canonical().head().unwrap();
        assert_eq!(head.chain().count(), 3);
        assert_eq!(head.anchor(), persisted);
        let live_fork_state = blocks.get(&hash(&live_fork)).unwrap();
        assert_eq!(live_fork_state.chain().count(), 2);
        assert_eq!(live_fork_state.anchor(), persisted);
        blocks.assert_invariants();
    }

    #[test]
    fn executed_blocks_move_between_sections_and_are_relinked_on_trim() {
        let mut builder = TestBlockBuilder::eth();
        let mut blocks = InMemoryBlocks::<EthPrimitives>::default();

        // Blocks are executed first and made canonical afterwards.
        let canonical = builder.get_executed_blocks(1..4).collect::<Vec<_>>();
        let inserted = canonical
            .iter()
            .map(|block| {
                assert!(blocks.insert_executed(block.clone()));
                Arc::clone(blocks.get(&hash(block)).unwrap())
            })
            .collect::<Vec<_>>();
        assert_eq!(blocks.counts(), InMemoryStateBlockCounts { canonical: 0, non_canonical: 3 });
        assert!(blocks.canonical().head().is_none());
        assert!(Arc::ptr_eq(inserted[2].parent().unwrap(), &inserted[1]));

        blocks.set_canonical_chain(canonical.clone());
        assert_eq!(blocks.counts(), InMemoryStateBlockCounts { canonical: 3, non_canonical: 0 });
        for (block, before) in canonical.iter().zip(&inserted) {
            let after = blocks.canonical().get(&hash(block)).unwrap();
            assert!(Arc::ptr_eq(before, after), "a block whose parent link is unchanged is moved");
        }

        // A fork off block 1 and the pending block on top of the head are not canonical.
        let fork = builder.get_executed_block_with_number(2, hash(&canonical[0]));
        assert!(blocks.insert_executed(fork.clone()));
        let pending = builder.get_executed_block_with_number(4, hash(&canonical[2]));
        assert!(blocks.insert_pending(pending.clone()));
        assert!(blocks.canonical().get(&hash(&fork)).is_none());
        assert!(blocks.canonical().get(&hash(&pending)).is_none());
        assert_eq!(
            non_canonical_children(&blocks, &canonical[0]),
            B256Set::from_iter([hash(&fork)])
        );
        assert_eq!(hashes_at(&blocks, 2), vec![hash(&canonical[1]), hash(&fork)]);
        assert_eq!(blocks.get(&hash(&fork)).unwrap().chain().count(), 2);
        assert_eq!(blocks.pending_state().unwrap().hash(), hash(&pending));
        assert_eq!(blocks.pending_state().unwrap().chain().count(), 4);

        // Removing the blocks up to block 2 relinks every remaining block, canonical or not, so
        // nothing keeps the removed blocks alive.
        let trimmed = inserted[..2].iter().map(Arc::downgrade).collect::<Vec<_>>();
        drop(inserted);
        let persisted = canonical[1].recovered_block().num_hash();
        let removed = blocks.remove_canonical_until(persisted.hash, persisted.number).unwrap();
        assert_eq!(
            removed.iter().map(|state| state.hash()).collect::<Vec<_>>(),
            vec![hash(&canonical[0]), hash(&canonical[1])]
        );
        drop(removed);
        assert!(trimmed.iter().all(|state| state.upgrade().is_none()), "removed blocks are pinned");

        let head = blocks.canonical().head().unwrap();
        assert_eq!(head.hash(), hash(&canonical[2]));
        assert!(head.parent().is_none());
        assert!(Arc::ptr_eq(blocks.pending_state().unwrap().parent().unwrap(), head));
        // The fork's parent was removed, so its chain starts at the fork itself.
        let fork_state = blocks.get(&hash(&fork)).unwrap();
        assert!(fork_state.parent().is_none());
        assert_eq!(fork_state.anchor(), canonical[0].recovered_block().num_hash());
        blocks.assert_invariants();
    }

    #[test]
    fn reorg_moves_blocks_between_sections() {
        let mut builder = TestBlockBuilder::eth();
        let mut blocks = InMemoryBlocks::<EthPrimitives>::default();

        let base = builder.get_executed_block_with_number(1, B256::random());
        let old_tip = builder.get_executed_block_with_number(2, hash(&base));
        blocks.set_canonical_chain(vec![base.clone(), old_tip.clone()]);
        let old_tip_state = Arc::clone(blocks.canonical().get(&hash(&old_tip)).unwrap());

        let new_tip = builder.get_executed_block_with_number(2, hash(&base));
        assert!(blocks.insert_pending(new_tip.clone()));
        let new_tip_state = Arc::clone(blocks.get(&hash(&new_tip)).unwrap());
        blocks.update_chain(NewCanonicalChain::Reorg {
            new: vec![new_tip.clone()],
            old: vec![old_tip.clone()],
        });

        assert!(Arc::ptr_eq(&new_tip_state, blocks.canonical().head().unwrap()));
        assert!(blocks.canonical().get(&hash(&old_tip)).is_none());
        // The reorged block stays tracked as non-canonical, with the same state.
        assert!(Arc::ptr_eq(&old_tip_state, blocks.get(&hash(&old_tip)).unwrap()));
        assert_eq!(non_canonical_children(&blocks, &base), B256Set::from_iter([hash(&old_tip)]));
        assert!(blocks.pending_state().is_none(), "a chain update clears the pending block");

        // Reorging back moves the same states back.
        blocks.update_chain(NewCanonicalChain::Reorg {
            new: vec![old_tip],
            old: vec![new_tip.clone()],
        });
        assert!(Arc::ptr_eq(&old_tip_state, blocks.canonical().head().unwrap()));
        assert!(Arc::ptr_eq(&new_tip_state, blocks.get(&hash(&new_tip)).unwrap()));
        assert_eq!(blocks.counts(), InMemoryStateBlockCounts { canonical: 2, non_canonical: 1 });
        blocks.assert_invariants();
    }

    #[test]
    fn inserting_a_removed_parent_relinks_only_its_non_canonical_children() {
        let mut builder = TestBlockBuilder::eth();
        let mut blocks = InMemoryBlocks::<EthPrimitives>::default();
        let chain = builder.get_executed_blocks(1..4).collect::<Vec<_>>();
        blocks.set_canonical_chain(chain.clone());
        let fork = builder.get_executed_block_with_number(2, hash(&chain[0]));
        blocks.insert_executed(fork.clone());
        let persisted = chain[0].recovered_block().num_hash();
        blocks.remove_canonical_until(persisted.hash, persisted.number).unwrap();
        assert!(blocks.get(&hash(&fork)).unwrap().parent().is_none());
        assert_eq!(blocks.canonical().head().unwrap().chain().count(), 2);

        // A persisted block can be tracked again, as a non-canonical block.
        assert!(blocks.insert_executed(chain[0].clone()));
        blocks.assert_invariants();

        // The fork links to it, but the canonical chain does not continue in the non-canonical
        // section.
        let parent = blocks.get(&hash(&chain[0])).unwrap();
        assert!(Arc::ptr_eq(blocks.get(&hash(&fork)).unwrap().parent().unwrap(), parent));
        assert!(blocks.canonical().get(&hash(&chain[1])).unwrap().parent().is_none());
        assert_eq!(blocks.canonical().head().unwrap().chain().count(), 2);

        // Once no block is canonical, the oldest of the demoted blocks links to it as well.
        blocks.demote_canonical_chain();
        assert_eq!(blocks.counts(), InMemoryStateBlockCounts { canonical: 0, non_canonical: 4 });
        assert_eq!(blocks.get(&hash(&chain[2])).unwrap().chain().count(), 3);
        blocks.assert_invariants();
    }

    #[test]
    fn prune_keeps_the_finalized_block_and_the_blocks_built_on_it() {
        let mut builder = TestBlockBuilder::eth();
        let mut blocks = InMemoryBlocks::<EthPrimitives>::default();
        // No block is canonical in memory, for example after the canonical head was moved to a
        // persisted block.
        let chain = builder.get_executed_blocks(1..4).collect::<Vec<_>>();
        let fork = builder.get_executed_block_with_number(2, hash(&chain[0]));
        let fork_child = builder.get_executed_block_with_number(3, hash(&fork));
        for block in chain.iter().chain([&fork, &fork_child]) {
            blocks.insert_executed(block.clone());
        }
        blocks.insert_pending(fork_child.clone());

        let removed =
            blocks.prune_conflicting_with_finalized(chain[1].recovered_block().num_hash());
        assert_eq!(
            removed.iter().map(|state| state.hash()).collect::<B256Set>(),
            B256Set::from_iter([hash(&chain[0]), hash(&fork), hash(&fork_child)])
        );
        assert!(blocks.pending_state().is_none(), "the removed pending block is cleared");
        assert_eq!(blocks.counts(), InMemoryStateBlockCounts { canonical: 0, non_canonical: 2 });

        // The finalized block is built on a removed block, so its chain starts at itself.
        let finalized = blocks.get(&hash(&chain[1])).unwrap();
        assert!(finalized.parent().is_none());
        assert_eq!(blocks.get(&hash(&chain[2])).unwrap().chain().count(), 2);
        blocks.assert_invariants();
    }
}
