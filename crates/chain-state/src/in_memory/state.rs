use super::{blocks::InMemoryBlocks, BlockState, ExecutedBlock, NewCanonicalChain};
use alloy_eips::BlockNumHash;
use alloy_primitives::{BlockNumber, B256};
use parking_lot::RwLock;
use reth_ethereum_primitives::EthPrimitives;
use reth_metrics::{metrics::Gauge, Metrics};
use reth_primitives_traits::NodePrimitives;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};

/// Read handle to the executed blocks that are not on disk yet.
///
/// The blocks are tracked in two sections:
///
/// - **canonical**: the canonical chain above the persisted frontier.
/// - **non-canonical**: every other executed block that can still become canonical, which are
///   blocks that were executed but not made canonical, blocks of forks, and blocks a reorg moved
///   off the canonical chain.
///
/// Every tracked block is served as a [`BlockState`]: a snapshot of the block and of the tracked
/// ancestors it is linked to. A canonical block is linked to its parent if the parent is
/// canonical, so the chain of a canonical block is the in-memory canonical chain up to that
/// block. A non-canonical block is linked to its parent if the parent is tracked in either
/// section.
///
/// Clones read the same blocks. The blocks only change through the [`InMemoryStateWriter`] that
/// [`Self::new`] returns. A state without blocks is valid, and a state from [`Self::read_only`]
/// never has any.
#[derive(Debug, Clone)]
pub struct InMemoryState<N: NodePrimitives = EthPrimitives> {
    inner: Arc<InMemoryStateInner<N>>,
}

impl<N: NodePrimitives> InMemoryState<N> {
    /// Creates an empty in-memory state and the only writer for it.
    #[must_use]
    pub fn new() -> (Self, InMemoryStateWriter<N>) {
        let in_memory_state = Self { inner: Arc::default() };
        (in_memory_state.clone(), InMemoryStateWriter { in_memory_state })
    }

    /// Creates an in-memory state that has no writer and therefore never tracks a block.
    ///
    /// This is the in-memory state of storage that is only read from disk.
    #[must_use]
    pub fn read_only() -> Self {
        Self { inner: Arc::default() }
    }

    /// Returns the state of the tracked block with the given hash, canonical or not.
    pub fn state_by_hash(&self, hash: B256) -> Option<Arc<BlockState<N>>> {
        self.read(|blocks| blocks.get(&hash).cloned())
    }

    /// Returns `true` if a block with the given hash is tracked, canonical or not.
    pub fn contains(&self, hash: &B256) -> bool {
        self.read(|blocks| blocks.contains(hash))
    }

    /// Returns the number of tracked blocks per section, read under one lock.
    pub fn block_counts(&self) -> InMemoryStateBlockCounts {
        self.read(InMemoryBlocks::counts)
    }

    /// Returns a counter that increases every time blocks are removed.
    ///
    /// State derived from the tracked blocks compares it with the value it saw last to learn that
    /// blocks it refers to may no longer be tracked.
    pub fn removal_generation(&self) -> u64 {
        self.inner.removal_generation.load(Ordering::Relaxed)
    }

    /// Applies `f` under the read lock.
    pub(super) fn read<R>(&self, f: impl FnOnce(&InMemoryBlocks<N>) -> R) -> R {
        f(&self.inner.blocks.read())
    }
}

/// Write access to the blocks of an [`InMemoryState`].
///
/// At most one writer exists per in-memory state: the one [`InMemoryState::new`] returned with
/// it. It cannot be cloned.
///
/// Every method takes the write lock of the in-memory state once, so readers never observe a
/// block in neither or both sections, or a partially applied update. Nothing else has to be
/// updated after any method.
///
/// # Pending block
///
/// An in-memory state marks at most one tracked block as the pending block, which is the block
/// [`CanonicalInMemoryState::pending_state`] returns. No method of this writer marks a block.
/// The methods that remove the mark say that they clear the pending block.
///
/// [`CanonicalInMemoryState::pending_state`]: super::CanonicalInMemoryState::pending_state
#[derive(Debug)]
pub struct InMemoryStateWriter<N: NodePrimitives = EthPrimitives> {
    in_memory_state: InMemoryState<N>,
}

