//! Level-batched hashing of dirty sparse trie nodes.
//!
//! Instead of encoding and hashing one branch at a time while walking the trie depth-first,
//! the dirty nodes of one or more arenas are first collected per depth. Then all dirty leaves
//! are encoded and hashed as one batch, and the branches of each depth, deepest first, are
//! encoded and hashed as one batch (the children of a depth are all cached once the deeper
//! depths are done). Hashing many independent inputs at once lets [keccak256_batch] use
//! multi-buffer SIMD implementations.

use super::{
    branch_data, branch_data_mut, keccak256_batch, node_state_ref, ArenaParallelSparseTrie,
    ArenaSparseNode, ArenaSparseNodeBranch, ArenaSparseNodeState, ArenaSparseSubtrie,
    HashingStrategy, Index, NodeArena,
};
use crate::{SparseTrieUpdates, TrieNodeEpoch};
use alloc::{boxed::Box, vec::Vec};
use alloy_primitives::B256;
use alloy_rlp::{Encodable, Header, EMPTY_STRING_CODE};
use core::mem;
use reth_trie_common::{BranchNodeMasks, ExtensionNodeRef, LeafNodeRef, Nibbles, RlpNode};

/// One arena whose dirty nodes are hashed by [`hash_levels`].
pub(super) struct LevelJob<'a> {
    /// The arena holding the nodes.
    pub(super) arena: &'a mut NodeArena,
    /// The root node of the arena; must be a branch or extension that is not cached.
    pub(super) root: Index,
    /// Absolute path of the root.
    pub(super) base_path: Nibbles,
    /// Trie updates of the arena, recorded for dirty branches.
    pub(super) updates: &'a mut Option<SparseTrieUpdates>,
}

/// A dirty branch (or extension wrapping one) collected for level-batched hashing.
#[derive(Debug, Clone, Copy)]
struct LevelNode {
    /// Index of the job owning the node.
    job: u32,
    /// Arena index of the branch, or of the extension wrapping it.
    idx: Index,
    /// Absolute path of the node, not including an extension key.
    path: Nibbles,
}

/// A branch encoded in the current level, waiting for its hash.
#[derive(Debug, Clone, Copy)]
struct PendingBranch {
    /// Newest epoch among the branch and its children.
    epoch: TrieNodeEpoch,
    /// Masks computed from the children.
    new_masks: BranchNodeMasks,
    /// Whether the branch was dirty.
    was_dirty: bool,
}

/// Reusable buffers for [`HashingStrategy::LevelBatched`].
#[derive(Debug, Default, Clone)]
pub(super) struct LevelBuffers {
    /// Dirty branches per depth.
    levels: Vec<Vec<LevelNode>>,
    /// Traversal stack.
    stack: Vec<LevelNode>,
    /// Concatenated RLP encodings of the batch being hashed.
    enc: Vec<u8>,
    /// (start, end) of each encoding in enc.
    ranges: Vec<(u32, u32)>,
    /// Hashes of the encodings that are at least 32 bytes long, in order.
    hashes: Vec<B256>,
    /// Leaves encoded in the leaf batch, with their job and epoch.
    leaves: Vec<(u32, Index, TrieNodeEpoch)>,
    /// Branches encoded in the current level.
    pending: Vec<PendingBranch>,
    /// RLP nodes of the current level.
    rlp: Vec<RlpNode>,
}

impl LevelBuffers {
    /// Hashes every encoding in ranges that is at least 32 bytes long and appends the resulting
    /// [`RlpNode`] of every encoding, in order, to out.
    fn hash_ranges(&mut self, out: &mut Vec<RlpNode>) {
        let enc = &self.enc;
        let inputs: Vec<&[u8]> = self
            .ranges
            .iter()
            .map(|&(s, e)| &enc[s as usize..e as usize])
            .filter(|r| r.len() >= 32)
            .collect();
        self.hashes.clear();
        self.hashes.resize(inputs.len(), B256::ZERO);
        keccak256_batch(&inputs, &mut self.hashes);
        let mut h = 0;
        out.reserve(self.ranges.len());
        for &(s, e) in &self.ranges {
            let data = &enc[s as usize..e as usize];
            out.push(if data.len() < 32 {
                RlpNode::from_raw(data).expect("short encoding fits")
            } else {
                h += 1;
                RlpNode::word_rlp(&self.hashes[h - 1])
            });
        }
    }

    /// Starts a new batch of encodings.
    fn clear_batch(&mut self) {
        self.enc.clear();
        self.ranges.clear();
    }
}