impl<N: NodePrimitives> InMemoryStateWriter<N> {
    /// Returns `true` if `in_memory_state` reads the blocks this writer changes.
    pub fn writes_to(&self, in_memory_state: &InMemoryState<N>) -> bool {
        Arc::ptr_eq(&self.in_memory_state.inner, &in_memory_state.inner)
    }

    /// Inserts `block` into the non-canonical section unless it is already tracked.
    ///
    /// The block is linked to its parent if the parent is tracked, canonical or not. Does not
    /// change the pending block. Returns `true` if the block was inserted. Nothing else has to be
    /// updated afterwards.
    pub fn insert_executed(&mut self, block: ExecutedBlock<N>) -> bool {
        self.update(|blocks| blocks.insert_executed(block))
    }

    /// Makes the new blocks of `new_chain` the top of the canonical chain.
    ///
    /// The new blocks must not be empty, must be a chain from oldest to newest, and the first of
    /// them must build on the canonical blocks below it: if a canonical block is tracked below
    /// the height of the first new block, the highest such block is its parent.
    ///
    /// Afterwards the canonical section consists of the new blocks and of the canonical blocks
    /// below the first new block. Every other canonical block is moved to the non-canonical
    /// section: the blocks a reorg replaces and any block above the new tip.
    ///
    /// A new block that is tracked as non-canonical is moved to the canonical section, a new
    /// block that is not tracked is inserted into it. The old blocks of a
    /// [`NewCanonicalChain::Reorg`] that are not tracked are inserted into the non-canonical
    /// section.
    ///
    /// Clears the pending block. Nothing else has to be updated afterwards.
    ///
    /// # Panics
    ///
    /// In debug builds, if the new blocks are empty, are not a chain, or do not build on the
    /// canonical blocks below the first of them.
    pub fn update_chain(&mut self, new_chain: NewCanonicalChain<N>) {
        self.update(|blocks| blocks.update_chain(new_chain))
    }

    /// Removes canonical blocks up to and including `remove_until` if `persisted_hash` is a
    /// canonical in-memory block or the block the in-memory canonical chain builds on.
    ///
    /// Otherwise the canonical blocks in memory are not the ones that were persisted, and nothing
    /// is removed.
    ///
    /// Clears the pending block if it was removed. Nothing else has to be updated afterwards.
    pub fn remove_canonical_until(
        &mut self,
        persisted_hash: B256,
        remove_until: BlockNumber,
    ) -> InMemoryCanonicalBlocksRemoval {
        let removed =
            self.update(|blocks| blocks.remove_canonical_until(persisted_hash, remove_until));
        removed.map_or(InMemoryCanonicalBlocksRemoval::PersistedHashNotCanonical, |removed| {
            InMemoryCanonicalBlocksRemoval::Removed(removed.len())
        })
    }

    /// Removes the non-canonical blocks that can never become canonical once `finalized` is
    /// final: the non-canonical blocks below its height, the other non-canonical blocks at its
    /// height, and every block built on such a block at its height.
    ///
    /// Clears the pending block if it was removed. Returns the number of removed blocks. Nothing
    /// else has to be updated afterwards.
    pub fn prune_conflicting_with_finalized(&mut self, finalized: BlockNumHash) -> usize {
        self.update(|blocks| blocks.prune_conflicting_with_finalized(finalized)).len()
    }

    /// Removes all blocks of both sections and clears the pending block.
    ///
    /// Nothing else has to be updated afterwards.
    pub fn clear(&mut self) {
        // The removed blocks are dropped here, after the lock is released.
        drop(self.update(InMemoryBlocks::clear));
    }

    /// Applies `f` under the write lock, bumps the removal generation if `f` removed blocks and
    /// records the metrics after releasing the lock.
    ///
    /// Removals return the removed states, so that they are dropped after the lock is released:
    /// the in-memory state can hold the last reference to a block and its execution output.
    #[expect(clippy::needless_pass_by_ref_mut, reason = "writing requires the unique writer")]
    pub(super) fn update<R>(&mut self, f: impl FnOnce(&mut InMemoryBlocks<N>) -> R) -> R {
        let inner = &self.in_memory_state.inner;
        let (result, stats) = {
            let mut blocks = inner.blocks.write();
            let removed_count = blocks.removed_count();
            let result = f(&mut blocks);
            #[cfg(debug_assertions)]
            blocks.assert_invariants();
            if blocks.removed_count() != removed_count {
                inner.removal_generation.fetch_add(1, Ordering::Relaxed);
            }
            (result, blocks.stats())
        };
        inner.metrics.record(stats);
        result
    }
}