/// Returns true if the node at root is a branch or extension whose RLP is not cached, i.e. if
/// it can be hashed by [`hash_levels`].
pub(super) fn root_needs_level_hashing(arena: &NodeArena, root: Index) -> bool {
    matches!(&arena[root], ArenaSparseNode::Branch(_) | ArenaSparseNode::Extension { .. }) &&
        !matches!(node_state_ref(arena, root), Some(ArenaSparseNodeState::Cached { .. }))
}

/// Prefetches every child of the branch (or extension's branch) at idx.
#[inline]
fn prefetch_children(arena: &NodeArena, idx: Index) {
    let branch = branch_data(arena, idx);
    for &child in &branch.children {
        arena.prefetch(child);
    }
}

/// Encodes the leaf at idx into the batch if it is not cached, recording it in lb.leaves.
#[inline]
fn push_leaf(
    lb: &mut LevelBuffers,
    arena: &NodeArena,
    job: u32,
    idx: Index,
    new_epoch: TrieNodeEpoch,
) {
    let ArenaSparseNode::Leaf { key, value, state } = &arena[idx] else { return };
    let epoch = match state {
        ArenaSparseNodeState::Cached { .. } => return,
        ArenaSparseNodeState::Revealed => TrieNodeEpoch::UNMODIFIED,
        ArenaSparseNodeState::Dirty => new_epoch,
    };
    let start = lb.enc.len() as u32;
    LeafNodeRef { key, value }.encode(&mut lb.enc);
    lb.ranges.push((start, lb.enc.len() as u32));
    lb.leaves.push((job, idx, epoch));
}