/// Number of tracked blocks per section.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InMemoryStateBlockCounts {
    /// Blocks in the canonical section.
    pub canonical: usize,
    /// Blocks in the non-canonical section.
    pub non_canonical: usize,
}

/// Outcome of removing persisted blocks from the canonical section.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InMemoryCanonicalBlocksRemoval {
    /// The persisted hash is on the in-memory canonical chain. The given number of blocks was
    /// removed, which is zero if none was at or below the requested height.
    Removed(usize),
    /// Nothing was removed, because the persisted hash is neither a canonical in-memory block nor
    /// the block the in-memory canonical chain builds on.
    PersistedHashNotCanonical,
}

/// The blocks of an [`InMemoryState`] with their lock, and what is derived from them.
#[derive(Debug, Default)]
struct InMemoryStateInner<N: NodePrimitives> {
    /// The canonical and the non-canonical section.
    blocks: RwLock<InMemoryBlocks<N>>,
    /// Counts the updates that removed blocks.
    removal_generation: AtomicU64,
    /// Metrics for the in-memory state.
    metrics: InMemoryStateMetrics,
}

/// Metrics for the in-memory state.
#[derive(Metrics)]
#[metrics(scope = "chain_state.in_memory_state")]
struct InMemoryStateMetrics {
    /// The number of the earliest canonical in-memory block.
    earliest_canonical_block_number: Gauge,
    /// The number of the latest canonical in-memory block.
    latest_canonical_block_number: Gauge,
    /// The number of canonical in-memory blocks.
    canonical_blocks: Gauge,
    /// The number of executed in-memory blocks that are not canonical.
    non_canonical_blocks: Gauge,
}

impl InMemoryStateMetrics {
    /// Records the metrics for the in-memory state.
    fn record(&self, stats: InMemoryStateStats) {
        if let Some(earliest_canonical) = stats.earliest_canonical {
            self.earliest_canonical_block_number.set(earliest_canonical as f64);
        }
        if let Some(latest_canonical) = stats.latest_canonical {
            self.latest_canonical_block_number.set(latest_canonical as f64);
        }
        self.canonical_blocks.set(stats.counts.canonical as f64);
        self.non_canonical_blocks.set(stats.counts.non_canonical as f64);
    }
}

/// The numbers the metrics of the in-memory state are recorded from, taken under the lock and
/// recorded after releasing it.
#[derive(Debug, Clone, Copy)]
pub(super) struct InMemoryStateStats {
    /// The number of the earliest canonical block.
    pub(super) earliest_canonical: Option<BlockNumber>,
    /// The number of the latest canonical block.
    pub(super) latest_canonical: Option<BlockNumber>,
    /// The number of blocks per section.
    pub(super) counts: InMemoryStateBlockCounts,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::TestBlockBuilder;

    fn hash(block: &ExecutedBlock) -> B256 {
        block.recovered_block().hash()
    }

    fn is_canonical(in_memory_state: &InMemoryState, block: &ExecutedBlock) -> bool {
        in_memory_state.read(|blocks| blocks.canonical().get(&hash(block)).is_some())
    }