/// Computes and caches the RLP of every dirty node reachable from the roots of jobs, level by
/// level.
///
/// Descends exactly where [`ArenaParallelSparseTrie::update_cached_rlp_depth_first`] descends
/// (uncached branches and every extension), and produces identical cached [`RlpNode`]s,
/// epochs, branch masks and trie updates.
pub(super) fn hash_levels(
    jobs: &mut [LevelJob<'_>],
    lb: &mut LevelBuffers,
    new_epoch: TrieNodeEpoch,
) {
    // Step 1: collect uncached branches per depth, and encode their uncached leaf children.
    for level in &mut lb.levels {
        level.clear();
    }
    lb.clear_batch();
    lb.leaves.clear();
    let mut max_depth = 0;
    for (job_idx, job) in jobs.iter().enumerate() {
        let job_idx = job_idx as u32;
        let arena = &*job.arena;
        lb.stack.clear();
        prefetch_children(arena, job.root);
        lb.stack.push(LevelNode { job: job_idx, idx: job.root, path: job.base_path });
        while let Some(node) = lb.stack.pop() {
            let branch = branch_data(arena, node.idx);
            if !matches!(branch.state, ArenaSparseNodeState::Cached { .. }) {
                let depth = node.path.len();
                if lb.levels.len() <= depth {
                    lb.levels.resize_with(depth + 1, Vec::new);
                }
                lb.levels[depth].push(node);
                max_depth = max_depth.max(depth);
            }
            let mut child_base = node.path;
            if let Some(short_key) = arena[node.idx].extension_key() {
                child_base.extend(short_key);
            }
            for (nibble, child) in branch.child_iter() {
                let Some(child_idx) = child.revealed_index() else { continue };
                let descend = match &arena[child_idx] {
                    ArenaSparseNode::Branch(b) => {
                        !matches!(b.state, ArenaSparseNodeState::Cached { .. })
                    }
                    ArenaSparseNode::Extension { .. } => true,
                    ArenaSparseNode::Leaf { .. } => {
                        push_leaf(lb, arena, job_idx, child_idx, new_epoch);
                        false
                    }
                    _ => false,
                };
                if descend {
                    // Start loading the grandchildren now; they are inspected when this child
                    // is popped.
                    prefetch_children(arena, child_idx);
                    let mut path = child_base;
                    path.push_unchecked(nibble);
                    lb.stack.push(LevelNode { job: job_idx, idx: child_idx, path });
                }
            }
        }
    }

    // Step 2: hash all leaves as one batch.
    if !lb.leaves.is_empty() {
        let mut rlp = mem::take(&mut lb.rlp);
        rlp.clear();
        lb.hash_ranges(&mut rlp);
        for (&(job, idx, epoch), rlp_node) in lb.leaves.iter().zip(rlp.drain(..)) {
            *jobs[job as usize].arena[idx].state_mut() =
                ArenaSparseNodeState::Cached { rlp_node, epoch };
        }
        lb.rlp = rlp;
    }

    // Step 3: encode and hash branches level by level, deepest first.
    for depth in (0..=max_depth).rev() {
        if lb.levels.get(depth).is_none_or(|level| level.is_empty()) {
            continue;
        }
        let level = mem::take(&mut lb.levels[depth]);

        lb.clear_batch();
        lb.pending.clear();
        let mut has_extensions = false;
        const PREFETCH_AHEAD: usize = 2;
        for node in level.iter().take(PREFETCH_AHEAD) {
            prefetch_children(&*jobs[node.job as usize].arena, node.idx);
        }
        for (i, node) in level.iter().enumerate() {
            if let Some(ahead) = level.get(i + PREFETCH_AHEAD) {
                prefetch_children(&*jobs[ahead.job as usize].arena, ahead.idx);
            }
            let arena = &*jobs[node.job as usize].arena;
            has_extensions |= arena[node.idx].extension_key().is_some();
            let pending = encode_branch(arena, node.idx, new_epoch, &mut lb.enc);
            lb.ranges.push((pending.0, lb.enc.len() as u32));
            lb.pending.push(pending.1);
        }

        // Hash the branches of this level.
        let mut rlp = mem::take(&mut lb.rlp);
        rlp.clear();
        lb.hash_ranges(&mut rlp);

        // Wrap branches behind extensions, and hash the extensions as a second batch.
        if has_extensions {
            lb.clear_batch();
            for (node, branch_rlp) in level.iter().zip(&rlp) {
                if let Some(short_key) = jobs[node.job as usize].arena[node.idx].extension_key() {
                    let start = lb.enc.len() as u32;
                    ExtensionNodeRef::new(short_key, branch_rlp).encode(&mut lb.enc);
                    lb.ranges.push((start, lb.enc.len() as u32));
                }
            }
            let mut ext_rlp = Vec::with_capacity(lb.ranges.len());
            lb.hash_ranges(&mut ext_rlp);
            let mut ext_iter = ext_rlp.into_iter();
            for (node, branch_rlp) in level.iter().zip(rlp.iter_mut()) {
                if jobs[node.job as usize].arena[node.idx].extension_key().is_some() {
                    *branch_rlp = ext_iter.next().expect("one extension rlp per extension");
                }
            }
        }

        // Write back cached state and record trie updates.
        for ((node, pending), rlp_node) in level.iter().zip(&lb.pending).zip(rlp.drain(..)) {
            let job = &mut jobs[node.job as usize];
            let arena = &mut *job.arena;
            let head_idx = node.idx;

            let branch = branch_data_mut(arena, head_idx);
            let prev_masks = branch.branch_masks;
            branch.state = ArenaSparseNodeState::Cached { rlp_node, epoch: pending.epoch };
            branch.branch_masks = pending.new_masks;

            if let Some(trie_updates) = job.updates.as_mut().filter(|_| pending.was_dirty) {
                let mut logical_path = node.path;
                if let Some(short_key) = arena[head_idx].extension_key() {
                    logical_path.extend(short_key);
                }

                if !logical_path.is_empty() {
                    if !prev_masks.is_empty() && pending.new_masks.is_empty() {
                        trie_updates.updated_nodes.remove(&logical_path);
                        trie_updates.removed_nodes.insert(logical_path);
                    } else if !pending.new_masks.is_empty() {
                        let compact = branch_data(arena, head_idx).branch_node_compact(arena);
                        trie_updates.updated_nodes.insert(logical_path, compact);
                        trie_updates.removed_nodes.remove(&logical_path);
                    }
                }
            }
        }

        lb.rlp = rlp;
        lb.levels[depth] = level;
    }
}

/// RLP-encodes the branch at head_idx (whose children must all be cached) into enc, and
/// returns the start offset of the encoding with the branch's new epoch and masks.
///
/// Equivalent to collecting the children's [`RlpNode`]s, computing
/// [`ArenaParallelSparseTrie::get_branch_masks`] and encoding a
/// [`reth_trie_common::BranchNodeRef`].
#[inline]
fn encode_branch(
    arena: &NodeArena,
    head_idx: Index,
    new_epoch: TrieNodeEpoch,
    enc: &mut Vec<u8>,
) -> (u32, PendingBranch) {
    let b = branch_data(arena, head_idx);
    let mut children: [&[u8]; 16] = [&[]; 16];
    let mut payload_length = 1 + 16 - b.state_mask.count_bits() as usize;
    let mut node_epoch = TrieNodeEpoch::UNMODIFIED;
    let mut masks = BranchNodeMasks::default();

    for (nibble, child) in b.child_iter() {
        let (rlp_node, hash_bit, tree_bit) = match child.revealed_index() {
            None => (
                arena.blinded(child),
                b.branch_masks.hash_mask.is_bit_set(nibble),
                b.branch_masks.tree_mask.is_bit_set(nibble),
            ),
            Some(child_idx) => {
                let (state, hash_bit, tree_bit) = match &arena[child_idx] {
                    ArenaSparseNode::Leaf { state, .. } => (state, false, false),
                    ArenaSparseNode::Branch(ArenaSparseNodeBranch {
                        state, branch_masks, ..
                    }) => {
                        let is_hash = state.cached_rlp_node().is_some_and(RlpNode::is_hash);
                        (state, is_hash, !branch_masks.is_empty())
                    }
                    ArenaSparseNode::Extension { child, .. } => {
                        let cb = arena[*child].branch_ref();
                        (&cb.state, false, !cb.branch_masks.is_empty())
                    }
                    ArenaSparseNode::Subtrie(subtrie) => {
                        let root = &subtrie.arena[subtrie.root];
                        let state = node_state_ref(&subtrie.arena, subtrie.root)
                            .expect("subtrie root has a state");
                        (
                            state,
                            root.hash_mask_bit(),
                            super::node_tree_mask_bit(&subtrie.arena, subtrie.root),
                        )
                    }
                    ArenaSparseNode::TakenSubtrie |
                    ArenaSparseNode::EmptyRoot { .. } |
                    ArenaSparseNode::Free => {
                        unreachable!("Unexpected child {:?}", arena[child_idx]);
                    }
                };
                let ArenaSparseNodeState::Cached { rlp_node, epoch } = state else {
                    panic!("revealed child must be cached before its parent level");
                };
                node_epoch = node_epoch.max(*epoch);
                (rlp_node, hash_bit, tree_bit)
            }
        };
        masks.set_child_bits(nibble, hash_bit, tree_bit);
        children[nibble as usize] = rlp_node.as_slice();
        payload_length += rlp_node.len();
    }

    let was_dirty = matches!(b.state, ArenaSparseNodeState::Dirty);
    if was_dirty {
        node_epoch = node_epoch.max(new_epoch);
    }

    let start = enc.len() as u32;
    enc.reserve(payload_length + 3);
    Header { list: true, payload_length }.encode(enc);
    for child in children {
        if child.is_empty() {
            enc.push(EMPTY_STRING_CODE);
        } else {
            enc.extend_from_slice(child);
        }
    }
    enc.push(EMPTY_STRING_CODE);

    (start, PendingBranch { epoch: node_epoch, new_masks: masks, was_dirty })
}

impl ArenaParallelSparseTrie {
    /// Level-batched variant of [`Self::update_cached_rlp_depth_first`] for a single arena
    /// whose root satisfies [`root_needs_level_hashing`].
    pub(super) fn update_cached_rlp_level_batched(
        arena: &mut NodeArena,
        root: Index,
        base_path: Nibbles,
        lb: &mut LevelBuffers,
        updates: &mut Option<SparseTrieUpdates>,
        new_epoch: TrieNodeEpoch,
    ) -> RlpNode {
        hash_levels(&mut [LevelJob { arena, root, base_path, updates }], lb, new_epoch);
        node_state_ref(arena, root)
            .and_then(|state| state.cached_rlp_node())
            .expect("root must be cached after level-batched hashing")
            .clone()
    }

    /// Hashes a group of taken subtries together, level by level, so that each level of the
    /// whole group is hashed as one batch.
    pub(super) fn hash_subtries_level_batched(
        group: &mut [(Index, Box<ArenaSparseSubtrie>)],
        new_epoch: TrieNodeEpoch,
    ) {
        let Some((_, first)) = group.first_mut() else { return };
        let mut lb = mem::take(&mut first.buffers.level);
        {
            let mut jobs = Vec::with_capacity(group.len());
            for (_, subtrie) in group.iter_mut() {
                let subtrie = &mut **subtrie;
                if root_needs_level_hashing(&subtrie.arena, subtrie.root) {
                    jobs.push(LevelJob {
                        arena: &mut subtrie.arena,
                        root: subtrie.root,
                        base_path: subtrie.path,
                        updates: &mut subtrie.buffers.updates,
                    });
                } else {
                    // Leaf, empty or already cached roots need no batching.
                    subtrie.update_cached_rlp(new_epoch, HashingStrategy::DepthFirst);
                }
            }
            hash_levels(&mut jobs, &mut lb, new_epoch);
        }
        for (_, subtrie) in group.iter_mut() {
            subtrie.num_dirty_leaves = 0;
            #[cfg(all(debug_assertions, not(feature = "fuzz")))]
            subtrie.debug_assert_counters();
        }
        group[0].1.buffers.level = lb;
    }
}

#[cfg(test)]
mod tests {
    use super::super::{ArenaParallelSparseTrie, ArenaParallelismThresholds, HashingStrategy};
    use crate::{LeafUpdate, SparseTrie, TrieNodeEpoch};
    use alloy_primitives::{map::B256Map, B256, U256};
    use proptest::prelude::*;
    use rand::{rngs::StdRng, Rng, SeedableRng};
    use reth_trie_common::ProofTrieNode;

    fn apply(trie: &mut ArenaParallelSparseTrie, mut updates: B256Map<LeafUpdate>) {
        trie.update_leaves(&mut updates, |key, _| {
            panic!("fully revealed trie needs no proof for {key}")
        })
        .unwrap();
        assert!(updates.is_empty());
    }

    /// Runs the same random workload through both hashing strategies and asserts identical
    /// roots, trie updates, root epochs and prune results.
    fn assert_strategies_match(
        seed: u64,
        initial: usize,
        rounds: usize,
        per_round: usize,
        thresholds: ArenaParallelismThresholds,
    ) {
        let mut rng = StdRng::seed_from_u64(seed);
        let mut base = ArenaParallelSparseTrie::default()
            .with_parallelism_thresholds(thresholds)
            .with_hashing_strategy(HashingStrategy::DepthFirst);
        base.set_root(&mut [ProofTrieNode::empty()], true).unwrap();
        let mut new = base.clone().with_hashing_strategy(HashingStrategy::LevelBatched);

        let mut keys: Vec<B256> = Vec::new();
        for round in 0..rounds {
            let n = if round == 0 { initial } else { per_round };
            let mut updates = B256Map::default();
            for _ in 0..n {
                let roll: f64 = rng.random();
                let value = alloy_rlp::encode(U256::from(rng.random::<u64>() | 1));
                if roll < 0.45 || keys.is_empty() {
                    // Insert a fresh key, often sharing a long prefix with an existing one to
                    // create extensions and deep branches.
                    let mut key = B256::from(rng.random::<[u8; 32]>());
                    if !keys.is_empty() && rng.random_bool(0.5) {
                        let other = keys[rng.random_range(0..keys.len())];
                        let shared = rng.random_range(1..31);
                        key[..shared].copy_from_slice(&other[..shared]);
                    }
                    keys.push(key);
                    updates.insert(key, LeafUpdate::Changed(value));
                } else if roll < 0.8 {
                    let key = keys[rng.random_range(0..keys.len())];
                    updates.insert(key, LeafUpdate::Changed(value));
                } else {
                    let i = rng.random_range(0..keys.len());
                    let key = keys.swap_remove(i);
                    updates.insert(key, LeafUpdate::Changed(Vec::new()));
                }
            }
            // A key may have been inserted then deleted in the same round; deduplicate keys.
            keys.sort_unstable();
            keys.dedup();
            keys.retain(
                |k| !matches!(updates.get(k), Some(LeafUpdate::Changed(v)) if v.is_empty()),
            );

            apply(&mut base, updates.clone());
            apply(&mut new, updates);

            let epoch = TrieNodeEpoch::new(round as u64 + 1);
            assert_eq!(base.root(epoch), new.root(epoch), "root mismatch in round {round}");
            assert_eq!(base.root_epoch(), new.root_epoch(), "root epoch mismatch in round {round}");
            assert_eq!(
                base.take_updates(),
                new.take_updates(),
                "trie updates mismatch in round {round}"
            );
        }
        for prune_before in (1..=rounds as u64).rev() {
            let mut base = base.clone();
            let mut new = new.clone();
            let epoch = TrieNodeEpoch::new(prune_before);
            assert_eq!(base.prune(epoch), new.prune(epoch), "prune mismatch before {prune_before}");
        }
    }

    const TINY: ArenaParallelismThresholds = ArenaParallelismThresholds {
        min_dirty_leaves: 1,
        min_revealed_nodes: 1,
        min_updates: 1,
        min_leaves_for_prune: 1,
    };

    #[test]
    fn level_batched_matches_depth_first_large() {
        assert_strategies_match(1, 50_000, 6, 5_000, ArenaParallelismThresholds::default());
        assert_strategies_match(2, 20_000, 6, 2_000, TINY);
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(256))]
        #[test]
        fn level_batched_matches_depth_first(
            seed in any::<u64>(),
            initial in 0..1500usize,
            per_round in 1..200usize,
            tiny in any::<bool>(),
        ) {
            let thresholds = if tiny { TINY } else { ArenaParallelismThresholds::default() };
            assert_strategies_match(seed, initial, 5, per_round, thresholds);
        }
    }
}