    #[test]
    fn lookups_cover_both_sections() {
        let mut builder = TestBlockBuilder::eth();
        let (in_memory_state, mut in_memory_state_writer) = InMemoryState::new();
        assert_eq!(
            in_memory_state.block_counts(),
            InMemoryStateBlockCounts { canonical: 0, non_canonical: 0 }
        );

        let canonical = builder.get_executed_block_with_number(1, B256::random());
        let non_canonical = builder.get_executed_block_with_number(2, hash(&canonical));
        in_memory_state_writer
            .update_chain(NewCanonicalChain::Commit { new: vec![canonical.clone()] });
        assert!(in_memory_state_writer.insert_executed(non_canonical.clone()));
        assert!(!in_memory_state_writer.insert_executed(non_canonical.clone()), "already tracked");

        assert_eq!(
            in_memory_state.block_counts(),
            InMemoryStateBlockCounts { canonical: 1, non_canonical: 1 }
        );
        for block in [&canonical, &non_canonical] {
            assert!(in_memory_state.contains(&hash(block)));
            assert_eq!(in_memory_state.state_by_hash(hash(block)).unwrap().hash(), hash(block));
        }
        assert!(is_canonical(&in_memory_state, &canonical));
        assert!(!is_canonical(&in_memory_state, &non_canonical));
        assert_eq!(
            in_memory_state.state_by_hash(hash(&non_canonical)).unwrap().chain().count(),
            2,
            "a non-canonical block is linked to its canonical parent"
        );

        let unknown = B256::random();
        assert!(!in_memory_state.contains(&unknown));
        assert!(in_memory_state.state_by_hash(unknown).is_none());
    }

    #[test]
    fn writer_writes_to_the_state_it_was_created_with() {
        let (in_memory_state, in_memory_state_writer) = InMemoryState::<EthPrimitives>::new();
        let other_in_memory_state = InMemoryState::<EthPrimitives>::read_only();

        let cloned_in_memory_state = in_memory_state.clone();
        assert!(in_memory_state_writer.writes_to(&cloned_in_memory_state));
        assert!(in_memory_state_writer.writes_to(&in_memory_state));
        assert!(!in_memory_state_writer.writes_to(&other_in_memory_state));
    }

    #[test]
    fn removal_generation_only_changes_on_removals() {
        let mut builder = TestBlockBuilder::eth();
        let (in_memory_state, mut in_memory_state_writer) = InMemoryState::new();
        let blocks = builder.get_executed_blocks(1..5).collect::<Vec<_>>();
        let generation = in_memory_state.removal_generation();

        // Insertions and section changes do not remove blocks.
        in_memory_state_writer.insert_executed(blocks[0].clone());
        in_memory_state_writer.update_chain(NewCanonicalChain::Commit { new: blocks.clone() });
        let fork = builder.get_executed_block_with_number(4, hash(&blocks[2]));
        in_memory_state_writer.insert_executed(fork.clone());
        in_memory_state_writer.update_chain(NewCanonicalChain::Reorg {
            new: vec![fork.clone()],
            old: vec![blocks[3].clone()],
        });
        assert_eq!(in_memory_state.removal_generation(), generation);

        // Neither do removals that remove nothing.
        assert_eq!(
            in_memory_state_writer.remove_canonical_until(B256::random(), 4),
            InMemoryCanonicalBlocksRemoval::PersistedHashNotCanonical
        );
        assert_eq!(
            in_memory_state_writer.remove_canonical_until(hash(&blocks[0]), 0),
            InMemoryCanonicalBlocksRemoval::Removed(0)
        );
        assert_eq!(
            in_memory_state_writer
                .prune_conflicting_with_finalized(blocks[0].recovered_block().num_hash()),
            0
        );
        assert_eq!(in_memory_state.removal_generation(), generation);

        // Every removal of blocks increases it.
        assert_eq!(
            in_memory_state_writer.remove_canonical_until(hash(&blocks[0]), 1),
            InMemoryCanonicalBlocksRemoval::Removed(1)
        );
        let after_trim = in_memory_state.removal_generation();
        assert!(after_trim > generation);

        assert_eq!(
            in_memory_state_writer
                .prune_conflicting_with_finalized(fork.recovered_block().num_hash()),
            1
        );
        let after_prune = in_memory_state.removal_generation();
        assert!(after_prune > after_trim);

        assert_eq!(
            in_memory_state.block_counts(),
            InMemoryStateBlockCounts { canonical: 3, non_canonical: 0 }
        );
        in_memory_state_writer.clear();
        assert!(in_memory_state.removal_generation() > after_prune);
        assert_eq!(
            in_memory_state.block_counts(),
            InMemoryStateBlockCounts { canonical: 0, non_canonical: 0 }
        );

        // Clearing an empty state removes nothing.
        let after_clear = in_memory_state.removal_generation();
        in_memory_state_writer.clear();
        assert_eq!(in_memory_state.removal_generation(), after_clear);
    }

    #[test]
    fn remove_canonical_until_reports_whether_the_persisted_hash_is_canonical() {
        let mut builder = TestBlockBuilder::eth();
        let (in_memory_state, mut in_memory_state_writer) = InMemoryState::new();
        let blocks = builder.get_executed_blocks(1..6).collect::<Vec<_>>();

        // Without canonical blocks there is no in-memory canonical chain to be on.
        assert_eq!(
            in_memory_state_writer.remove_canonical_until(hash(&blocks[0]), 1),
            InMemoryCanonicalBlocksRemoval::PersistedHashNotCanonical
        );

        in_memory_state_writer.update_chain(NewCanonicalChain::Commit { new: blocks.clone() });
        let fork = builder.get_executed_block_with_number(2, hash(&blocks[0]));
        in_memory_state_writer.insert_executed(fork.clone());

        // A persisted block that is tracked, but not canonical, does not remove anything.
        assert_eq!(
            in_memory_state_writer.remove_canonical_until(hash(&fork), 2),
            InMemoryCanonicalBlocksRemoval::PersistedHashNotCanonical
        );
        assert_eq!(
            in_memory_state.block_counts(),
            InMemoryStateBlockCounts { canonical: 5, non_canonical: 1 }
        );

        // The persisted hash can be the block the in-memory canonical chain builds on.
        let anchor = blocks[0].recovered_block().parent_num_hash();
        assert_eq!(
            in_memory_state_writer.remove_canonical_until(anchor.hash, anchor.number),
            InMemoryCanonicalBlocksRemoval::Removed(0)
        );
        // Or a canonical in-memory block.
        assert_eq!(
            in_memory_state_writer.remove_canonical_until(hash(&blocks[1]), 2),
            InMemoryCanonicalBlocksRemoval::Removed(2)
        );
        assert_eq!(
            in_memory_state.block_counts(),
            InMemoryStateBlockCounts { canonical: 3, non_canonical: 1 }
        );
        assert!(!in_memory_state.contains(&hash(&blocks[0])));
        assert!(!in_memory_state.contains(&hash(&blocks[1])));

        // The remaining chain builds on the last removed block.
        assert_eq!(
            in_memory_state_writer.remove_canonical_until(hash(&blocks[1]), 2),
            InMemoryCanonicalBlocksRemoval::Removed(0)
        );
        let head = in_memory_state.state_by_hash(hash(&blocks[4])).unwrap();
        assert_eq!(head.chain().count(), 3);
        assert_eq!(head.anchor(), blocks[1].recovered_block().num_hash());
        // The fork was built on a removed block.
        let fork_state = in_memory_state.state_by_hash(hash(&fork)).unwrap();
        assert_eq!(fork_state.chain().count(), 1);
        assert_eq!(fork_state.anchor(), blocks[0].recovered_block().num_hash());
    }

    #[test]
    fn update_chain_tracks_untracked_blocks_in_the_section_they_belong_to() {
        let mut builder = TestBlockBuilder::eth();
        let (in_memory_state, mut in_memory_state_writer) = InMemoryState::new();
        // Blocks 1 to 3 are not tracked, for example because they are persisted. Blocks 4 and 5
        // are the in-memory canonical chain.
        let blocks = builder.get_executed_blocks(1..6).collect::<Vec<_>>();
        in_memory_state_writer
            .update_chain(NewCanonicalChain::Commit { new: blocks[3..].to_vec() });
        assert!(is_canonical(&in_memory_state, &blocks[3]), "an untracked new block is inserted");

        // A reorg to a fork off block 1 replaces blocks 2 to 5. Only the first fork block is
        // tracked.
        let mut parent = hash(&blocks[0]);
        let fork = (2..=6)
            .map(|number| {
                let block = builder.get_executed_block_with_number(number, parent);
                parent = hash(&block);
                block
            })
            .collect::<Vec<_>>();
        in_memory_state_writer.insert_executed(fork[0].clone());
        in_memory_state_writer.update_chain(NewCanonicalChain::Reorg {
            new: fork.clone(),
            old: blocks[1..].to_vec(),
        });

        assert_eq!(
            in_memory_state.block_counts(),
            InMemoryStateBlockCounts { canonical: 5, non_canonical: 4 }
        );
        for block in &fork {
            assert!(is_canonical(&in_memory_state, block));
        }
        let head = in_memory_state.state_by_hash(hash(&fork[4])).unwrap();
        assert_eq!(head.chain().count(), 5);
        assert_eq!(head.anchor(), blocks[0].recovered_block().num_hash());

        // The replaced blocks are tracked as non-canonical, the ones that were not tracked
        // included, and are linked to each other down to block 2.
        for block in &blocks[1..] {
            assert!(in_memory_state.contains(&hash(block)));
            assert!(!is_canonical(&in_memory_state, block));
        }
        let old_tip = in_memory_state.state_by_hash(hash(&blocks[4])).unwrap();
        assert_eq!(old_tip.chain().count(), 4);
        assert_eq!(old_tip.anchor(), blocks[0].recovered_block().num_hash());

        // Reorging back finds them.
        in_memory_state_writer.update_chain(NewCanonicalChain::Reorg {
            new: blocks[1..].to_vec(),
            old: fork.clone(),
        });
        assert_eq!(
            in_memory_state.block_counts(),
            InMemoryStateBlockCounts { canonical: 4, non_canonical: 5 }
        );
        assert!(is_canonical(&in_memory_state, &blocks[4]));
        assert!(!is_canonical(&in_memory_state, &fork[4]));
    }

    #[test]
    fn update_chain_moves_canonical_blocks_above_the_new_tip_off_the_canonical_chain() {
        let mut builder = TestBlockBuilder::eth();
        let (in_memory_state, mut in_memory_state_writer) = InMemoryState::new();
        let blocks = builder.get_executed_blocks(1..6).collect::<Vec<_>>();
        in_memory_state_writer.update_chain(NewCanonicalChain::Commit { new: blocks.clone() });

        in_memory_state_writer
            .update_chain(NewCanonicalChain::Commit { new: vec![blocks[2].clone()] });

        assert_eq!(
            in_memory_state.block_counts(),
            InMemoryStateBlockCounts { canonical: 3, non_canonical: 2 }
        );
        assert!(is_canonical(&in_memory_state, &blocks[2]));
        assert!(!is_canonical(&in_memory_state, &blocks[3]));
        // The blocks above stay linked to the canonical chain they are built on.
        assert_eq!(in_memory_state.state_by_hash(hash(&blocks[4])).unwrap().chain().count(), 5);
    }

    #[test]
    fn prune_conflicting_with_finalized_removes_blocks_that_cannot_become_canonical() {
        let mut builder = TestBlockBuilder::eth();
        let (in_memory_state, mut in_memory_state_writer) = InMemoryState::new();
        let canonical = builder.get_executed_blocks(1..5).collect::<Vec<_>>();
        in_memory_state_writer.update_chain(NewCanonicalChain::Commit { new: canonical.clone() });

        let fork_a = builder.get_executed_block_with_number(2, hash(&canonical[0]));
        let fork_a_child = builder.get_executed_block_with_number(3, hash(&fork_a));
        let fork_b = builder.get_executed_block_with_number(3, hash(&canonical[1]));
        let fork_b_child = builder.get_executed_block_with_number(4, hash(&fork_b));
        let fork_c = builder.get_executed_block_with_number(4, hash(&canonical[2]));
        let inserted = [&fork_a, &fork_a_child, &fork_b, &fork_b_child, &fork_c].map(|block| {
            assert!(in_memory_state_writer.insert_executed(block.clone()));
            Arc::downgrade(&in_memory_state.state_by_hash(hash(block)).unwrap())
        });

        // With block 3 finalized, only the fork built on it can still become canonical.
        let removed = in_memory_state_writer
            .prune_conflicting_with_finalized(canonical[2].recovered_block().num_hash());
        assert_eq!(removed, 4);
        for block in [&fork_a, &fork_a_child, &fork_b, &fork_b_child] {
            assert!(!in_memory_state.contains(&hash(block)));
        }
        assert!(in_memory_state.contains(&hash(&fork_c)));
        assert!(
            inserted[..4].iter().all(|state| state.upgrade().is_none()),
            "removed blocks are pinned"
        );
        assert_eq!(
            in_memory_state.block_counts(),
            InMemoryStateBlockCounts { canonical: 4, non_canonical: 1 },
            "canonical blocks are removed separately"
        );
    }
}
