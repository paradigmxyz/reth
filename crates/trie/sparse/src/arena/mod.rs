mod branch_child_idx;
mod cursor;
mod nodes;
mod worker_pool;

use branch_child_idx::{BranchChildIdx, BranchChildIter};
use cursor::{ArenaCursor, NextResult, SeekResult};
use nodes::{
    ArenaSparseNode, ArenaSparseNodeBranch, ArenaSparseNodeBranchChild, ArenaSparseNodeState,
};

use crate::{LeafLookup, LeafLookupError, LeafUpdate, SparseTrie, SparseTrieUpdates};
use alloc::{borrow::Cow, boxed::Box, vec::Vec};
use alloy_primitives::{
    keccak256,
    map::{B256Map, HashMap, HashSet},
    B256,
};
use alloy_trie::{BranchNodeCompact, TrieMask};
use core::{cmp::Reverse, mem};
use reth_execution_errors::SparseTrieResult;
use reth_trie_common::{
    BranchNodeMasks, BranchNodeRef, ExtensionNodeRef, LeafNodeRef, Nibbles, ProofTrieNodeV2,
    RlpNode, TrieNodeV2, EMPTY_ROOT_HASH,
};
use slotmap::{DefaultKey, Key as _, SlotMap};
use smallvec::SmallVec;
use tracing::{instrument, trace};

#[cfg(feature = "metrics")]
use reth_primitives_traits::FastInstant as Instant;

#[cfg(feature = "trie-debug")]
use crate::debug_recorder::{LeafUpdateRecord, ProofTrieNodeRecord, RecordedOp, TrieDebugRecorder};

/// Alias for the slotmap key type used as node references throughout the arena trie.
type Index = DefaultKey;
/// Alias for the slotmap used as the node arena throughout the arena trie.
type NodeArena = SlotMap<Index, ArenaSparseNode>;

const TRACE_TARGET: &str = "trie::arena";

/// Wrapper to send raw pointers across threads.
///
/// # Safety
/// The caller must guarantee exclusive access to the pointee for the duration
/// of the send, and that the pointee outlives the receiving thread's use.
struct SendPtr<T>(*mut T);
unsafe impl<T> Send for SendPtr<T> {}

impl<T> SendPtr<T> {
    fn get(self) -> *mut T {
        self.0
    }
}

/// The maximum path length (in nibbles) for nodes that live in the upper trie. Nodes at this
/// depth or deeper belong to lower subtries.
const UPPER_TRIE_MAX_DEPTH: usize = 2;

/// Finds the sub-range of `sorted_keys[start..]` whose entries start with `prefix`.
///
/// Returns the half-open range `start_idx..end_idx` into `sorted_keys`. The returned
/// `end_idx` can be used as the `start` for the next call when iterating prefixes in
/// lexicographic order.
fn prefix_range(
    sorted_keys: &[Nibbles],
    start: usize,
    prefix: &Nibbles,
) -> core::ops::Range<usize> {
    // Advance past entries before `prefix`.
    let begin = start + sorted_keys[start..].partition_point(|p| p < prefix);
    // Find the end of entries that start with `prefix`.
    let mut end = begin;
    while end < sorted_keys.len() && sorted_keys[end].starts_with(prefix) {
        end += 1;
    }
    begin..end
}

/// Reusable buffers shared by both [`ArenaSparseSubtrie`] and [`ArenaParallelSparseTrie`].
#[derive(Debug, Default, Clone)]
struct ArenaTrieBuffers {
    /// Reusable cursor for trie traversals.
    cursor: ArenaCursor,
    /// Trie updates built up directly during hashing and structural changes. `Some` when
    /// tracking updates, `None` otherwise. Initialized alongside `updates` in `set_updates`.
    updates: Option<SparseTrieUpdates>,
    /// Reusable buffer for RLP encoding.
    rlp_buf: Vec<u8>,
    /// Reusable buffer for child `RlpNode`s during hashing.
    rlp_node_buf: Vec<RlpNode>,
}

impl ArenaTrieBuffers {
    fn clear(&mut self) {
        if let Some(updates) = self.updates.as_mut() {
            updates.clear();
        }
        self.rlp_buf.clear();
        self.rlp_node_buf.clear();
    }
}

/// A subtrie within the arena-based parallel sparse trie.
///
/// Each subtrie owns its own arena, allowing parallel mutations across subtries.
#[derive(Debug, Clone)]
struct ArenaSparseSubtrie {
    /// The arena allocating nodes within this subtrie.
    arena: NodeArena,
    /// The root node of this subtrie.
    root: Index,
    /// The absolute path of this subtrie's root in the full trie.
    path: Nibbles,
    /// Reusable buffers for traversal, RLP encoding, and update actions.
    buffers: ArenaTrieBuffers,
    /// Reusable buffer for collecting required proofs during leaf updates.
    /// Each entry is `(index, proof)` where `index` is the position of the target in the
    /// `sorted_updates` slice passed to [`Self::update_leaves`].
    required_proofs: Vec<(usize, ArenaRequiredProof)>,
    /// Total number of revealed leaves in this subtrie.
    num_leaves: u64,
    /// Number of dirty (modified since last hash) leaves in this subtrie.
    num_dirty_leaves: u64,
    /// Metrics for subtrie-level `update_leaves` breakdown.
    #[cfg(feature = "metrics")]
    metrics: crate::metrics::ArenaUpdateLeavesMetrics,
    /// Elapsed time of the last `update_leaves` call (for straggler analysis).
    #[cfg(feature = "metrics")]
    last_update_leaves_elapsed: core::time::Duration,
    /// Timestamp set right before parallel dispatch, so subtrie can measure scheduling delay.
    #[cfg(feature = "metrics")]
    dispatch_timestamp: Instant,
    /// Scheduling delay: time between dispatch and actual start of work.
    #[cfg(feature = "metrics")]
    last_schedule_delay: core::time::Duration,
}

impl ArenaSparseSubtrie {
    /// Asserts that `num_leaves` and `num_dirty_leaves` match the actual counts in the arena.
    #[cfg(debug_assertions)]
    fn debug_assert_counters(&self) {
        let (actual_leaves, actual_dirty) =
            ArenaParallelSparseTrie::count_leaves_and_dirty(&self.arena, self.root);
        debug_assert_eq!(
            self.num_leaves, actual_leaves,
            "subtrie {:?} num_leaves mismatch: stored {} vs actual {}",
            self.path, self.num_leaves, actual_leaves,
        );
        debug_assert_eq!(
            self.num_dirty_leaves, actual_dirty,
            "subtrie {:?} num_dirty_leaves mismatch: stored {} vs actual {}",
            self.path, self.num_dirty_leaves, actual_dirty,
        );
    }

    fn clear(&mut self) {
        self.arena.clear();
        self.buffers.clear();
        self.required_proofs.clear();
        self.num_leaves = 0;
        self.num_dirty_leaves = 0;
    }

    /// Prunes revealed subtrees that are not ancestors of any retained leaf.
    ///
    /// `retained_leaves` must be sorted and scoped to this subtrie's key range. The method
    /// walks all nodes depth-first with the cursor, removing non-retained nodes bottom-up.
    /// Only the boundary nodes (direct children of retained branches) have their parent's
    /// child slot replaced with `Blinded`; deeper nodes are simply removed since their parent
    /// will also be removed.
    ///
    /// Expects that all nodes have computed hashes (i.e. `prune` is called after hashing).
    fn prune(&mut self, retained_leaves: &[Nibbles]) -> usize {
        // Only branches can have pruneable children.
        if !matches!(&self.arena[self.root], ArenaSparseNode::Branch(_)) {
            return 0;
        }

        debug_assert!(
            retained_leaves.windows(2).all(|w| w[0] <= w[1]),
            "retained_leaves must be sorted"
        );

        let mut pruned: usize = 0;
        let mut pruned_leaves: u64 = 0;

        self.buffers.cursor.reset(&self.arena, self.root, self.path);

        loop {
            let result = self.buffers.cursor.next(&mut self.arena, |_, node| {
                matches!(node, ArenaSparseNode::Branch(_) | ArenaSparseNode::Leaf { .. })
            });

            match result {
                NextResult::Done => break,
                NextResult::NonBranch | NextResult::Branch => {
                    // Don't prune the root.
                    if self.buffers.cursor.depth() == 0 {
                        continue;
                    }

                    let head = self.buffers.cursor.head().expect("cursor is non-empty");
                    let head_idx = head.index;
                    let nibble = head.path.last();

                    // Compute the node's key prefix for the retention check. We use
                    // `short_key()` directly rather than `head_logical_branch_path()`
                    // because the head can be a leaf.
                    let short_key =
                        self.arena[head_idx].short_key().expect("must be branch or leaf");
                    let mut node_prefix = head.path;
                    node_prefix.extend(short_key);

                    // Check if this node (or any descendant) is retained. Always search
                    // from 0 because DFS order is not lexicographic — backtracking can
                    // revisit prefixes earlier than previously visited subtrees.
                    let range = prefix_range(retained_leaves, 0, &node_prefix);
                    if !range.is_empty() {
                        continue;
                    }

                    if matches!(self.arena[head_idx], ArenaSparseNode::Leaf { .. }) {
                        pruned_leaves += 1;
                    }

                    ArenaParallelSparseTrie::remove_pruned_node(
                        &mut self.arena,
                        &self.buffers.cursor,
                        head_idx,
                        nibble,
                    );
                    pruned += 1;
                }
            }
        }

        self.num_leaves -= pruned_leaves;
        #[cfg(debug_assertions)]
        self.debug_assert_counters();
        pruned
    }

    /// Applies leaf updates within this subtrie. Uses the same walk-down-with-cursor pattern as
    /// [`Self::reveal_nodes`], but checks accessibility for [`LeafUpdate::Touched`] entries.
    ///
    /// `sorted_updates` must be sorted lexicographically by their nibbles path (index 1).
    ///
    /// Any required proofs are appended to `self.required_proofs` and should be drained by the
    /// caller after this method returns.
    #[instrument(
        level = "trace",
        target = TRACE_TARGET,
        skip_all,
        fields(
            subtrie = ?self.path,
            num_updates = sorted_updates.len(),
        ),
    )]
    fn update_leaves(&mut self, sorted_updates: &[(B256, Nibbles, LeafUpdate)]) {
        if sorted_updates.is_empty() {
            return;
        }
        trace!(target: TRACE_TARGET, "Subtrie update_leaves");

        debug_assert!(
            !matches!(self.arena[self.root], ArenaSparseNode::EmptyRoot),
            "subtrie root must not be EmptyRoot at start of update_leaves"
        );

        self.buffers.cursor.reset(&self.arena, self.root, self.path);

        #[cfg(feature = "metrics")]
        let subtrie_update_start = Instant::now();
        #[cfg(feature = "metrics")]
        {
            self.last_schedule_delay = subtrie_update_start - self.dispatch_timestamp;
        }
        #[cfg(feature = "metrics")]
        let mut seek_elapsed = core::time::Duration::ZERO;
        #[cfg(feature = "metrics")]
        let mut upsert_elapsed = core::time::Duration::ZERO;
        #[cfg(feature = "metrics")]
        let mut remove_elapsed = core::time::Duration::ZERO;
        #[cfg(feature = "metrics")]
        let mut seek_count = 0u64;
        #[cfg(feature = "metrics")]
        let mut upsert_count = 0u64;
        #[cfg(feature = "metrics")]
        let mut remove_count = 0u64;

        for (idx, &(key, ref full_path, ref update)) in sorted_updates.iter().enumerate() {
            #[cfg(feature = "metrics")]
            let seek_start = Instant::now();
            let find_result = self.buffers.cursor.seek(&mut self.arena, full_path);
            #[cfg(feature = "metrics")]
            {
                seek_elapsed += seek_start.elapsed();
                seek_count += 1;
            }

            // If the path hits a blinded node, request a proof regardless of update type.
            if matches!(find_result, SeekResult::Blinded) {
                let logical_len = self.buffers.cursor.head_logical_branch_path_len(&self.arena);
                self.required_proofs.push((
                    idx,
                    ArenaRequiredProof { key, min_len: (logical_len as u8 + 1).min(64) },
                ));
                continue;
            }

            match update {
                LeafUpdate::Changed(value) if !value.is_empty() => {
                    // Upsert: insert or update a leaf with the given value.
                    #[cfg(feature = "metrics")]
                    let upsert_start = Instant::now();
                    let (_result, deltas) = ArenaParallelSparseTrie::upsert_leaf(
                        &mut self.arena,
                        &mut self.buffers.cursor,
                        &mut self.root,
                        full_path,
                        value,
                        find_result,
                    );
                    #[cfg(feature = "metrics")]
                    {
                        upsert_elapsed += upsert_start.elapsed();
                        upsert_count += 1;
                    }
                    self.num_leaves = (self.num_leaves as i64 + deltas.num_leaves_delta) as u64;
                    self.num_dirty_leaves =
                        (self.num_dirty_leaves as i64 + deltas.num_dirty_leaves_delta) as u64;
                }
                LeafUpdate::Changed(_) => {
                    #[cfg(feature = "metrics")]
                    let remove_start = Instant::now();
                    let (result, deltas) = ArenaParallelSparseTrie::remove_leaf(
                        &mut self.arena,
                        &mut self.buffers.cursor,
                        &mut self.root,
                        key,
                        full_path,
                        find_result,
                        &mut self.buffers.updates,
                    );
                    #[cfg(feature = "metrics")]
                    {
                        remove_elapsed += remove_start.elapsed();
                        remove_count += 1;
                    }
                    self.num_leaves = (self.num_leaves as i64 + deltas.num_leaves_delta) as u64;
                    self.num_dirty_leaves =
                        (self.num_dirty_leaves as i64 + deltas.num_dirty_leaves_delta) as u64;

                    if let RemoveLeafResult::NeedsProof { key, proof_key, min_len } = result {
                        self.required_proofs
                            .push((idx, ArenaRequiredProof { key: proof_key, min_len }));
                        self.required_proofs.push((idx, ArenaRequiredProof { key, min_len }));
                    }
                }
                LeafUpdate::Touched => {}
            }
        }

        // Drain remaining cursor entries, propagating dirty state.
        #[cfg(feature = "metrics")]
        let drain_start = Instant::now();
        self.buffers.cursor.drain(&mut self.arena);
        #[cfg(feature = "metrics")]
        {
            self.metrics.subtrie_update_leaves_seek_latency.record(seek_elapsed);
            self.metrics.subtrie_update_leaves_upsert_latency.record(upsert_elapsed);
            self.metrics.subtrie_update_leaves_remove_latency.record(remove_elapsed);
            self.metrics.subtrie_update_leaves_drain_latency.record(drain_start.elapsed());
            self.metrics.subtrie_update_leaves_seek_count.record(seek_count as f64);
            self.metrics.subtrie_update_leaves_upsert_count.record(upsert_count as f64);
            self.metrics.subtrie_update_leaves_remove_count.record(remove_count as f64);
            self.last_update_leaves_elapsed = subtrie_update_start.elapsed();

            // Uncomment to find out about the outliers:
            // 
            // if self.last_update_leaves_elapsed.as_micros() >= 20 {
            //     tracing::info!(
            //         target: "reth::trie::subtrie_profile",
            //         elapsed_us = self.last_update_leaves_elapsed.as_micros() as u64,
            //         schedule_delay_us = self.last_schedule_delay.as_micros() as u64,
            //         thread_id = ?std::thread::current().id(),
            //         num_updates = sorted_updates.len(),
            //         arena_nodes = self.arena.len(),
            //         num_leaves = self.num_leaves,
            //         seek_count,
            //         upsert_count,
            //         remove_count,
            //         seek_us = seek_elapsed.as_micros() as u64,
            //         upsert_us = upsert_elapsed.as_micros() as u64,
            //         remove_us = remove_elapsed.as_micros() as u64,
            //         drain_us = drain_start.elapsed().as_micros() as u64,
            //         subtrie = ?self.path,
            //         "heavy subtrie update_leaves"
            //     );
            // }
        }

        #[cfg(debug_assertions)]
        self.debug_assert_counters();
    }

    /// Reveals nodes inside this subtrie. Uses [`ArenaCursor::seek`] to locate the ancestor
    /// node, then replaces blinded children with the proof nodes.
    fn reveal_nodes(&mut self, nodes: &mut [ProofTrieNodeV2]) -> SparseTrieResult<()> {
        if nodes.is_empty() {
            return Ok(());
        }
        trace!(target: TRACE_TARGET, path = ?self.path, num_nodes = nodes.len(), "Subtrie reveal_nodes");

        debug_assert!(
            !matches!(self.arena[self.root], ArenaSparseNode::EmptyRoot),
            "subtrie root must not be EmptyRoot in reveal_nodes"
        );

        self.buffers.cursor.reset(&self.arena, self.root, self.path);

        for node in nodes.iter_mut() {
            let find_result = self.buffers.cursor.seek(&mut self.arena, &node.path);
            if ArenaParallelSparseTrie::reveal_node(
                &mut self.arena,
                &self.buffers.cursor,
                node,
                find_result,
            )
            .is_some_and(|child_idx| matches!(self.arena[child_idx], ArenaSparseNode::Leaf { .. }))
            {
                self.num_leaves += 1;
            }
        }

        // Drain remaining cursor entries, propagating dirty state.
        self.buffers.cursor.drain(&mut self.arena);

        #[cfg(debug_assertions)]
        self.debug_assert_counters();

        Ok(())
    }

    /// Computes and caches `RlpNode` for all dirty nodes via iterative post-order DFS.
    /// After this call every node reachable from `self.root` will be in `Cached` state.
    ///
    /// Trie updates are written directly to `self.buffers.updates` (if `Some`).
    fn update_cached_rlp(&mut self) {
        ArenaParallelSparseTrie::update_cached_rlp(
            &mut self.arena,
            self.root,
            &mut self.buffers.cursor,
            &mut self.buffers.rlp_buf,
            &mut self.buffers.rlp_node_buf,
            self.path,
            &mut self.buffers.updates,
        );
        self.num_dirty_leaves = 0;
        #[cfg(debug_assertions)]
        self.debug_assert_counters();
    }
}

/// Tracks the net change in leaf counters caused by a trie mutation (upsert or removal).
/// Returned alongside [`UpsertLeafResult`] / [`RemoveLeafResult`] so the caller can maintain
/// aggregate counters on [`ArenaSparseSubtrie`] without scanning the arena.
#[derive(Debug, Default)]
struct SubtrieCounterDeltas {
    num_leaves_delta: i64,
    num_dirty_leaves_delta: i64,
}

/// Result of `upsert_leaf` indicating whether a new child was created that the caller
/// may need to wrap as a subtrie (in the upper trie).
#[derive(Debug)]
enum UpsertLeafResult {
    /// A leaf was updated in place (no structural change).
    Updated,
    /// A new leaf was created (e.g. EmptyRoot→Leaf, or root-level split).
    NewLeaf,
    /// A new child (branch or leaf) was created or inserted. The child is the cursor head
    /// and its parent is the cursor's parent.
    NewChild,
}

/// Result of `remove_leaf` indicating whether a proof is needed to complete a branch
/// collapse.
#[derive(Debug)]
enum RemoveLeafResult {
    /// No proof needed — the removal (and any collapse) completed fully.
    Removed,
    /// No leaf was found at the given path (no-op).
    NotFound,
    /// The branch collapse requires revealing a blinded sibling. The caller must request a
    /// proof for the given key at the given minimum depth.
    NeedsProof { key: B256, proof_key: B256, min_len: u8 },
}

/// A proof request generated during leaf updates when a blinded node is encountered.
#[derive(Debug, Clone)]
struct ArenaRequiredProof {
    /// The key requiring a proof.
    key: B256,
    /// Minimum depth at which proof nodes should be returned.
    min_len: u8,
}

/// Info about a taken subtrie, used during post-parallel processing.
struct TakenInfo {
    /// The absolute path of the subtrie root.
    path: Nibbles,
    /// The range into the sorted updates array for this subtrie.
    range: core::ops::Range<usize>,
    /// Whether the subtrie became empty after processing.
    is_empty: bool,
}

/// An arena-based parallel sparse trie.
///
/// Configuration for controlling when parallelism is enabled in [`ArenaParallelSparseTrie`]
/// operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArenaParallelismThresholds {
    /// Minimum number of dirty leaves in a subtrie before it is eligible for parallel hash
    /// computation. Subtries with fewer dirty leaves than this are hashed serially during
    /// [`ArenaParallelSparseTrie::update_subtrie_hashes`].
    pub min_dirty_leaves: u64,
    /// Minimum number of nodes to reveal in a subtrie before it is eligible for parallel
    /// reveal. Subtries with fewer nodes to reveal than this are revealed inline during the
    /// upper trie walk.
    pub min_revealed_nodes: usize,
    /// Minimum number of leaf updates targeting a subtrie before it is eligible for parallel
    /// update. Subtries with fewer updates than this are updated inline during the upper trie
    /// walk.
    pub min_updates: usize,
    /// Minimum number of revealed leaves in a subtrie before it is eligible for parallel
    /// pruning. Subtries with fewer leaves than this are pruned inline during the upper trie
    /// walk.
    pub min_leaves_for_prune: u64,
}

impl Default for ArenaParallelismThresholds {
    fn default() -> Self {
        Self {
            min_dirty_leaves: 64,
            min_revealed_nodes: 16,
            min_updates: 50,
            min_leaves_for_prune: 128,
        }
    }
}

/// An arena-based sparse trie whose subtries can be mutated in parallel.
///
/// ## Structure
///
/// Uses arena allocation ([`slotmap::SlotMap`]) for node storage with direct index-based child
/// pointers, avoiding the per-node hashing overhead of a `HashMap`-based trie. The trie is split
/// into two tiers:
///
/// - **Upper trie** (`upper_arena`): Contains nodes whose path is shorter than
///   `UPPER_TRIE_MAX_DEPTH` nibbles. These are the root and its immediate children.
/// - **Lower subtries** (`ArenaSparseSubtrie`): Each child of an upper-trie branch at the depth
///   boundary becomes the root of its own subtrie, stored as an `ArenaSparseNode::Subtrie` child in
///   the upper arena. Each subtrie owns its own arena, enabling lock-free parallel mutation.
///
/// Node placement is determined by path length (not counting a branch's short key):
///
/// - Paths with **< `UPPER_TRIE_MAX_DEPTH`** nibbles live in `upper_arena`.
/// - Paths with **≥ `UPPER_TRIE_MAX_DEPTH`** nibbles live in a subtrie.
///
/// ## Node Revealing
///
/// Nodes are lazily revealed from proof data via [`SparseTrie::reveal_nodes`]. Each node is
/// placed into the upper arena or delegated to its subtrie based on path depth. Unrevealed
/// children are stored as `ArenaSparseNodeBranchChild::Blinded` with their RLP encoding.
/// When multiple subtries have pending reveals, they are processed in parallel using rayon
/// (controlled by [`ArenaParallelismThresholds::min_revealed_nodes`]).
///
/// ## Leaf Operations
///
/// Leaf updates and removals are applied via [`SparseTrie::update_leaves`]. The method walks
/// the upper trie to route each update to the correct subtrie, then processes subtries in
/// parallel when the update count exceeds [`ArenaParallelismThresholds::min_updates`].
///
/// [`SparseTrie::update_leaf`] and [`SparseTrie::remove_leaf`] are not yet implemented.
///
/// After updates, structural changes (branch collapse, subtrie unwrapping) are handled by
/// propagating dirty state back up through the upper trie.
///
/// ## Root Hash Calculation
///
/// Root hash computation follows a bottom-up approach:
///
/// 1. **[`SparseTrie::update_subtrie_hashes`]**: Takes dirty subtries from the upper arena and
///    hashes them in parallel (when dirty leaf count meets
///    [`ArenaParallelismThresholds::min_dirty_leaves`]), then walks the upper trie to restore
///    hashed subtries and inline-hash any remaining dirty nodes.
/// 2. **[`SparseTrie::root`]**: Calls `update_subtrie_hashes`, then RLP-encodes the full upper trie
///    depth-first to produce the root hash.
///
/// Each node tracks its state via `ArenaSparseNodeState` (`Revealed`, `Cached`, or `Dirty`)
/// so only modified subtrees are recomputed.
///
/// ## Pruning
///
/// [`SparseTrie::prune`] removes revealed nodes that are not ancestors of any retained leaf.
/// Pruned nodes are replaced with `ArenaSparseNodeBranchChild::Blinded` entries using their
/// cached RLP. Subtries are pruned in parallel when their leaf count exceeds
/// [`ArenaParallelismThresholds::min_leaves_for_prune`].
///
/// ## Subtrie Recycling
///
/// Cleared subtries are pooled in `cleared_subtries` and reused by
/// `take_or_create_cleared_subtrie` to avoid repeated arena allocations.
#[derive(Debug)]
pub struct ArenaParallelSparseTrie {
    /// The arena allocating nodes in the upper trie.
    upper_arena: NodeArena,
    /// The root node of the upper trie.
    root: Index,
    /// Optional tracking of trie updates for database persistence.
    updates: Option<SparseTrieUpdates>,
    /// Reusable buffers for traversal, RLP encoding, and update actions.
    buffers: ArenaTrieBuffers,
    /// Pool of cleared `ArenaSparseSubtrie`s available for reuse.
    #[allow(clippy::vec_box)]
    cleared_subtries: Vec<Box<ArenaSparseSubtrie>>,
    /// Thresholds controlling when parallelism is enabled for different operations.
    parallelism_thresholds: ArenaParallelismThresholds,
    /// Persistent thread pool for parallel subtrie updates (2 workers).
    worker_pool: worker_pool::SubtrieWorkerPool,
    /// Metrics for `update_leaves` breakdown.
    #[cfg(feature = "metrics")]
    metrics: crate::metrics::ArenaUpdateLeavesMetrics,
    /// Debug recorder for tracking mutating operations.
    #[cfg(feature = "trie-debug")]
    debug_recorder: TrieDebugRecorder,
}

impl ArenaParallelSparseTrie {
    /// Sets the thresholds that control when parallelism is used during operations.
    pub const fn with_parallelism_thresholds(
        mut self,
        thresholds: ArenaParallelismThresholds,
    ) -> Self {
        self.parallelism_thresholds = thresholds;
        self
    }

    /// Returns the arena indexes of all [`ArenaSparseNode::Subtrie`] nodes in the upper arena.
    fn all_subtries(&self) -> SmallVec<[Index; 16]> {
        self.upper_arena
            .iter()
            .filter_map(|(idx, node)| matches!(node, ArenaSparseNode::Subtrie(_)).then_some(idx))
            .collect()
    }

    /// Resets the debug recorder and records the current trie state as `SetRoot` + `RevealNodes`
    /// ops, representing the initial state at the beginning of a block (after pruning).
    ///
    /// Walks the upper arena and all subtries depth-first using the cursor, converting each
    /// node into a [`crate::debug_recorder::ProofTrieNodeRecord`].
    #[cfg(feature = "trie-debug")]
    fn record_initial_state(&mut self) {
        use crate::debug_recorder::{NodeStateRecord, TrieNodeRecord};
        use alloy_primitives::hex;
        use alloy_trie::nodes::{BranchNode, TrieNode};

        fn state_to_record(state: &ArenaSparseNodeState) -> NodeStateRecord {
            match state {
                ArenaSparseNodeState::Revealed => NodeStateRecord::Revealed,
                ArenaSparseNodeState::Cached { rlp_node } => {
                    NodeStateRecord::Cached { rlp_node: hex::encode(rlp_node.as_ref()) }
                }
                ArenaSparseNodeState::Dirty => NodeStateRecord::Dirty,
            }
        }

        /// Converts an [`ArenaSparseNode`] into a [`ProofTrieNodeRecord`] at the given path.
        /// For branch children, resolves revealed children's cached RLP from `arena`.
        /// Returns `None` for subtrie/taken-subtrie nodes (handled separately).
        fn node_to_record(
            arena: &NodeArena,
            idx: Index,
            path: Nibbles,
        ) -> Option<ProofTrieNodeRecord> {
            match &arena[idx] {
                ArenaSparseNode::EmptyRoot => Some(ProofTrieNodeRecord {
                    path,
                    node: TrieNodeRecord(TrieNode::EmptyRoot),
                    masks: None,
                    short_key: None,
                    state: None,
                }),
                ArenaSparseNode::Branch(b) => {
                    let stack = b
                        .children
                        .iter()
                        .map(|child| match child {
                            ArenaSparseNodeBranchChild::Blinded(rlp) => rlp.clone(),
                            ArenaSparseNodeBranchChild::Revealed(child_idx) => {
                                // After pruning / root(), all nodes have cached RLP.
                                arena[*child_idx]
                                    .state_ref()
                                    .and_then(|s| s.cached_rlp_node())
                                    .cloned()
                                    .unwrap_or_default()
                            }
                        })
                        .collect();
                    Some(ProofTrieNodeRecord {
                        path,
                        node: TrieNodeRecord(TrieNode::Branch(BranchNode::new(
                            stack,
                            b.state_mask,
                        ))),
                        masks: Some((
                            b.branch_masks.hash_mask.get(),
                            b.branch_masks.tree_mask.get(),
                        )),
                        short_key: (!b.short_key.is_empty()).then_some(b.short_key),
                        state: Some(state_to_record(&b.state)),
                    })
                }
                ArenaSparseNode::Leaf { key, value, state, .. } => Some(ProofTrieNodeRecord {
                    path,
                    node: TrieNodeRecord(TrieNode::Leaf(alloy_trie::nodes::LeafNode::new(
                        *key,
                        value.clone(),
                    ))),
                    masks: None,
                    short_key: None,
                    state: Some(state_to_record(state)),
                }),
                ArenaSparseNode::Subtrie(_) | ArenaSparseNode::TakenSubtrie => None,
            }
        }

        /// Walks an arena depth-first using `cursor` and collects all nodes as records.
        fn collect_records(
            arena: &mut NodeArena,
            root: Index,
            root_path: Nibbles,
            cursor: &mut ArenaCursor,
            result: &mut Vec<ProofTrieNodeRecord>,
        ) {
            cursor.reset(arena, root, root_path);

            // The cursor starts with root on the stack but `next` only yields children.
            if let Some(record) = node_to_record(arena, root, root_path) {
                result.push(record);
            }

            loop {
                match cursor.next(arena, |_, node| {
                    matches!(node, ArenaSparseNode::Branch(_) | ArenaSparseNode::Leaf { .. })
                }) {
                    NextResult::Done => break,
                    NextResult::Branch | NextResult::NonBranch => {
                        let head = cursor.head().expect("cursor is non-empty");
                        if let Some(record) = node_to_record(arena, head.index, head.path) {
                            result.push(record);
                        }
                    }
                }
            }
        }

        let mut nodes = Vec::new();

        // Collect from the upper arena.
        collect_records(
            &mut self.upper_arena,
            self.root,
            Nibbles::default(),
            &mut self.buffers.cursor,
            &mut nodes,
        );

        // Collect from all subtries.
        for (_, node) in &mut self.upper_arena {
            if let ArenaSparseNode::Subtrie(subtrie) = node {
                collect_records(
                    &mut subtrie.arena,
                    subtrie.root,
                    subtrie.path,
                    &mut self.buffers.cursor,
                    &mut nodes,
                );
            }
        }

        // Reset the recorder and record that we pruned, then the initial state.
        self.debug_recorder.reset();
        self.debug_recorder.record(RecordedOp::Prune);

        // First node is the root → SetRoot, remaining → RevealNodes.
        if let Some(root_record) = nodes.first() {
            self.debug_recorder.record(RecordedOp::SetRoot { node: root_record.clone() });
        }
        if nodes.len() > 1 {
            self.debug_recorder.record(RecordedOp::RevealNodes { nodes: nodes[1..].to_vec() });
        }
    }

    /// Takes a cleared [`ArenaSparseSubtrie`] from the pool (or creates a new one) and
    /// pre-allocates a root slot with a placeholder. The caller must overwrite
    /// `subtrie.arena[subtrie.root]` before use.
    fn take_or_create_cleared_subtrie(&mut self) -> Box<ArenaSparseSubtrie> {
        let mut subtrie = if let Some(s) = self.cleared_subtries.pop() {
            debug_assert!(s.arena.is_empty());
            s
        } else {
            Box::new(ArenaSparseSubtrie {
                arena: SlotMap::new(),
                root: Index::null(),
                path: Nibbles::default(),
                buffers: ArenaTrieBuffers::default(),
                required_proofs: Vec::new(),
                num_leaves: 0,
                num_dirty_leaves: 0,
                #[cfg(feature = "metrics")]
                metrics: Default::default(),
                #[cfg(feature = "metrics")]
                last_update_leaves_elapsed: core::time::Duration::ZERO,
                #[cfg(feature = "metrics")]
                dispatch_timestamp: Instant::now(),
                #[cfg(feature = "metrics")]
                last_schedule_delay: core::time::Duration::ZERO,
            })
        };
        subtrie.root = subtrie.arena.insert(ArenaSparseNode::EmptyRoot);
        if self.updates.is_some() {
            subtrie.buffers.updates.get_or_insert_with(SparseTrieUpdates::default).clear();
        }
        subtrie
    }

    /// Returns `true` if a node at the given path length should be placed in a subtrie rather
    /// than the upper arena.
    const fn should_be_subtrie(path_len: usize) -> bool {
        path_len == UPPER_TRIE_MAX_DEPTH
    }

    /// If the child at the cursor head should be a subtrie based on its depth, wraps it
    /// in [`ArenaSparseNode::Subtrie`].
    ///
    /// The child must be the cursor head and its parent the cursor's parent.
    fn maybe_wrap_in_subtrie(&mut self, child_idx: Index, child_path: &Nibbles) {
        if !Self::should_be_subtrie(child_path.len()) {
            return;
        }

        // Only branch and leaf nodes can become subtrie roots.
        if !matches!(
            self.upper_arena[child_idx],
            ArenaSparseNode::Branch(_) | ArenaSparseNode::Leaf { .. }
        ) {
            return;
        }

        trace!(target: TRACE_TARGET, ?child_path, "Wrapping child into subtrie");
        let mut subtrie = self.take_or_create_cleared_subtrie();
        subtrie.path = *child_path;
        let mut root_node =
            mem::replace(&mut self.upper_arena[child_idx], ArenaSparseNode::TakenSubtrie);

        // Migrate any revealed children from the upper arena into the subtrie arena.
        if let ArenaSparseNode::Branch(b) = &mut root_node {
            for child in &mut b.children {
                if let ArenaSparseNodeBranchChild::Revealed(idx) = child {
                    *idx =
                        Self::migrate_nodes(&mut subtrie.arena, &mut self.upper_arena, *idx, None);
                }
            }
        }

        subtrie.arena[subtrie.root] = root_node;
        let (leaves, dirty) = Self::count_leaves_and_dirty(&subtrie.arena, subtrie.root);
        subtrie.num_leaves = leaves;
        subtrie.num_dirty_leaves = dirty;
        #[cfg(debug_assertions)]
        subtrie.debug_assert_counters();
        self.upper_arena[child_idx] = ArenaSparseNode::Subtrie(subtrie);
    }

    /// If the cursor head is a branch, wraps any revealed children that sit at
    /// the subtrie boundary depth (`UPPER_TRIE_MAX_DEPTH`). This is needed after
    /// structural changes like root-level splits or subtrie unwraps that can place
    /// non-subtrie nodes at the boundary depth.
    fn maybe_wrap_branch_children(&mut self, cursor: &ArenaCursor) {
        let head = cursor.head().expect("cursor is non-empty");
        let head_idx = head.index;
        let head_path = head.path;

        let ArenaSparseNode::Branch(b) = &self.upper_arena[head_idx] else { return };
        let short_key = b.short_key;
        let children: SmallVec<[_; 4]> = b
            .child_iter()
            .filter_map(|(nibble, child)| match child {
                ArenaSparseNodeBranchChild::Revealed(idx) => Some((nibble, *idx)),
                ArenaSparseNodeBranchChild::Blinded(_) => None,
            })
            .collect();

        for (nibble, child_idx) in children {
            let mut child_path = head_path;
            child_path.extend(&short_key);
            child_path.push_unchecked(nibble);
            self.maybe_wrap_in_subtrie(child_idx, &child_path);
        }
    }

    /// Checks whether the subtrie at the cursor head has become empty after updates.
    /// If the subtrie's root is [`ArenaSparseNode::EmptyRoot`] (all leaves were removed), the
    /// child slot is removed from the parent branch entirely, the subtrie is recycled, and
    /// if the parent is left with a single revealed child, it is collapsed via
    /// `collapse_branch`.
    ///
    /// The subtrie must be the cursor head and its parent the cursor's parent.
    /// Pops the subtrie entry (propagating leaf count deltas) before returning.
    #[instrument(
        level = "trace",
        target = TRACE_TARGET,
        skip_all,
        fields(subtrie_path = ?cursor.head().expect("cursor is non-empty").path),
    )]
    fn maybe_unwrap_subtrie(&mut self, cursor: &mut ArenaCursor) {
        let subtrie_idx = cursor.head().expect("cursor is non-empty").index;

        let ArenaSparseNode::Subtrie(subtrie) = &self.upper_arena[subtrie_idx] else {
            return;
        };

        if !matches!(subtrie.arena[subtrie.root], ArenaSparseNode::EmptyRoot) {
            return;
        }

        let child_nibble = cursor
            .head()
            .expect("cursor is non-empty")
            .path
            .last()
            .expect("subtrie path must have at least one nibble");
        let parent_idx = cursor.parent().expect("cursor has parent").index;

        // Pop the subtrie entry before mutating, so collapse_branch sees the parent as
        // the cursor head.
        cursor.pop(&mut self.upper_arena);

        self.recycle_subtrie_from_idx(subtrie_idx);

        trace!(target: TRACE_TARGET, "Unwrapping empty subtrie, removing child slot");
        let parent_branch = self.upper_arena[parent_idx].branch_mut();
        let child_idx = BranchChildIdx::new(parent_branch.state_mask, child_nibble)
            .expect("child nibble not found in parent state_mask");

        parent_branch.children.remove(child_idx.get());
        parent_branch.unset_child_bit(child_nibble);
        // The branch structure changed (child removed), so any cached RLP is stale.
        parent_branch.state = parent_branch.state.to_dirty();

        self.maybe_collapse_or_remove_branch(cursor);
    }

    /// Merges buffered updates from a [`ArenaSparseNode::Subtrie`], clears it, and pushes it
    /// onto `cleared_subtries` for reuse.
    ///
    /// # Panics
    ///
    /// Panics if `node` is not a `Subtrie`.
    fn recycle_subtrie(&mut self, node: ArenaSparseNode) {
        let ArenaSparseNode::Subtrie(mut subtrie) = node else {
            unreachable!("recycle_subtrie called on non-Subtrie node")
        };
        Self::merge_subtrie_updates(&mut self.buffers.updates, &mut subtrie.buffers.updates);
        subtrie.clear();
        self.cleared_subtries.push(subtrie);
    }

    /// Removes a [`ArenaSparseNode::Subtrie`] from the upper arena at `idx` and recycles it.
    fn recycle_subtrie_from_idx(&mut self, idx: Index) {
        let node = self.upper_arena.remove(idx).expect("subtrie exists in arena");
        self.recycle_subtrie(node);
    }

    /// Handles cascading structural changes on the branch at the cursor head after a child
    /// has been removed.
    ///
    /// Depending on the remaining child count:
    /// - **0 children**: the branch becomes `EmptyRoot` (if root) or is removed from its parent,
    ///   cascading upward.
    /// - **1 child**: collapses the branch into its sole child, unless that child is a
    ///   `TakenSubtrie` (deferred) or blinded. If the remaining child is an empty subtrie, it is
    ///   also removed, reducing to the 0-children case.
    /// - **2+ children**: nothing to do.
    fn maybe_collapse_or_remove_branch(&mut self, cursor: &mut ArenaCursor) {
        loop {
            let branch_idx = cursor.head().expect("cursor is non-empty").index;

            // Read-only phase: extract the count and remaining-child info we need before
            // mutating. All values here are Copy so the borrow is released.
            let count = {
                let ArenaSparseNode::Branch(b) = &self.upper_arena[branch_idx] else {
                    return;
                };
                b.state_mask.count_bits()
            };

            if count >= 2 {
                return;
            }

            if count == 0 {
                if branch_idx == self.root {
                    self.upper_arena[branch_idx] = ArenaSparseNode::EmptyRoot;
                    return;
                }
                // Remove the empty branch from its parent.
                let branch_nibble = cursor
                    .head()
                    .expect("cursor is non-empty")
                    .path
                    .last()
                    .expect("non-root branch");
                cursor.pop(&mut self.upper_arena);
                self.upper_arena.remove(branch_idx);
                let parent_idx = cursor.head().expect("cursor is non-empty").index;
                let parent_branch = self.upper_arena[parent_idx].branch_mut();
                let child_idx = BranchChildIdx::new(parent_branch.state_mask, branch_nibble)
                    .expect("child nibble not found in parent state_mask");
                parent_branch.children.remove(child_idx.get());
                parent_branch.unset_child_bit(branch_nibble);
                parent_branch.state = parent_branch.state.to_dirty();
                continue; // re-check the parent
            }

            // count == 1 — determine what kind of child remains.
            let (remaining_nibble, remaining_child_idx) = {
                let b = self.upper_arena[branch_idx].branch_ref();
                let nibble = b.state_mask.iter().next().expect("branch has at least one child");
                let child_idx = match &b.children[0] {
                    ArenaSparseNodeBranchChild::Revealed(idx) => Some(*idx),
                    ArenaSparseNodeBranchChild::Blinded(_) => None,
                };
                (nibble, child_idx)
            };

            let Some(child_idx) = remaining_child_idx else {
                debug_assert!(false, "single remaining child is blinded — should have been caught by check_subtrie_collapse_needs_proof or post-parallel pre-scan");
                return;
            };

            if matches!(self.upper_arena[child_idx], ArenaSparseNode::TakenSubtrie) {
                // Subtrie hasn't been restored yet; collapse is deferred to the
                // post-restore phase.
                return;
            }

            // Check if the remaining child is an empty subtrie that should also be removed.
            let is_empty_subtrie = matches!(
                &self.upper_arena[child_idx],
                ArenaSparseNode::Subtrie(s) if matches!(s.arena[s.root], ArenaSparseNode::EmptyRoot)
            );

            if is_empty_subtrie {
                self.recycle_subtrie_from_idx(child_idx);
                let branch = self.upper_arena[branch_idx].branch_mut();
                branch.children.remove(0);
                branch.unset_child_bit(remaining_nibble);
                branch.state = branch.state.to_dirty();
                continue; // now count == 0, will be handled next iteration
            }

            // Normal collapse: the remaining child is a Leaf, Branch, or non-empty Subtrie.
            Self::collapse_branch(
                &mut self.upper_arena,
                cursor,
                &mut self.root,
                &mut self.buffers.updates,
            );

            // After collapse, the remaining child (now at cursor head) may be a
            // Subtrie whose path was shortened by the collapsed branch's prefix. Since
            // should_be_subtrie requires path_len == UPPER_TRIE_MAX_DEPTH and the collapse
            // made the path shorter, the subtrie is no longer eligible — unwrap it.
            let child_idx = cursor.head().expect("cursor is non-empty").index;
            if let ArenaSparseNode::Subtrie(_) = &self.upper_arena[child_idx] {
                let ArenaSparseNode::Subtrie(mut subtrie) =
                    mem::replace(&mut self.upper_arena[child_idx], ArenaSparseNode::TakenSubtrie)
                else {
                    unreachable!()
                };
                Self::migrate_nodes(
                    &mut self.upper_arena,
                    &mut subtrie.arena,
                    subtrie.root,
                    Some(child_idx),
                );
                Self::merge_subtrie_updates(
                    &mut self.buffers.updates,
                    &mut subtrie.buffers.updates,
                );
                subtrie.clear();
                self.cleared_subtries.push(subtrie);

                // The migrated subtrie root may be a branch whose children now live in
                // the upper arena at or beyond the subtrie boundary depth. Re-wrap any
                // such children as subtries.
                self.maybe_wrap_branch_children(cursor);
            }
            return;
        }
    }

    /// Merges updates from a subtrie's buffer into the parent's buffer.
    /// Both `dst` and `src` must be `Some` when updates are being tracked.
    ///
    /// Source removals cancel destination insertions (and vice versa) so that
    /// updates accumulated across multiple `root()` calls within a single block
    /// stay consistent.
    fn merge_subtrie_updates(
        dst: &mut Option<SparseTrieUpdates>,
        src: &mut Option<SparseTrieUpdates>,
    ) {
        if let Some(dst_updates) = dst.as_mut() {
            let src_updates = src.as_mut().expect("updates are enabled");
            debug_assert!(!src_updates.wiped, "subtrie updates should never have wiped=true");

            // Source insertions cancel destination removals.
            for path in src_updates.updated_nodes.keys() {
                dst_updates.removed_nodes.remove(path);
            }
            dst_updates.updated_nodes.extend(src_updates.updated_nodes.drain());

            // Source removals cancel destination insertions.
            for path in &src_updates.removed_nodes {
                dst_updates.updated_nodes.remove(path);
            }
            dst_updates.removed_nodes.extend(src_updates.removed_nodes.drain());
        }
    }

    /// Right-pads a nibble path with zeros and packs it into a [`B256`].
    fn nibbles_to_padded_b256(path: &Nibbles) -> B256 {
        let mut bytes = [0u8; 32];
        path.pack_to(&mut bytes);
        B256::from(bytes)
    }

    /// Returns the [`BranchNodeMasks`] for a branch based on the status of its children.
    fn get_branch_masks(arena: &NodeArena, branch: &ArenaSparseNodeBranch) -> BranchNodeMasks {
        let mut masks = BranchNodeMasks::default();

        for (nibble, child) in branch.child_iter() {
            let (hash_bit, tree_bit) = match child {
                ArenaSparseNodeBranchChild::Blinded(_) => (
                    branch.branch_masks.hash_mask.is_bit_set(nibble),
                    branch.branch_masks.tree_mask.is_bit_set(nibble),
                ),
                ArenaSparseNodeBranchChild::Revealed(child_idx) => {
                    let child = &arena[*child_idx];
                    (child.hash_mask_bit(), child.tree_mask_bit())
                }
            };

            masks.set_child_bits(nibble, hash_bit, tree_bit);
        }

        masks
    }

    /// Computes and caches `RlpNode` for all dirty nodes reachable from `root` in `arena`.
    ///
    /// Uses the cursor's stack to walk dirty branches depth-first. For each branch,
    /// children are iterated left-to-right:
    /// - Blinded, cached, leaf, and `EmptyRoot` children have their `RlpNode` pushed directly onto
    ///   `rlp_node_buf`.
    /// - Dirty branch children are pushed onto `stack` and processed recursively first.
    ///
    /// When a dirty branch child finishes and is popped, the parent resumes iteration after
    /// the child's nibble. Once all children of a branch are processed, the branch is encoded
    /// via `BranchNodeRef` using the last N entries on `rlp_node_buf`, then replaced with a
    /// single result `RlpNode`.
    #[instrument(level = "trace", target = TRACE_TARGET, skip_all, fields(base_path = ?base_path), ret)]
    fn update_cached_rlp(
        arena: &mut NodeArena,
        root: Index,
        cursor: &mut ArenaCursor,
        rlp_buf: &mut Vec<u8>,
        rlp_node_buf: &mut Vec<RlpNode>,
        base_path: Nibbles,
        updates: &mut Option<SparseTrieUpdates>,
    ) -> RlpNode {
        rlp_node_buf.clear();

        // Step 1: Handle trivial roots that don't need the stack-based walk.
        // EmptyRoot has no state to update. Leaves are encoded in place. Already-cached
        // branches need no work. Only dirty branches enter the main loop below.
        match &arena[root] {
            ArenaSparseNode::EmptyRoot => return RlpNode::word_rlp(&EMPTY_ROOT_HASH),
            ArenaSparseNode::Leaf { .. } => {
                Self::encode_leaf(arena, root, rlp_buf, rlp_node_buf);
                return rlp_node_buf.pop().expect("encode_leaf must push an RlpNode");
            }
            ArenaSparseNode::Branch(b) => {
                if let ArenaSparseNodeState::Cached { rlp_node, .. } = &b.state {
                    return rlp_node.clone();
                }
            }
            ArenaSparseNode::Subtrie(_) | ArenaSparseNode::TakenSubtrie => {
                unreachable!("Subtrie/TakenSubtrie should not appear inside a subtrie's own arena");
            }
        }

        cursor.reset(arena, root, base_path);

        // Step 2: Walk dirty branches depth-first using `cursor.next`. Only dirty branches
        // are descended into; all other children (leaves, cached branches, blinded, subtries)
        // are encoded when their parent branch is popped.
        loop {
            let result = cursor.next(&mut *arena, |_, node| {
                matches!(
                    node,
                    ArenaSparseNode::Branch(b) if matches!(b.state, ArenaSparseNodeState::Dirty)
                )
            });

            match result {
                NextResult::Done => break,
                NextResult::NonBranch => {
                    unreachable!("should_descend only returns true for dirty branches")
                }
                NextResult::Branch => {}
            };

            let head = cursor.head().expect("cursor is non-empty");
            let head_idx = head.index;
            let head_path = head.path;

            // The branch at `head_idx` is exhausted. All its dirty child branches
            // have already been encoded and cached. Collect all children's RLP nodes
            // and encode the branch.
            trace!(
                target: TRACE_TARGET,
                branch_path = ?head_path,
                branch_short_key = ?arena[head_idx].short_key().expect("head is a branch"),
                state_mask = ?arena[head_idx].branch_ref().state_mask,
                "Calculating branch RlpNode",
            );

            rlp_node_buf.clear();
            let state_mask = arena[head_idx].branch_ref().state_mask;
            for (child_idx, _nibble) in BranchChildIter::new(state_mask) {
                match &arena[head_idx].branch_ref().children[child_idx] {
                    ArenaSparseNodeBranchChild::Blinded(rlp_node) => {
                        rlp_node_buf.push(rlp_node.clone());
                    }
                    ArenaSparseNodeBranchChild::Revealed(child_idx) => {
                        let child_idx = *child_idx;
                        match &arena[child_idx] {
                            ArenaSparseNode::Leaf { .. } => {
                                Self::encode_leaf(arena, child_idx, rlp_buf, rlp_node_buf);
                            }
                            ArenaSparseNode::Branch(child_b) => {
                                let ArenaSparseNodeState::Cached { rlp_node, .. } = &child_b.state
                                else {
                                    panic!("child branch must be cached after DFS");
                                };
                                rlp_node_buf.push(rlp_node.clone());
                            }
                            ArenaSparseNode::Subtrie(subtrie) => {
                                let subtrie_root = &subtrie.arena[subtrie.root];
                                match subtrie_root {
                                    ArenaSparseNode::Branch(ArenaSparseNodeBranch {
                                        state: ArenaSparseNodeState::Cached { rlp_node, .. },
                                        ..
                                    })
                                    | ArenaSparseNode::Leaf {
                                        state: ArenaSparseNodeState::Cached { rlp_node, .. },
                                        ..
                                    } => {
                                        rlp_node_buf.push(rlp_node.clone());
                                    }
                                    _ => panic!("subtrie root must be a cached Branch or Leaf"),
                                }
                            }
                            ArenaSparseNode::TakenSubtrie | ArenaSparseNode::EmptyRoot => {
                                unreachable!("Unexpected child {:?}", arena[child_idx]);
                            }
                        }
                    }
                }
            }

            // Encode the branch, optionally wrapping in an extension if it has a short_key.
            let b = arena[head_idx].branch_ref();
            let short_key = b.short_key;
            let state_mask = b.state_mask;
            let prev_branch_masks = b.branch_masks;
            let new_branch_masks = Self::get_branch_masks(arena, b);
            let was_dirty = matches!(b.state, ArenaSparseNodeState::Dirty);

            rlp_buf.clear();
            let rlp_node = BranchNodeRef::new(rlp_node_buf, state_mask).rlp(rlp_buf);

            let rlp_node = if short_key.is_empty() {
                rlp_node
            } else {
                rlp_buf.clear();
                ExtensionNodeRef::new(&short_key, &rlp_node).rlp(rlp_buf)
            };

            trace!(
                target: TRACE_TARGET,
                path = ?head_path,
                short_key = ?arena[head_idx].short_key(),
                children = ?state_mask.iter().zip(rlp_node_buf.iter()).collect::<Vec<_>>(),
                rlp_node = ?rlp_node,
                "Calculated branch RlpNode",
            );

            let branch = arena[head_idx].branch_mut();
            branch.state = ArenaSparseNodeState::Cached { rlp_node: rlp_node.clone() };
            branch.branch_masks = new_branch_masks;

            // Record trie updates for dirty branches only.
            // Skip the root node (empty logical path) as PST does.
            if let Some(trie_updates) = updates.as_mut().filter(|_| was_dirty) {
                let mut logical_path = head_path;
                logical_path.extend(&short_key);

                if !logical_path.is_empty() {
                    if !prev_branch_masks.is_empty() && new_branch_masks.is_empty() {
                        trie_updates.updated_nodes.remove(&logical_path);
                        trie_updates.removed_nodes.insert(logical_path);
                    } else if !new_branch_masks.is_empty() {
                        let compact = arena[head_idx].branch_ref().branch_node_compact(arena);
                        trie_updates.updated_nodes.insert(logical_path, compact);
                        trie_updates.removed_nodes.remove(&logical_path);
                    }
                }
            }
        }

        let ArenaSparseNodeState::Cached { rlp_node, .. } = &arena[root].branch_ref().state else {
            panic!("root must be cached after update_cached_rlp");
        };
        rlp_node.clone()
    }

    /// Immutable traversal to find a leaf value at `full_path` starting from `root` in `arena`.
    /// `path_offset` is the number of nibbles already consumed from `full_path`.
    fn get_leaf_value_in_arena<'a>(
        arena: &'a NodeArena,
        mut current: Index,
        full_path: &Nibbles,
        mut path_offset: usize,
    ) -> Option<&'a Vec<u8>> {
        loop {
            match &arena[current] {
                ArenaSparseNode::EmptyRoot | ArenaSparseNode::TakenSubtrie => return None,
                ArenaSparseNode::Leaf { key, value, .. } => {
                    let remaining = full_path.slice(path_offset..);
                    return (remaining == *key).then_some(value);
                }
                ArenaSparseNode::Branch(b) => {
                    let short_key = &b.short_key;
                    let logical_end = path_offset + short_key.len();
                    if full_path.len() <= logical_end
                        || full_path.slice(path_offset..logical_end) != *short_key
                    {
                        return None;
                    }

                    let child_nibble = full_path.get_unchecked(logical_end);
                    let child_idx = BranchChildIdx::new(b.state_mask, child_nibble)?;
                    match &b.children[child_idx] {
                        ArenaSparseNodeBranchChild::Blinded(_) => return None,
                        ArenaSparseNodeBranchChild::Revealed(child_idx) => {
                            current = *child_idx;
                            path_offset = logical_end + 1;
                        }
                    }
                }
                ArenaSparseNode::Subtrie(subtrie) => {
                    return Self::get_leaf_value_in_arena(
                        &subtrie.arena,
                        subtrie.root,
                        full_path,
                        path_offset,
                    );
                }
            }
        }
    }

    /// Immutable traversal from the given root in `arena`, following `full_path` to find a leaf.
    /// Returns whether the leaf exists or not, or an error if a blinded node is encountered or
    /// the value doesn't match.
    fn find_leaf_in_arena(
        arena: &NodeArena,
        mut current: Index,
        full_path: &Nibbles,
        mut path_offset: usize,
        expected_value: Option<&Vec<u8>>,
    ) -> Result<LeafLookup, LeafLookupError> {
        loop {
            match &arena[current] {
                ArenaSparseNode::EmptyRoot | ArenaSparseNode::TakenSubtrie => {
                    return Ok(LeafLookup::NonExistent);
                }
                ArenaSparseNode::Leaf { key, value, .. } => {
                    let remaining = full_path.slice(path_offset..);
                    if remaining != *key {
                        return Ok(LeafLookup::NonExistent);
                    }
                    if let Some(expected) = expected_value
                        && *expected != *value
                    {
                        return Err(LeafLookupError::ValueMismatch {
                            path: *full_path,
                            expected: Some(expected.clone()),
                            actual: value.clone(),
                        });
                    }
                    return Ok(LeafLookup::Exists);
                }
                ArenaSparseNode::Branch(b) => {
                    let short_key = &b.short_key;
                    let logical_end = path_offset + short_key.len();

                    if full_path.len() <= logical_end {
                        return Ok(LeafLookup::NonExistent);
                    }

                    if full_path.slice(path_offset..logical_end) != *short_key {
                        return Ok(LeafLookup::NonExistent);
                    }

                    let child_nibble = full_path.get_unchecked(logical_end);
                    let Some(child_idx) = BranchChildIdx::new(b.state_mask, child_nibble) else {
                        return Ok(LeafLookup::NonExistent);
                    };

                    match &b.children[child_idx] {
                        ArenaSparseNodeBranchChild::Blinded(rlp_node) => {
                            let hash = rlp_node
                                .as_hash()
                                .unwrap_or_else(|| keccak256(rlp_node.as_slice()));
                            let mut blinded_path = full_path.slice(..logical_end);
                            blinded_path.push_unchecked(child_nibble);
                            return Err(LeafLookupError::BlindedNode { path: blinded_path, hash });
                        }
                        ArenaSparseNodeBranchChild::Revealed(child_idx) => {
                            current = *child_idx;
                            path_offset = logical_end + 1;
                        }
                    }
                }
                ArenaSparseNode::Subtrie(subtrie) => {
                    return Self::find_leaf_in_arena(
                        &subtrie.arena,
                        subtrie.root,
                        full_path,
                        path_offset,
                        expected_value,
                    );
                }
            }
        }
    }

    /// Encodes a leaf node's RLP and pushes it onto `rlp_node_buf`. If the leaf is already
    /// cached, its existing `RlpNode` is reused.
    fn encode_leaf(
        arena: &mut NodeArena,
        idx: Index,
        rlp_buf: &mut Vec<u8>,
        rlp_node_buf: &mut Vec<RlpNode>,
    ) {
        let (key, value, state) = match &arena[idx] {
            ArenaSparseNode::Leaf { key, value, state } => (key, value, state),
            _ => unreachable!("encode_leaf called on non-Leaf node"),
        };

        if let ArenaSparseNodeState::Cached { rlp_node, .. } = state {
            rlp_node_buf.push(rlp_node.clone());
            return;
        }

        rlp_buf.clear();
        let rlp_node = LeafNodeRef { key, value }.rlp(rlp_buf);

        *arena[idx].state_mut() = ArenaSparseNodeState::Cached { rlp_node: rlp_node.clone() };
        rlp_node_buf.push(rlp_node);
    }

    /// Creates a new leaf and a new branch that splits an existing child from the new leaf at
    /// a divergence point. Returns the index of the new branch.
    ///
    /// `new_leaf_path` is the full remaining path for the new leaf (relative to the split
    /// point's parent).
    ///
    /// The old child's key (leaf) or `short_key` (branch) is truncated to the suffix after the
    /// divergence nibble and its state is set to dirty.
    ///
    /// The top of `stack` must be the leaf or branch being split. The top of stack will be the
    /// newly created branch once this returns.
    /// Returns `true` if the existing node was not already dirty (i.e., the split newly dirtied
    /// it).
    fn split_and_insert_leaf(
        arena: &mut NodeArena,
        cursor: &mut ArenaCursor,
        root: &mut Index,
        new_leaf_path: Nibbles,
        value: &[u8],
    ) -> bool {
        let old_child_entry = cursor.head().expect("cursor must have head");
        let old_child_idx = old_child_entry.index;
        let old_child_short_key = arena[old_child_idx].short_key().expect("top of stack is a leaf");
        let diverge_len = new_leaf_path.common_prefix_length(old_child_short_key);

        trace!(
            target: TRACE_TARGET,
            path = ?old_child_entry.path,
            ?new_leaf_path,
            ?old_child_short_key,
            diverge_len,
            "Splitting node and inserting new leaf",
        );

        let old_child_nibble = old_child_short_key.get_unchecked(diverge_len);
        let old_child_suffix = old_child_short_key.slice(diverge_len + 1..);

        // Truncate the old child's key/short_key and mark it dirty.
        // Track whether the existing node was not already dirty (a leaf that becomes newly dirty).
        let newly_dirtied_existing = match &mut arena[old_child_idx] {
            ArenaSparseNode::Leaf { key, state, .. } => {
                *key = old_child_suffix;
                let was_clean = !matches!(state, ArenaSparseNodeState::Dirty);
                *state = ArenaSparseNodeState::Dirty;
                was_clean
            }
            ArenaSparseNode::Branch(b) => {
                b.short_key = old_child_suffix;
                b.state = b.state.to_dirty();
                // Branches don't contribute to num_dirty_leaves.
                false
            }
            _ => unreachable!("split_and_insert_leaf called on non-Leaf/Branch node"),
        };

        let short_key = new_leaf_path.slice(..diverge_len);
        let new_leaf_nibble = new_leaf_path.get_unchecked(diverge_len);
        debug_assert_ne!(old_child_nibble, new_leaf_nibble);

        let new_leaf_idx = arena.insert(ArenaSparseNode::Leaf {
            state: ArenaSparseNodeState::Dirty,
            key: new_leaf_path.slice(diverge_len + 1..),
            value: value.to_vec(),
        });

        let (first_nibble, first_child, second_nibble, second_child) =
            if old_child_nibble < new_leaf_nibble {
                (old_child_nibble, old_child_idx, new_leaf_nibble, new_leaf_idx)
            } else {
                (new_leaf_nibble, new_leaf_idx, old_child_nibble, old_child_idx)
            };

        let state_mask = TrieMask::from(1u16 << first_nibble | 1u16 << second_nibble);
        let mut children = SmallVec::with_capacity(2);
        children.push(ArenaSparseNodeBranchChild::Revealed(first_child));
        children.push(ArenaSparseNodeBranchChild::Revealed(second_child));

        let new_branch_idx = arena.insert(ArenaSparseNode::Branch(ArenaSparseNodeBranch {
            state: ArenaSparseNodeState::Dirty,
            children,
            state_mask,
            short_key,
            branch_masks: BranchNodeMasks::default(),
        }));

        cursor.replace_head_index(arena, root, new_branch_idx);
        newly_dirtied_existing
    }

    /// Performs a leaf upsert using a pre-computed [`SeekResult`] from
    /// [`ArenaCursor::seek`].
    ///
    /// Handles three cases based on `find_result`:
    /// 1. `RevealedLeaf` — the cursor head is a leaf; update in place or split into a branch.
    /// 2. Diverged — the path diverges within the branch's `short_key`, split it.
    /// 3. `NoChild` — the target nibble has no child, insert a new leaf.
    ///
    /// The caller must handle [`SeekResult::Blinded`] and
    /// [`SeekResult::RevealedSubtrie`] before calling this function.
    /// The cursor must be non-empty when called.
    ///
    /// Returns an [`UpsertLeafResult`] and [`SubtrieCounterDeltas`] so the caller can maintain
    /// aggregate counters and decide whether to wrap the result as a subtrie.
    #[instrument(level = "trace", target = TRACE_TARGET, skip_all, fields(full_path = ?full_path))]
    fn upsert_leaf(
        arena: &mut NodeArena,
        cursor: &mut ArenaCursor,
        root: &mut Index,
        full_path: &Nibbles,
        value: &[u8],
        find_result: SeekResult,
    ) -> (UpsertLeafResult, SubtrieCounterDeltas) {
        trace!(target: TRACE_TARGET, ?find_result, "Upserting leaf");
        let head = cursor.head().expect("cursor is non-empty");

        match find_result {
            SeekResult::Blinded => {
                unreachable!("Blinded case must be handled by caller")
            }
            SeekResult::EmptyRoot => {
                let head_idx = head.index;
                let head_path = head.path;
                arena[head_idx] = ArenaSparseNode::Leaf {
                    state: ArenaSparseNodeState::Dirty,
                    key: full_path.slice(head_path.len()..),
                    value: value.to_vec(),
                };
                (
                    UpsertLeafResult::NewLeaf,
                    SubtrieCounterDeltas { num_leaves_delta: 1, num_dirty_leaves_delta: 1 },
                )
            }
            SeekResult::RevealedLeaf => {
                // RevealedLeaf guarantees the leaf's full path matches the target exactly.
                let head_idx = head.index;
                let was_clean =
                    if let ArenaSparseNode::Leaf { value: v, state, .. } = &mut arena[head_idx] {
                        v.clear();
                        v.extend_from_slice(value);
                        let was_clean = !matches!(state, ArenaSparseNodeState::Dirty);
                        *state = ArenaSparseNodeState::Dirty;
                        was_clean
                    } else {
                        unreachable!("RevealedLeaf but cursor head is not a leaf")
                    };
                (
                    UpsertLeafResult::Updated,
                    SubtrieCounterDeltas {
                        num_leaves_delta: 0,
                        num_dirty_leaves_delta: was_clean as i64,
                    },
                )
            }
            SeekResult::Diverged => {
                let head_path = head.path;
                let full_path_from_head = full_path.slice(head_path.len()..);

                let split_dirtied_existing =
                    Self::split_and_insert_leaf(arena, cursor, root, full_path_from_head, value);

                let result = if cursor.depth() >= 1 {
                    UpsertLeafResult::NewChild
                } else {
                    UpsertLeafResult::NewLeaf
                };
                (
                    result,
                    SubtrieCounterDeltas {
                        num_leaves_delta: 1,
                        num_dirty_leaves_delta: 1 + split_dirtied_existing as i64,
                    },
                )
            }
            SeekResult::NoChild { child_nibble } => {
                let head_idx = head.index;

                let head_branch_logical_path = cursor.head_logical_branch_path(arena);
                let leaf_key = full_path.slice(head_branch_logical_path.len() + 1..);
                let new_leaf = arena.insert(ArenaSparseNode::Leaf {
                    state: ArenaSparseNodeState::Dirty,
                    key: leaf_key,
                    value: value.to_vec(),
                });

                let branch = arena[head_idx].branch_mut();
                branch.set_child(child_nibble, ArenaSparseNodeBranchChild::Revealed(new_leaf));

                // Re-seek to position the cursor on the newly inserted leaf.
                cursor.seek(arena, full_path);

                (
                    UpsertLeafResult::NewChild,
                    SubtrieCounterDeltas { num_leaves_delta: 1, num_dirty_leaves_delta: 1 },
                )
            }
            SeekResult::RevealedSubtrie => {
                unreachable!("RevealedSubtrie must be handled by caller")
            }
        }
    }

    /// Removes a leaf node from the trie using a pre-computed [`SeekResult`] from
    /// [`ArenaCursor::seek`].
    ///
    /// Only the `RevealedLeaf` case performs a removal — the leaf must exist and its full path
    /// must match `full_path`. All other cases (`Diverged`, `NoChild`) are no-ops since the leaf
    /// doesn't exist at that path.
    ///
    /// When removing a leaf from a branch, if the branch is left with only one remaining child,
    /// the branch is collapsed: the remaining child absorbs the branch's `short_key` + the child's
    /// nibble as a prefix to its own key/`short_key`, and replaces the branch in the parent.
    /// If the remaining child is blinded, the collapse cannot proceed and a
    /// [`RemoveLeafResult::NeedsProof`] is returned so the caller can request a proof.
    ///
    /// The caller must handle [`SeekResult::Blinded`] and
    /// [`SeekResult::RevealedSubtrie`] before calling this function.
    fn remove_leaf(
        arena: &mut NodeArena,
        cursor: &mut ArenaCursor,
        root: &mut Index,
        key: B256,
        full_path: &Nibbles,
        find_result: SeekResult,
        updates: &mut Option<SparseTrieUpdates>,
    ) -> (RemoveLeafResult, SubtrieCounterDeltas) {
        match find_result {
            SeekResult::Blinded | SeekResult::RevealedSubtrie => {
                unreachable!("Blinded/RevealedSubtrie must be handled by caller")
            }
            SeekResult::EmptyRoot | SeekResult::Diverged | SeekResult::NoChild { .. } => {
                (RemoveLeafResult::NotFound, SubtrieCounterDeltas::default())
            }
            SeekResult::RevealedLeaf => {
                // RevealedLeaf guarantees the leaf's full path matches the target exactly.
                let head = cursor.head().expect("cursor is non-empty");
                let head_idx = head.index;
                let head_path = head.path;

                trace!(
                    target: TRACE_TARGET,
                    path = ?head_path,
                    ?full_path,
                    "Removing leaf",
                );

                // Before mutating, check if removing this leaf would leave the parent
                // branch with a single blinded sibling (requiring a proof to collapse).
                if let Some(parent_entry) = cursor.parent() {
                    let parent_idx = parent_entry.index;
                    let child_nibble = head_path.last().expect("non-root leaf");
                    let parent_branch = arena[parent_idx].branch_ref();

                    if parent_branch.state_mask.count_bits() == 2
                        && parent_branch.sibling_child(child_nibble).is_blinded()
                    {
                        let sibling_nibble = parent_branch
                            .state_mask
                            .iter()
                            .find(|&n| n != child_nibble)
                            .expect("branch has two children");
                        let mut sibling_path = cursor.parent_logical_branch_path(arena);
                        sibling_path.push_unchecked(sibling_nibble);
                        trace!(target: TRACE_TARGET, ?full_path, ?sibling_path, "Removal would collapse branch onto blinded sibling, requesting proof");
                        return (
                            RemoveLeafResult::NeedsProof {
                                key,
                                proof_key: Self::nibbles_to_padded_b256(&sibling_path),
                                min_len: (sibling_path.len() as u8).min(64),
                            },
                            SubtrieCounterDeltas::default(),
                        );
                    }
                }

                // Check if the removed leaf was dirty before removing it.
                let removed_was_dirty =
                    matches!(arena[head_idx].state_ref(), Some(ArenaSparseNodeState::Dirty));

                if cursor.depth() == 0 {
                    // The leaf is the root — replace with EmptyRoot and reset the cursor
                    // so subsequent iterations can call seek normally.
                    arena.remove(head_idx);
                    *root = arena.insert(ArenaSparseNode::EmptyRoot);
                    cursor.reset(arena, *root, head_path);
                    return (
                        RemoveLeafResult::Removed,
                        SubtrieCounterDeltas {
                            num_leaves_delta: -1,
                            num_dirty_leaves_delta: -(removed_was_dirty as i64),
                        },
                    );
                }

                // Pop the leaf entry, propagating dirty state to the parent.
                cursor.pop(arena);

                // The parent must be a branch. Remove the leaf from it.
                let parent_entry = cursor.head().expect("cursor is non-empty");
                let parent_idx = parent_entry.index;
                let child_nibble = head_path.last().expect("non-root leaf");

                // Remove the leaf from the arena and from the parent's children.
                arena.remove(head_idx);
                let parent_branch = arena[parent_idx].branch_mut();
                parent_branch.remove_child(child_nibble);

                // If the branch now has only one child, collapse it. The blinded sibling
                // case was already handled above before any mutations.
                let collapse_dirtied_leaf = if parent_branch.state_mask.count_bits() == 1 {
                    Self::collapse_branch(arena, cursor, root, updates)
                } else {
                    false
                };
                (
                    RemoveLeafResult::Removed,
                    SubtrieCounterDeltas {
                        num_leaves_delta: -1,
                        num_dirty_leaves_delta: (collapse_dirtied_leaf as i64)
                            - (removed_was_dirty as i64),
                    },
                )
            }
        }
    }

    /// Checks whether a subtrie receiving only removals would cause its parent branch to collapse
    /// onto a single blinded sibling. If so, returns the proof needed to reveal that blinded
    /// sibling so the caller can request it and skip the subtrie's updates.
    ///
    /// Returns `Some(proof)` for the blinded sibling when the edge-case applies, `None` otherwise.
    fn check_subtrie_collapse_needs_proof(
        arena: &NodeArena,
        cursor: &ArenaCursor,
        subtrie_updates: &[(B256, Nibbles, LeafUpdate)],
    ) -> Option<ArenaRequiredProof> {
        let num_removals = subtrie_updates
            .iter()
            .filter(|(_, _, u)| matches!(u, LeafUpdate::Changed(v) if v.is_empty()))
            .count() as u64;

        if num_removals == 0 || num_removals as usize != subtrie_updates.len() {
            return None;
        }

        // The subtrie is the cursor head; its parent is the cursor's parent.
        let subtrie_entry = cursor.head()?;
        let subtrie_num_leaves = match &arena[subtrie_entry.index] {
            ArenaSparseNode::Subtrie(s) => s.num_leaves,
            _ => return None,
        };
        if num_removals < subtrie_num_leaves {
            return None;
        }

        // This subtrie would be fully emptied by all-removal updates. If the parent
        // has any blinded children, removing this subtrie could eventually leave the
        // parent with a single blinded child — especially when multiple sibling
        // subtries are taken in the same batch and also emptied. Conservatively
        // request a proof for the first blinded sibling found.
        let child_nibble =
            subtrie_entry.path.last().expect("subtrie path must have at least one nibble");

        let parent_entry = cursor.parent()?;
        let parent_branch = arena[parent_entry.index].branch_ref();

        // Find any blinded sibling under the parent.
        let blinded_sibling_nibble = parent_branch
            .child_iter()
            .find(|(nibble, child)| *nibble != child_nibble && child.is_blinded())
            .map(|(nibble, _)| nibble)?;

        let mut sibling_path = cursor.parent_logical_branch_path(arena);
        sibling_path.push_unchecked(blinded_sibling_nibble);

        Some(ArenaRequiredProof {
            key: Self::nibbles_to_padded_b256(&sibling_path),
            min_len: (sibling_path.len() as u8).min(64),
        })
    }

    /// Identifies empty taken subtries whose unwrapping would cascade into a blinded sibling
    /// at any level of the upper trie.
    ///
    /// Returns the set of indices into `taken_info` that should NOT be unwrapped. For those
    /// subtries, their updates are re-inserted into `updates` and a proof is requested.
    ///
    /// This handles the multi-subtrie-emptying case that `check_subtrie_collapse_needs_proof`
    /// cannot catch: when subtries are evaluated independently, each parent appears to have
    /// enough children, but after ALL empty subtries are removed, cascading collapses may
    /// reach a branch whose sole remaining child is blinded.
    #[allow(clippy::too_many_arguments)]
    fn find_unsafe_empty_subtries(
        &mut self,
        taken_info: &[TakenInfo],
        cursor: &mut ArenaCursor,
        sorted: &[(B256, Nibbles, LeafUpdate)],
        updates: &mut B256Map<LeafUpdate>,
        proof_required_fn: &mut impl FnMut(B256, u8),
        #[cfg(feature = "trie-debug")] recorded_proof_targets: &mut Vec<(B256, u8)>,
    ) -> HashSet<usize> {
        let mut skip_indices: HashSet<usize> = HashSet::default();

        // Group empty taken subtrie indices by the direct parent branch's arena index.
        let mut parent_groups: HashMap<Index, SmallVec<[usize; 4]>> = HashMap::default();
        // Map parent_idx → parent path (from cursor) for path reconstruction.
        let mut parent_paths: HashMap<Index, Nibbles> = HashMap::default();

        for (i, info) in taken_info.iter().enumerate() {
            if !info.is_empty {
                continue;
            }

            let find_result = cursor.seek(&mut self.upper_arena, &info.path);
            if matches!(find_result, SeekResult::RevealedSubtrie) {
                if let Some(parent) = cursor.parent() {
                    parent_groups.entry(parent.index).or_default().push(i);
                    parent_paths.entry(parent.index).or_insert(parent.path);
                }
            }
        }

        // Drain and reset cursor so subsequent operations start fresh.
        cursor.drain(&mut self.upper_arena);
        cursor.reset(&self.upper_arena, self.root, Nibbles::default());

        if parent_groups.is_empty() {
            return skip_indices;
        }

        // For each parent group, simulate the cascade and check if it would hit
        // a blinded child at any level.
        for (parent_idx, empty_indices) in &parent_groups {
            if let Some(proof) =
                self.simulate_cascade(*parent_idx, &parent_paths, taken_info, empty_indices)
            {
                proof_required_fn(proof.key, proof.min_len);
                #[cfg(feature = "trie-debug")]
                recorded_proof_targets.push((proof.key, proof.min_len));

                for &idx in empty_indices {
                    skip_indices.insert(idx);
                    for &(key, _, ref update) in &sorted[taken_info[idx].range.clone()] {
                        updates.insert(key, update.clone());
                    }
                }
            }
        }

        skip_indices
    }

    /// Simulates the cascade that would occur if all `empty_indices` subtries were
    /// unwrapped from `start_parent_idx`. Walks up the upper trie checking if any
    /// ancestor branch would be left with only blinded children.
    ///
    /// Returns `Some(proof)` for the first blinded sibling found, or `None` if safe.
    fn simulate_cascade(
        &self,
        start_parent_idx: Index,
        parent_paths: &HashMap<Index, Nibbles>,
        taken_info: &[TakenInfo],
        empty_indices: &SmallVec<[usize; 4]>,
    ) -> Option<ArenaRequiredProof> {
        let ArenaSparseNode::Branch(parent_branch) = &self.upper_arena[start_parent_idx] else {
            return None;
        };

        // Collect nibbles of the empty subtries being removed.
        let empty_nibbles: HashSet<u8> = empty_indices
            .iter()
            .filter_map(|&i| taken_info[i].path.last())
            .collect();

        // Count non-empty, non-blinded children that would remain.
        let mut remaining_revealed = 0usize;
        let mut remaining_blinded = 0usize;
        let mut first_blinded_nibble = None;

        for (nibble, child) in parent_branch.child_iter() {
            if empty_nibbles.contains(&nibble) {
                continue; // this child would be removed
            }
            if child.is_blinded() {
                remaining_blinded += 1;
                if first_blinded_nibble.is_none() {
                    first_blinded_nibble = Some(nibble);
                }
            } else {
                // Check if this revealed child is also an empty subtrie being unwrapped
                // (from a different parent group — shouldn't happen, but be safe).
                remaining_revealed += 1;
            }
        }

        let total_remaining = remaining_revealed + remaining_blinded;

        if total_remaining >= 2 {
            // Enough children remain — no collapse cascade.
            return None;
        }

        if total_remaining == 1 && remaining_blinded == 1 {
            // Only one blinded child remains — would cascade into it.
            let blinded_nibble = first_blinded_nibble.expect("counted 1 blinded");
            let short_key = parent_branch.short_key;
            let parent_path = parent_paths.get(&start_parent_idx).copied()
                .unwrap_or_default();

            let mut sibling_path = parent_path;
            sibling_path.extend(&short_key);
            sibling_path.push_unchecked(blinded_nibble);

            return Some(ArenaRequiredProof {
                key: Self::nibbles_to_padded_b256(&sibling_path),
                min_len: (sibling_path.len() as u8).min(64),
            });
        }

        if total_remaining == 0 {
            // Parent would become empty. If it's the root, that's fine (EmptyRoot).
            // If not root, it would be removed from grandparent. We need to check
            // the grandparent cascade. Walk up via the arena structure.
            if start_parent_idx == self.root {
                return None; // Root becoming empty is fine.
            }

            // Find grandparent by scanning the upper arena for a branch whose child
            // points to start_parent_idx.
            let grandparent = self.find_parent_of(start_parent_idx);
            if let Some((gp_idx, child_nibble)) = grandparent {
                return self.simulate_ancestor_cascade(gp_idx, child_nibble, parent_paths);
            }
        }

        if total_remaining == 1 && remaining_revealed == 1 {
            // One revealed child remains — it would be collapsed into the parent.
            // The parent gets removed from grandparent. Check grandparent cascade.
            if start_parent_idx == self.root {
                return None; // Collapsing at root level is fine.
            }

            let grandparent = self.find_parent_of(start_parent_idx);
            if let Some((gp_idx, _child_nibble)) = grandparent {
                // After collapse, the grandparent loses the child slot for start_parent_idx
                // and gains nothing new (the collapsed child replaces the branch in-place).
                // Actually, collapse_branch replaces the branch's slot in the grandparent
                // with the remaining child. So grandparent's child count doesn't change.
                // No cascade needed.
                let _ = gp_idx;
            }
            return None;
        }

        None
    }

    /// Simulates cascade upward from a branch that just lost a child at `removed_nibble`.
    fn simulate_ancestor_cascade(
        &self,
        branch_idx: Index,
        removed_nibble: u8,
        parent_paths: &HashMap<Index, Nibbles>,
    ) -> Option<ArenaRequiredProof> {
        let ArenaSparseNode::Branch(branch) = &self.upper_arena[branch_idx] else {
            return None;
        };

        let mut remaining_revealed = 0usize;
        let mut remaining_blinded = 0usize;
        let mut first_blinded_nibble = None;

        for (nibble, child) in branch.child_iter() {
            if nibble == removed_nibble {
                continue;
            }
            if child.is_blinded() {
                remaining_blinded += 1;
                if first_blinded_nibble.is_none() {
                    first_blinded_nibble = Some(nibble);
                }
            } else {
                remaining_revealed += 1;
            }
        }

        let total_remaining = remaining_revealed + remaining_blinded;

        if total_remaining >= 2 {
            return None;
        }

        if total_remaining == 1 && remaining_blinded == 1 {
            let blinded_nibble = first_blinded_nibble.expect("counted 1 blinded");
            let short_key = branch.short_key;
            let branch_path = parent_paths.get(&branch_idx).copied()
                .unwrap_or_default();

            let mut sibling_path = branch_path;
            sibling_path.extend(&short_key);
            sibling_path.push_unchecked(blinded_nibble);

            return Some(ArenaRequiredProof {
                key: Self::nibbles_to_padded_b256(&sibling_path),
                min_len: (sibling_path.len() as u8).min(64),
            });
        }

        if total_remaining == 0 && branch_idx != self.root {
            if let Some((gp_idx, child_nibble)) = self.find_parent_of(branch_idx) {
                return self.simulate_ancestor_cascade(gp_idx, child_nibble, parent_paths);
            }
        }

        None
    }

    /// Finds the parent branch of a node at `child_idx` in the upper arena.
    /// Returns `(parent_idx, child_nibble)`.
    fn find_parent_of(&self, child_idx: Index) -> Option<(Index, u8)> {
        for (idx, node) in &self.upper_arena {
            if let ArenaSparseNode::Branch(b) = node {
                for (nibble, child) in b.child_iter() {
                    if let ArenaSparseNodeBranchChild::Revealed(ci) = child {
                        if *ci == child_idx {
                            return Some((idx, nibble));
                        }
                    }
                }
            }
        }
        None
    }

    /// Collapses a branch node that has exactly one remaining revealed child. The branch's
    /// `short_key`, the remaining child's nibble, and the child's own key/`short_key` are
    /// concatenated to form the child's new key/`short_key`. The child then replaces the branch
    /// in the grandparent (or becomes the new root).
    ///
    /// The caller must verify that the remaining child is not blinded before calling this function.
    ///
    /// The branch being collapsed must be the current cursor head. The cursor head will be
    /// replaced with the remaining child which has taken its place.
    /// Returns `true` if the collapse dirtied a surviving leaf that was not already dirty.
    fn collapse_branch(
        arena: &mut NodeArena,
        cursor: &mut ArenaCursor,
        root: &mut Index,
        updates: &mut Option<SparseTrieUpdates>,
    ) -> bool {
        let branch_entry = cursor.head().expect("cursor is non-empty");
        let branch_idx = branch_entry.index;
        let branch = arena[branch_idx].branch_ref();
        let remaining_nibble =
            branch.state_mask.iter().next().expect("branch has at least one child");
        let branch_short_key = branch.short_key;

        debug_assert_eq!(
            branch.state_mask.count_bits(),
            1,
            "collapse_branch requires exactly 1 child"
        );
        debug_assert!(
            !branch.children[0].is_blinded(),
            "collapse_branch called with a blinded remaining child"
        );

        trace!(
            target: TRACE_TARGET,
            path = ?branch_entry.path,
            short_key = ?branch_short_key,
            branch_masks = ?branch.branch_masks,
            ?remaining_nibble,
            "Collapsing single-child branch",
        );

        // Record the collapsed branch's logical path for trie update tracking if it
        // was previously persisted in the DB trie.
        if let Some(trie_updates) = updates.as_mut()
            && !branch.branch_masks.is_empty()
        {
            let logical_path = cursor.head_logical_branch_path(arena);
            if !logical_path.is_empty() {
                trie_updates.updated_nodes.remove(&logical_path);
                trie_updates.removed_nodes.insert(logical_path);
            }
        }

        // Build the prefix: branch's short_key + remaining child's nibble.
        let mut prefix = branch_short_key;
        prefix.push_unchecked(remaining_nibble);

        let ArenaSparseNodeBranchChild::Revealed(child_idx) = branch.children[0] else {
            unreachable!()
        };

        // Prepend the prefix to the child's key/short_key and mark dirty.
        // Track whether a leaf was newly dirtied by this collapse.
        let newly_dirtied_leaf = match &mut arena[child_idx] {
            ArenaSparseNode::Leaf { key, state, .. } => {
                let mut new_key = prefix;
                new_key.extend(key);
                *key = new_key;
                let was_clean = !matches!(state, ArenaSparseNodeState::Dirty);
                *state = ArenaSparseNodeState::Dirty;
                was_clean
            }
            ArenaSparseNode::Branch(b) => {
                let mut new_short_key = prefix;
                new_short_key.extend(&b.short_key);
                b.short_key = new_short_key;
                b.state = b.state.to_dirty();
                false
            }
            ArenaSparseNode::Subtrie(subtrie) => {
                subtrie.path = branch_entry.path;
                match &mut subtrie.arena[subtrie.root] {
                    ArenaSparseNode::Branch(b) => {
                        let mut new_short_key = prefix;
                        new_short_key.extend(&b.short_key);
                        b.short_key = new_short_key;
                        b.state = b.state.to_dirty();
                    }
                    ArenaSparseNode::Leaf { key, state, .. } => {
                        let mut new_key = prefix;
                        new_key.extend(key);
                        *key = new_key;
                        let was_clean = !matches!(state, ArenaSparseNodeState::Dirty);
                        *state = ArenaSparseNodeState::Dirty;
                        if was_clean {
                            subtrie.num_dirty_leaves += 1;
                        }
                    }
                    _ => {
                        unreachable!("subtrie root must be a Branch or Leaf during collapse_branch")
                    }
                }
                false
            }
            _ => unreachable!("remaining child must be Leaf, Branch, or Subtrie"),
        };

        // Replace the branch with the remaining child in the grandparent (or root).
        cursor.replace_head_index(arena, root, child_idx);

        // Free the collapsed branch.
        arena.remove(branch_idx);
        newly_dirtied_leaf
    }

    /// Counts the total leaves and dirty leaves in a subtree rooted at `idx`.
    fn count_leaves_and_dirty(arena: &NodeArena, idx: Index) -> (u64, u64) {
        match &arena[idx] {
            ArenaSparseNode::Leaf { state, .. } => {
                let dirty = matches!(state, ArenaSparseNodeState::Dirty) as u64;
                (1, dirty)
            }
            ArenaSparseNode::Branch(b) => {
                let mut leaves = 0u64;
                let mut dirty = 0u64;
                for c in &b.children {
                    if let ArenaSparseNodeBranchChild::Revealed(child_idx) = c {
                        let (l, d) = Self::count_leaves_and_dirty(arena, *child_idx);
                        leaves += l;
                        dirty += d;
                    }
                }
                (leaves, dirty)
            }
            _ => (0, 0),
        }
    }

    /// Asserts that every node in the upper arena satisfies the subtrie structure invariant:
    /// - Nodes at `UPPER_TRIE_MAX_DEPTH` path length must be `Subtrie` (or `TakenSubtrie`).
    /// - Nodes at other depths must NOT be `Subtrie`.
    ///
    /// Uses the cursor to DFS the upper arena, checking each visited node's path length.
    #[instrument(level = "trace", target = TRACE_TARGET, skip_all)]
    #[cfg(debug_assertions)]
    fn debug_assert_subtrie_structure(&mut self) {
        let mut cursor = mem::take(&mut self.buffers.cursor);
        cursor.reset(&self.upper_arena, self.root, Nibbles::default());

        loop {
            let result = cursor.next(&mut self.upper_arena, |_, _| true);
            match result {
                NextResult::Done => break,
                NextResult::NonBranch | NextResult::Branch => {
                    let head = cursor.head().expect("cursor is non-empty");
                    let path_len = head.path.len();
                    let node = &self.upper_arena[head.index];

                    if Self::should_be_subtrie(path_len) {
                        debug_assert!(
                            matches!(
                                node,
                                ArenaSparseNode::Subtrie(_) | ArenaSparseNode::TakenSubtrie
                            ),
                            "node at path_len={path_len} should be a Subtrie but is {node:?}",
                        );
                    } else {
                        debug_assert!(
                            !matches!(node, ArenaSparseNode::Subtrie(_)),
                            "node at path_len={path_len} should NOT be a Subtrie but is",
                        );
                    }
                }
            }
        }

        self.buffers.cursor = cursor;
    }

    /// Recursively migrates all nodes from `src` into `dst`, starting at `src_idx`.
    /// Branch children's `Revealed` indices are remapped to the new `dst` indices during
    /// the migration.
    ///
    /// If `dst_slot` is `Some(idx)`, the node at `src_idx` is placed into `dst[idx]`
    /// (overwriting); otherwise a new slot is allocated. Returns the `dst` index of the
    /// migrated node.
    fn migrate_nodes(
        dst: &mut NodeArena,
        src: &mut NodeArena,
        src_idx: Index,
        dst_slot: Option<Index>,
    ) -> Index {
        let mut node = src.remove(src_idx).expect("node exists in source arena");

        // Recursively migrate children first so their new indices are known.
        if let ArenaSparseNode::Branch(b) = &mut node {
            for child in &mut b.children {
                if let ArenaSparseNodeBranchChild::Revealed(child_idx) = child {
                    *child_idx = Self::migrate_nodes(dst, src, *child_idx, None);
                }
            }
        }

        if let Some(slot) = dst_slot {
            dst[slot] = node;
            slot
        } else {
            dst.insert(node)
        }
    }

    /// Removes a pruned node from the arena and blinds the parent's child slot with the node's
    /// cached RLP. If the node has no cached RLP (e.g. it was never hashed), the parent slot
    /// is left dangling — this is safe because the parent will also be removed during pruning.
    fn remove_pruned_node(
        arena: &mut NodeArena,
        cursor: &ArenaCursor,
        idx: Index,
        nibble: Option<u8>,
    ) -> ArenaSparseNode {
        trace!(target: TRACE_TARGET, path = ?cursor.head().unwrap().path, "pruning node");
        let node = arena.remove(idx).expect("node must exist to be pruned");
        let rlp_node = node.state_ref().and_then(|s| s.cached_rlp_node()).cloned();

        if let Some(rlp_node) = rlp_node {
            let parent_idx = cursor.parent().expect("pruned child has parent").index;
            let child_nibble = nibble.expect("non-root child");
            let parent_branch = arena[parent_idx].branch_mut();
            let child_idx = BranchChildIdx::new(parent_branch.state_mask, child_nibble)
                .expect("child nibble not found in parent state_mask");
            parent_branch.children[child_idx] = ArenaSparseNodeBranchChild::Blinded(rlp_node);
        }

        node
    }

    /// Reveals a single proof node using a pre-computed [`SeekResult`] from
    /// [`ArenaCursor::seek`].
    ///
    /// If the result is `Blinded`, the blinded child is replaced with the proof node (converted to
    /// an arena node with `Cached` state). All other cases (already revealed, no child, diverged,
    /// leaf head) are no-ops — the proof node is skipped.
    ///
    /// Returns the `Index` of the revealed node in the arena, if any was revealed.
    #[instrument(level = "trace", target = TRACE_TARGET, skip_all)]
    fn reveal_node(
        arena: &mut NodeArena,
        cursor: &ArenaCursor,
        node: &mut ProofTrieNodeV2,
        find_result: SeekResult,
    ) -> Option<Index> {
        let SeekResult::Blinded = find_result else {
            // Already revealed, no child slot, or diverged — skip this proof node.
            return None;
        };

        let head = cursor.head().expect("cursor is non-empty");
        let head_idx = head.index;
        let head_branch_logical_path = cursor.head_logical_branch_path(arena);

        debug_assert_eq!(
            node.path.len(),
            head_branch_logical_path.len() + 1,
            "proof node path {:?} is not a direct child of branch at {:?} (expected depth {})",
            node.path,
            head_branch_logical_path,
            head_branch_logical_path.len() + 1,
        );

        let child_nibble = node.path.get_unchecked(head_branch_logical_path.len());
        let head_branch = arena[head_idx].branch_ref();
        let dense_child_idx = BranchChildIdx::new(head_branch.state_mask, child_nibble)
            .expect("Blinded result but child nibble not in state_mask");

        let cached_rlp = match &head_branch.children[dense_child_idx] {
            ArenaSparseNodeBranchChild::Blinded(rlp) => rlp.clone(),
            ArenaSparseNodeBranchChild::Revealed(_) => return None,
        };

        trace!(
            target: TRACE_TARGET,
            path = ?node.path,
            rlp_node = ?cached_rlp,
            "Revealing node",
        );

        let proof_node = mem::replace(node, ProofTrieNodeV2::empty());
        let mut arena_node = ArenaSparseNode::from_proof_node(proof_node);

        let state = arena_node.state_mut();
        *state = ArenaSparseNodeState::Cached { rlp_node: cached_rlp };

        let child_idx = arena.insert(arena_node);
        arena[head_idx].branch_mut().children[dense_child_idx] =
            ArenaSparseNodeBranchChild::Revealed(child_idx);

        Some(child_idx)
    }

    #[cfg(debug_assertions)]
    fn collect_reachable_nodes(arena: &NodeArena, idx: Index, reachable: &mut HashSet<Index>) {
        if !reachable.insert(idx) {
            return;
        }
        if let ArenaSparseNode::Branch(b) = &arena[idx] {
            for child in &b.children {
                if let ArenaSparseNodeBranchChild::Revealed(child_idx) = child {
                    Self::collect_reachable_nodes(arena, *child_idx, reachable);
                }
            }
        }
    }

    #[cfg(debug_assertions)]
    fn assert_no_orphaned_nodes(arena: &NodeArena, root: Index, label: &str) {
        let mut reachable = HashSet::default();
        Self::collect_reachable_nodes(arena, root, &mut reachable);
        let all_indices: HashSet<Index> = arena.iter().map(|(idx, _)| idx).collect();
        let orphaned: Vec<_> = all_indices.difference(&reachable).collect();
        debug_assert!(
            orphaned.is_empty(),
            "{label} has {} orphaned node(s): {orphaned:?}",
            orphaned.len(),
        );
    }
}

#[cfg(debug_assertions)]
impl Drop for ArenaParallelSparseTrie {
    fn drop(&mut self) {
        Self::assert_no_orphaned_nodes(&self.upper_arena, self.root, "upper arena");

        for (_, node) in &self.upper_arena {
            if let Some(subtrie) = node.as_subtrie() {
                Self::assert_no_orphaned_nodes(
                    &subtrie.arena,
                    subtrie.root,
                    &alloc::format!("subtrie {:?}", subtrie.path),
                );
            }
        }
    }
}

impl Default for ArenaParallelSparseTrie {
    fn default() -> Self {
        let mut upper_arena = SlotMap::new();
        let root = upper_arena.insert(ArenaSparseNode::EmptyRoot);
        Self {
            upper_arena,
            root,
            updates: None,
            buffers: ArenaTrieBuffers::default(),
            cleared_subtries: Vec::new(),
            parallelism_thresholds: ArenaParallelismThresholds::default(),
            worker_pool: worker_pool::SubtrieWorkerPool::new(2),
            #[cfg(feature = "metrics")]
            metrics: Default::default(),
            #[cfg(feature = "trie-debug")]
            debug_recorder: Default::default(),
        }
    }
}

impl Clone for ArenaParallelSparseTrie {
    fn clone(&self) -> Self {
        Self {
            upper_arena: self.upper_arena.clone(),
            root: self.root,
            updates: self.updates.clone(),
            buffers: self.buffers.clone(),
            cleared_subtries: self.cleared_subtries.clone(),
            parallelism_thresholds: self.parallelism_thresholds,
            worker_pool: worker_pool::SubtrieWorkerPool::new(2),
            #[cfg(feature = "metrics")]
            metrics: self.metrics.clone(),
            #[cfg(feature = "trie-debug")]
            debug_recorder: self.debug_recorder.clone(),
        }
    }
}

impl ArenaParallelSparseTrie {
    /// Hashes a subtrie at `head_idx` and collects its update actions.
    fn update_upper_subtrie(&mut self, head_idx: Index) {
        let ArenaSparseNode::Subtrie(subtrie) = &mut self.upper_arena[head_idx] else {
            unreachable!()
        };

        if !subtrie.arena[subtrie.root].is_cached() {
            subtrie.update_cached_rlp();
        }

        Self::merge_subtrie_updates(&mut self.buffers.updates, &mut subtrie.buffers.updates);
    }
}

impl SparseTrie for ArenaParallelSparseTrie {
    #[instrument(level = "trace", target = TRACE_TARGET, skip_all)]
    fn set_root(
        &mut self,
        root: TrieNodeV2,
        masks: Option<BranchNodeMasks>,
        retain_updates: bool,
    ) -> SparseTrieResult<()> {
        #[cfg(feature = "trie-debug")]
        self.debug_recorder.record(RecordedOp::SetRoot {
            node: ProofTrieNodeRecord::from_proof_trie_node_v2(&ProofTrieNodeV2 {
                path: Nibbles::default(),
                node: root.clone(),
                masks,
            }),
        });

        debug_assert!(
            matches!(self.upper_arena[self.root], ArenaSparseNode::EmptyRoot),
            "set_root called on a trie that already has revealed nodes"
        );

        self.set_updates(retain_updates);

        match root {
            TrieNodeV2::EmptyRoot => {
                trace!(target: TRACE_TARGET, "Setting empty root");
            }
            TrieNodeV2::Leaf(leaf) => {
                trace!(target: TRACE_TARGET, key = ?leaf.key, "Setting leaf root");
                self.upper_arena[self.root] = ArenaSparseNode::Leaf {
                    state: ArenaSparseNodeState::Revealed,
                    key: leaf.key,
                    value: leaf.value,
                };
            }
            TrieNodeV2::Branch(branch) => {
                trace!(target: TRACE_TARGET, state_mask = ?branch.state_mask, num_children = branch.state_mask.count_bits(), "Setting branch root");
                let mut children = SmallVec::with_capacity(branch.state_mask.count_bits() as usize);
                for (stack_ptr, _nibble) in branch.state_mask.iter().enumerate() {
                    children
                        .push(ArenaSparseNodeBranchChild::Blinded(branch.stack[stack_ptr].clone()));
                }

                self.upper_arena[self.root] = ArenaSparseNode::Branch(ArenaSparseNodeBranch {
                    state: ArenaSparseNodeState::Revealed,
                    children,
                    state_mask: branch.state_mask,
                    short_key: branch.key,
                    branch_masks: masks.unwrap_or_default(),
                });
            }
            TrieNodeV2::Extension(_) => {
                panic!("set_root does not support Extension nodes; extensions are represented as branches with a short_key")
            }
        }

        Ok(())
    }

    fn set_updates(&mut self, retain_updates: bool) {
        self.updates = retain_updates.then(Default::default);
        if retain_updates {
            self.buffers.updates.get_or_insert_with(SparseTrieUpdates::default).clear();
        } else {
            self.buffers.updates = None;
        }
    }

    fn reserve_nodes(&mut self, additional: usize) {
        self.upper_arena.reserve(additional);
    }

    #[instrument(level = "trace", target = TRACE_TARGET, skip_all, fields(num_nodes = nodes.len()))]
    fn reveal_nodes(&mut self, nodes: &mut [ProofTrieNodeV2]) -> SparseTrieResult<()> {
        if nodes.is_empty() {
            return Ok(());
        }

        #[cfg(feature = "metrics")]
        let reveal_start = Instant::now();
        #[cfg(feature = "metrics")]
        let num_nodes = nodes.len();

        #[cfg(feature = "trie-debug")]
        self.debug_recorder.record(RecordedOp::RevealNodes {
            nodes: nodes.iter().map(ProofTrieNodeRecord::from_proof_trie_node_v2).collect(),
        });

        if matches!(self.upper_arena[self.root], ArenaSparseNode::EmptyRoot) {
            trace!(target: TRACE_TARGET, "Skipping reveal_nodes on empty root");
            return Ok(());
        }

        // Sort nodes lexicographically by path.
        nodes.sort_unstable_by_key(|n| n.path);

        let threshold = self.parallelism_thresholds.min_revealed_nodes;

        // Take the cursor out to avoid borrow conflicts with `self`.
        let mut cursor = mem::take(&mut self.buffers.cursor);
        cursor.reset(&self.upper_arena, self.root, Nibbles::default());

        // Skip root node if present (set_root handles the root).
        let mut node_idx = if nodes[0].path.is_empty() { 1 } else { 0 };

        // Walk the upper trie, revealing upper nodes inline and collecting subtrie work.
        // Subtries with enough nodes to reveal are taken for parallel processing; the rest
        // are revealed inline.
        let mut taken: Vec<(Index, Box<ArenaSparseSubtrie>, Vec<ProofTrieNodeV2>)> = Vec::new();

        while node_idx < nodes.len() {
            let find_result = cursor.seek(&mut self.upper_arena, &nodes[node_idx].path);

            match find_result {
                SeekResult::RevealedLeaf => {
                    trace!(target: TRACE_TARGET, path = ?nodes[node_idx].path, "Skipping reveal: leaf head");
                    node_idx += 1;
                }
                SeekResult::Blinded => {
                    // Save the proof node's path before reveal_node consumes it.
                    let child_path = nodes[node_idx].path;
                    let child_idx = Self::reveal_node(
                        &mut self.upper_arena,
                        &cursor,
                        &mut nodes[node_idx],
                        SeekResult::Blinded,
                    );
                    node_idx += 1;

                    if let Some(child_idx) = child_idx {
                        self.maybe_wrap_in_subtrie(child_idx, &child_path);
                    }
                }
                SeekResult::RevealedSubtrie => {
                    let subtrie_entry = cursor.head().expect("cursor is non-empty");
                    let child_idx = subtrie_entry.index;
                    let prefix = subtrie_entry.path;

                    let subtrie_start = node_idx;
                    while node_idx < nodes.len() && nodes[node_idx].path.starts_with(&prefix) {
                        node_idx += 1;
                    }
                    let num_subtrie_nodes = node_idx - subtrie_start;

                    if num_subtrie_nodes >= threshold {
                        // Take subtrie for parallel reveal.
                        trace!(target: TRACE_TARGET, ?prefix, num_subtrie_nodes, "Taking subtrie for parallel reveal");
                        let ArenaSparseNode::Subtrie(subtrie) = mem::replace(
                            &mut self.upper_arena[child_idx],
                            ArenaSparseNode::TakenSubtrie,
                        ) else {
                            unreachable!("RevealedSubtrie must point to a Subtrie node")
                        };
                        let node_vec: Vec<ProofTrieNodeV2> = (subtrie_start..node_idx)
                            .map(|i| mem::replace(&mut nodes[i], ProofTrieNodeV2::empty()))
                            .collect();
                        taken.push((child_idx, subtrie, node_vec));
                    } else {
                        // Reveal inline.
                        trace!(target: TRACE_TARGET, ?prefix, num_subtrie_nodes, "Revealing subtrie inline");
                        let ArenaSparseNode::Subtrie(subtrie) = &mut self.upper_arena[child_idx]
                        else {
                            unreachable!("RevealedSubtrie must point to a Subtrie node")
                        };
                        let mut subtrie_nodes: Vec<ProofTrieNodeV2> = (subtrie_start..node_idx)
                            .map(|i| mem::replace(&mut nodes[i], ProofTrieNodeV2::empty()))
                            .collect();
                        subtrie.reveal_nodes(&mut subtrie_nodes)?;
                    }
                }
                _ => {
                    trace!(target: TRACE_TARGET, path = ?nodes[node_idx].path, ?find_result, "Skipping reveal: no blinded child");
                    node_idx += 1;
                }
            }
        }

        // Drain remaining cursor entries from the upper-trie walk.
        cursor.drain(&mut self.upper_arena);
        self.buffers.cursor = cursor;

        if taken.is_empty() {
            #[cfg(feature = "metrics")]
            {
                self.metrics.reveal_nodes_total_latency.record(reveal_start.elapsed());
                self.metrics.reveal_nodes_count.record(num_nodes as f64);
                self.metrics.reveal_nodes_parallel_subtrie_count.record(0.0);
            }
            return Ok(());
        }

        #[cfg(feature = "metrics")]
        let parallel_subtrie_count = taken.len();

        // Reveal taken subtries, in parallel if more than one.
        if taken.len() == 1 {
            let (_, subtrie, node_vec) = &mut taken[0];
            subtrie.reveal_nodes(node_vec)?;
        } else {
            use rayon::iter::{IntoParallelRefMutIterator, ParallelIterator};

            let results: Vec<SparseTrieResult<()>> = taken
                .par_iter_mut()
                .map(|(_, subtrie, node_vec)| subtrie.reveal_nodes(node_vec))
                .collect();

            if let Some(err) = results.into_iter().find(|r| r.is_err()) {
                // Restore before returning so we don't leave TakenSubtrie holes.
                for (idx, subtrie, _) in taken {
                    self.upper_arena[idx] = ArenaSparseNode::Subtrie(subtrie);
                }
                return err;
            }
        }

        // Restore taken subtries into the upper arena.
        for (idx, subtrie, _) in taken {
            self.upper_arena[idx] = ArenaSparseNode::Subtrie(subtrie);
        }

        #[cfg(debug_assertions)]
        self.debug_assert_subtrie_structure();

        #[cfg(feature = "metrics")]
        {
            self.metrics.reveal_nodes_total_latency.record(reveal_start.elapsed());
            self.metrics.reveal_nodes_count.record(num_nodes as f64);
            self.metrics
                .reveal_nodes_parallel_subtrie_count
                .record(parallel_subtrie_count as f64);
        }

        Ok(())
    }

    fn update_leaf<P: crate::provider::TrieNodeProvider>(
        &mut self,
        _full_path: Nibbles,
        _value: Vec<u8>,
        _provider: P,
    ) -> SparseTrieResult<()> {
        unimplemented!("ArenaParallelSparseTrie uses update_leaves for batch leaf updates")
    }

    fn remove_leaf<P: crate::provider::TrieNodeProvider>(
        &mut self,
        _full_path: &Nibbles,
        _provider: P,
    ) -> SparseTrieResult<()> {
        unimplemented!("ArenaParallelSparseTrie uses update_leaves for batch leaf removals")
    }

    #[instrument(level = "trace", target = TRACE_TARGET, skip_all, ret)]
    fn root(&mut self) -> B256 {
        #[cfg(feature = "metrics")]
        let root_start = Instant::now();

        #[cfg(feature = "trie-debug")]
        self.debug_recorder.record(RecordedOp::Root);

        self.update_subtrie_hashes();

        #[cfg(feature = "metrics")]
        let upper_rlp_start = Instant::now();

        // Merge buffered subtrie updates into self.updates before hashing the upper trie,
        // which will add its own updates directly into self.updates.
        Self::merge_subtrie_updates(&mut self.updates, &mut self.buffers.updates);

        let rlp_node = Self::update_cached_rlp(
            &mut self.upper_arena,
            self.root,
            &mut self.buffers.cursor,
            &mut self.buffers.rlp_buf,
            &mut self.buffers.rlp_node_buf,
            Nibbles::default(),
            &mut self.updates,
        );

        #[cfg(feature = "metrics")]
        {
            self.metrics.root_upper_rlp_latency.record(upper_rlp_start.elapsed());
            self.metrics.root_total_latency.record(root_start.elapsed());
        }

        rlp_node.as_hash().expect("root RlpNode must be a hash")
    }

    fn is_root_cached(&self) -> bool {
        self.upper_arena[self.root].is_cached()
    }

    #[instrument(level = "trace", target = TRACE_TARGET, skip_all)]
    fn update_subtrie_hashes(&mut self) {
        #[cfg(feature = "metrics")]
        let subtrie_hashes_start = Instant::now();

        #[cfg(feature = "trie-debug")]
        self.debug_recorder.record(RecordedOp::UpdateSubtrieHashes);

        trace!(target: TRACE_TARGET, "Updating subtrie hashes");

        // Only descend if the root is a branch; otherwise there are no subtries.
        if !matches!(&self.upper_arena[self.root], ArenaSparseNode::Branch(_)) {
            #[cfg(feature = "metrics")]
            self.metrics.root_subtrie_hashes_latency.record(subtrie_hashes_start.elapsed());
            return;
        }

        // Count total dirty leaves across all subtries to make a global parallelism
        // decision, matching the approach in ParallelSparseTrie.
        let mut total_dirty_leaves: u64 = 0;
        let mut taken: Vec<(Index, Box<ArenaSparseSubtrie>)> = Vec::new();
        for (idx, node) in &mut self.upper_arena {
            let ArenaSparseNode::Subtrie(s) = node else { continue };
            if s.num_dirty_leaves == 0 {
                continue;
            }
            total_dirty_leaves += s.num_dirty_leaves;
            let ArenaSparseNode::Subtrie(subtrie) =
                mem::replace(node, ArenaSparseNode::TakenSubtrie)
            else {
                unreachable!()
            };
            taken.push((idx, subtrie));
        }

        #[cfg(feature = "metrics")]
        let taken_count = taken.len();

        // Hash taken subtries: in parallel if total dirty leaves meet the threshold,
        // otherwise serially. This mirrors ParallelSparseTrie's all-or-nothing approach.
        if !taken.is_empty() {
            if taken.len() == 1 || total_dirty_leaves < self.parallelism_thresholds.min_dirty_leaves
            {
                for (_, subtrie) in &mut taken {
                    subtrie.update_cached_rlp();
                }
            } else {
                use rayon::iter::{IntoParallelIterator, ParallelIterator};

                taken = taken
                    .into_par_iter()
                    .map(|(idx, mut subtrie)| {
                        subtrie.update_cached_rlp();
                        (idx, subtrie)
                    })
                    .collect();
            }
        }

        // If the root branch is already cached and nothing was taken for parallel
        // hashing, there are no dirty subtries to process.
        if taken.is_empty() && self.upper_arena[self.root].is_cached() {
            #[cfg(feature = "metrics")]
            {
                self.metrics.root_subtries_hashed_count.record(0.0);
                self.metrics.root_total_dirty_leaves.record(total_dirty_leaves as f64);
                self.metrics.root_subtrie_hashes_latency.record(subtrie_hashes_start.elapsed());
            }
            return;
        }

        // Walk the upper trie depth-first, restoring hashed subtries and inline-hashing
        // any remaining dirty subtries. Only descend into dirty branches; clean subtrees
        // cannot contain dirty subtries since dirty state propagates upward.
        taken.sort_unstable_by_key(|(_, b)| Reverse(b.path));

        self.buffers.cursor.reset(&self.upper_arena, self.root, Nibbles::default());

        loop {
            let result = self.buffers.cursor.next(&mut self.upper_arena, |_, child| match child {
                ArenaSparseNode::Branch(_) | ArenaSparseNode::Subtrie(_) => !child.is_cached(),
                ArenaSparseNode::TakenSubtrie => true,
                _ => false,
            });

            match result {
                NextResult::Done => break,
                NextResult::Branch => continue,
                NextResult::NonBranch => {}
            }

            // Head is a subtrie or taken-subtrie — process it.
            let head_idx = self.buffers.cursor.head().expect("cursor is non-empty").index;

            if matches!(&self.upper_arena[head_idx], ArenaSparseNode::TakenSubtrie) {
                let (_, subtrie) = taken.pop().expect("taken subtries must not be exhausted");
                debug_assert_eq!(
                    subtrie.path,
                    self.buffers.cursor.head().expect("cursor is non-empty").path,
                    "taken subtrie path mismatch",
                );
                self.upper_arena[head_idx] = ArenaSparseNode::Subtrie(subtrie);
            }

            self.update_upper_subtrie(head_idx);
        }

        #[cfg(feature = "metrics")]
        {
            self.metrics.root_subtries_hashed_count.record(taken_count as f64);
            self.metrics.root_total_dirty_leaves.record(total_dirty_leaves as f64);
            self.metrics.root_subtrie_hashes_latency.record(subtrie_hashes_start.elapsed());
        }
    }

    fn get_leaf_value(&self, full_path: &Nibbles) -> Option<&Vec<u8>> {
        Self::get_leaf_value_in_arena(&self.upper_arena, self.root, full_path, 0)
    }

    fn find_leaf(
        &self,
        full_path: &Nibbles,
        expected_value: Option<&Vec<u8>>,
    ) -> Result<LeafLookup, LeafLookupError> {
        Self::find_leaf_in_arena(&self.upper_arena, self.root, full_path, 0, expected_value)
    }

    fn updates_ref(&self) -> Cow<'_, SparseTrieUpdates> {
        self.updates.as_ref().map_or(Cow::Owned(SparseTrieUpdates::default()), Cow::Borrowed)
    }

    fn take_updates(&mut self) -> SparseTrieUpdates {
        match self.updates.take() {
            Some(updates) => {
                self.updates = Some(SparseTrieUpdates::with_capacity(
                    updates.updated_nodes.len(),
                    updates.removed_nodes.len(),
                ));
                updates
            }
            None => SparseTrieUpdates::default(),
        }
    }

    #[instrument(level = "trace", target = TRACE_TARGET, skip_all)]
    fn wipe(&mut self) {
        trace!(target: TRACE_TARGET, "Wiping arena trie");
        self.clear();
        self.updates = self.updates.is_some().then(SparseTrieUpdates::wiped);
    }

    #[instrument(level = "trace", target = TRACE_TARGET, skip_all)]
    fn clear(&mut self) {
        #[cfg(feature = "trie-debug")]
        self.debug_recorder.reset();

        for idx in self.all_subtries() {
            if let ArenaSparseNode::Subtrie(mut subtrie) =
                self.upper_arena.remove(idx).expect("subtrie exists in arena")
            {
                subtrie.clear();
                self.cleared_subtries.push(subtrie);
            }
        }
        self.upper_arena.clear();
        self.root = self.upper_arena.insert(ArenaSparseNode::EmptyRoot);
        if let Some(updates) = self.updates.as_mut() {
            updates.clear()
        }
        self.buffers.clear();
    }

    fn shrink_nodes_to(&mut self, size: usize) {
        self.upper_arena.shrink_to(size);
        for (_, node) in &mut self.upper_arena {
            if let ArenaSparseNode::Subtrie(s) = node {
                s.arena.shrink_to(size);
            }
        }
        for s in &mut self.cleared_subtries {
            s.arena.shrink_to(size);
        }
    }

    fn shrink_values_to(&mut self, _size: usize) {
        // Values are stored inline in nodes; no separate value storage.
    }

    fn size_hint(&self) -> usize {
        self.upper_arena
            .iter()
            .map(|(_, node)| match node {
                ArenaSparseNode::Subtrie(s) => s.num_leaves as usize,
                ArenaSparseNode::Leaf { .. } => 1,
                _ => 0,
            })
            .sum()
    }

    fn memory_size(&self) -> usize {
        let node_size = core::mem::size_of::<ArenaSparseNode>();

        // Upper arena: capacity × node size.
        let upper = self.upper_arena.capacity() * node_size;

        // Active subtries: capacity × node size per subtrie arena.
        let subtrie_size: usize = self
            .upper_arena
            .iter()
            .filter_map(|(_, node)| match node {
                ArenaSparseNode::Subtrie(s) => Some(s.arena.capacity() * node_size),
                _ => None,
            })
            .sum();

        // Cleared subtries still hold allocated arena capacity.
        let cleared_size: usize =
            self.cleared_subtries.iter().map(|s| s.arena.capacity() * node_size).sum();

        // RLP buffers.
        let buffer_size = self.buffers.rlp_buf.capacity()
            + self.buffers.rlp_node_buf.capacity() * core::mem::size_of::<RlpNode>();

        upper + subtrie_size + cleared_size + buffer_size
    }

    #[instrument(
        level = "trace",
        target = TRACE_TARGET,
        skip_all,
        fields(num_retained_leaves = retained_leaves.len()),
    )]
    fn prune(&mut self, retained_leaves: &[Nibbles]) -> usize {
        #[cfg(feature = "metrics")]
        let prune_start = Instant::now();
        #[cfg(feature = "metrics")]
        let num_retained = retained_leaves.len();

        // Only descend if the root is a branch; otherwise there are no subtries.
        if !matches!(&self.upper_arena[self.root], ArenaSparseNode::Branch(_)) {
            #[cfg(feature = "metrics")]
            {
                self.metrics.prune_total_latency.record(prune_start.elapsed());
                self.metrics.prune_nodes_pruned.record(0.0);
                self.metrics.prune_retained_leaves_count.record(num_retained as f64);
                self.metrics.prune_parallel_subtrie_count.record(0.0);
            }
            return 0;
        }

        let mut retained_leaves = retained_leaves.to_vec();
        retained_leaves.sort_unstable();

        let threshold = self.parallelism_thresholds.min_leaves_for_prune;

        let mut cursor = mem::take(&mut self.buffers.cursor);
        cursor.reset(&self.upper_arena, self.root, Nibbles::default());

        // Subtries taken for parallel pruning: (arena_index, subtrie, retained_range).
        let mut taken: Vec<(Index, Box<ArenaSparseSubtrie>, core::ops::Range<usize>)> = Vec::new();

        let mut pruned = 0;
        let mut retained_idx = 0;

        loop {
            let result = cursor.next(&mut self.upper_arena, |_, child| {
                matches!(
                    child,
                    ArenaSparseNode::Branch(_)
                        | ArenaSparseNode::Subtrie(_)
                        | ArenaSparseNode::Leaf { .. }
                )
            });

            match result {
                NextResult::Done => break,
                NextResult::NonBranch | NextResult::Branch => {}
            }

            let head = cursor.head().expect("cursor is non-empty");
            let head_idx = head.index;
            let head_path = head.path;

            match &self.upper_arena[head_idx] {
                ArenaSparseNode::Branch(_) | ArenaSparseNode::Leaf { .. } => {
                    // Don't prune the root.
                    if cursor.depth() == 0 {
                        continue;
                    }

                    let short_key =
                        self.upper_arena[head_idx].short_key().expect("must be branch or leaf");
                    let mut node_prefix = head_path;
                    node_prefix.extend(short_key);

                    let range = prefix_range(&retained_leaves, 0, &node_prefix);
                    if !range.is_empty() {
                        continue;
                    }

                    Self::remove_pruned_node(
                        &mut self.upper_arena,
                        &cursor,
                        head_idx,
                        head_path.last(),
                    );
                    pruned += 1;
                }
                ArenaSparseNode::Subtrie(_) => {
                    let subtrie_range = prefix_range(&retained_leaves, retained_idx, &head_path);
                    retained_idx = subtrie_range.end;

                    if subtrie_range.is_empty() {
                        let removed = Self::remove_pruned_node(
                            &mut self.upper_arena,
                            &cursor,
                            head_idx,
                            head_path.last(),
                        );
                        let ArenaSparseNode::Subtrie(s) = &removed else { unreachable!() };
                        pruned += s.num_leaves as usize;
                        self.recycle_subtrie(removed);
                        continue;
                    }

                    let ArenaSparseNode::Subtrie(subtrie) = &self.upper_arena[head_idx] else {
                        unreachable!()
                    };
                    if subtrie.num_leaves >= threshold {
                        let ArenaSparseNode::Subtrie(subtrie) = mem::replace(
                            &mut self.upper_arena[head_idx],
                            ArenaSparseNode::TakenSubtrie,
                        ) else {
                            unreachable!()
                        };
                        taken.push((head_idx, subtrie, subtrie_range));
                    } else {
                        let ArenaSparseNode::Subtrie(subtrie) = &mut self.upper_arena[head_idx]
                        else {
                            unreachable!()
                        };
                        pruned += subtrie.prune(&retained_leaves[subtrie_range]);
                    }
                }
                _ => unreachable!("NonBranch in prune walk must be Subtrie, Leaf, or Branch"),
            }
        }

        self.buffers.cursor = cursor;

        #[cfg(feature = "metrics")]
        let parallel_subtrie_count = taken.len();

        if !taken.is_empty() {
            // Prune taken subtries, in parallel if more than one.
            if taken.len() == 1 {
                let (_, ref mut subtrie, ref range) = taken[0];
                pruned += subtrie.prune(&retained_leaves[range.clone()]);
            } else {
                use rayon::iter::{IntoParallelRefMutIterator, ParallelIterator};

                pruned += taken
                    .par_iter_mut()
                    .map(|(_, subtrie, range)| subtrie.prune(&retained_leaves[range.clone()]))
                    .sum::<usize>();
            }

            // Restore taken subtries into the upper arena.
            for (child_idx, subtrie, _) in taken {
                self.upper_arena[child_idx] = ArenaSparseNode::Subtrie(subtrie);
            }
        }

        #[cfg(feature = "trie-debug")]
        self.record_initial_state();

        #[cfg(feature = "metrics")]
        {
            self.metrics.prune_total_latency.record(prune_start.elapsed());
            self.metrics.prune_nodes_pruned.record(pruned as f64);
            self.metrics.prune_retained_leaves_count.record(num_retained as f64);
            self.metrics.prune_parallel_subtrie_count.record(parallel_subtrie_count as f64);
        }

        pruned
    }

    #[instrument(
        level = "trace",
        target = TRACE_TARGET,
        skip_all,
        fields(num_updates = updates.len()),
    )]
    fn update_leaves(
        &mut self,
        updates: &mut B256Map<LeafUpdate>,
        mut proof_required_fn: impl FnMut(B256, u8),
    ) -> SparseTrieResult<()> {
        if updates.is_empty() {
            return Ok(());
        }

        #[cfg(feature = "metrics")]
        let total_start = Instant::now();
        #[cfg(feature = "metrics")]
        let num_updates = updates.len();

        #[cfg(feature = "trie-debug")]
        let recorded_updates: Vec<_> =
            updates.iter().map(|(k, v)| (*k, LeafUpdateRecord::from(v))).collect();
        #[cfg(feature = "trie-debug")]
        let mut recorded_proof_targets: Vec<(B256, u8)> = Vec::new();

        // Drain and sort updates lexicographically by nibbles path.
        #[cfg(feature = "metrics")]
        let drain_sort_start = Instant::now();
        let mut sorted: Vec<_> =
            updates.drain().map(|(key, update)| (key, Nibbles::unpack(key), update)).collect();
        sorted.sort_unstable_by_key(|entry| entry.1);
        #[cfg(feature = "metrics")]
        self.metrics.update_leaves_drain_sort_latency.record(drain_sort_start.elapsed());

        let mut cursor = mem::take(&mut self.buffers.cursor);
        cursor.reset(&self.upper_arena, self.root, Nibbles::default());

        // Subtries taken for parallel processing: (arena_index, subtrie, update_range).
        let mut taken: Vec<(Index, Box<ArenaSparseSubtrie>, core::ops::Range<usize>)> = Vec::new();

        #[cfg(feature = "metrics")]
        let upper_walk_start = Instant::now();
        #[cfg(feature = "metrics")]
        let inline_subtrie_elapsed = core::time::Duration::ZERO;
        #[cfg(feature = "metrics")]
        let inline_subtrie_count = 0u64;
        #[cfg(feature = "metrics")]
        let mut upper_seek_count = 0u64;

        let mut update_idx = 0;
        while update_idx < sorted.len() {
            let (key, ref full_path, ref update) = sorted[update_idx];

            let find_result = cursor.seek(&mut self.upper_arena, full_path);
            #[cfg(feature = "metrics")]
            {
                upper_seek_count += 1;
            }

            match find_result {
                // Blinded — request a proof regardless of update type.
                SeekResult::Blinded => {
                    let logical_len = cursor.head_logical_branch_path_len(&self.upper_arena);
                    let min_len = (logical_len as u8 + 1).min(64);
                    trace!(target: TRACE_TARGET, ?key, min_len, "Update hit blinded node, requesting proof");
                    proof_required_fn(key, min_len);
                    #[cfg(feature = "trie-debug")]
                    recorded_proof_targets.push((key, min_len));
                    updates.insert(key, update.clone());
                }
                // Subtrie — forward all consecutive updates under this subtrie's prefix.
                SeekResult::RevealedSubtrie => {
                    let subtrie_entry = cursor.head().expect("cursor is non-empty");
                    let child_idx = subtrie_entry.index;
                    let subtrie_root_path = subtrie_entry.path;

                    let subtrie_start = update_idx;
                    while update_idx < sorted.len()
                        && sorted[update_idx].1.starts_with(&subtrie_root_path)
                    {
                        update_idx += 1;
                    }

                    let subtrie_updates = &sorted[subtrie_start..update_idx];

                    // Edge-case: if all updates are removals that could empty the
                    // subtrie and collapse the parent onto a blinded sibling, request
                    // a proof for the sibling and skip the subtrie's updates.
                    if let Some(proof) = Self::check_subtrie_collapse_needs_proof(
                        &self.upper_arena,
                        &cursor,
                        subtrie_updates,
                    ) {
                        trace!(target: TRACE_TARGET, proof_key = ?proof.key, proof_min_len = proof.min_len, "Subtrie collapse would need blinded sibling, requesting proof");
                        proof_required_fn(proof.key, proof.min_len);
                        #[cfg(feature = "trie-debug")]
                        recorded_proof_targets.push((proof.key, proof.min_len));
                        for &(key, _, ref update) in subtrie_updates {
                            updates.insert(key, update.clone());
                        }
                        // Pop the subtrie entry before continuing.
                        continue;
                    }

                    // Take subtrie for batched processing.
                    trace!(target: TRACE_TARGET, ?subtrie_root_path, num_subtrie_updates = update_idx - subtrie_start, "Taking subtrie");
                    let ArenaSparseNode::Subtrie(subtrie) = mem::replace(
                        &mut self.upper_arena[child_idx],
                        ArenaSparseNode::TakenSubtrie,
                    ) else {
                        unreachable!()
                    };
                    taken.push((child_idx, subtrie, subtrie_start..update_idx));

                    // Don't increment update_idx — already advanced past subtrie updates.
                    continue;
                }
                // EmptyRoot, leaf, diverged branch, or empty child slot — upsert directly.
                find_result @ (SeekResult::EmptyRoot
                | SeekResult::RevealedLeaf
                | SeekResult::Diverged
                | SeekResult::NoChild { .. }) => match update {
                    LeafUpdate::Changed(v) if !v.is_empty() => {
                        let (result, _deltas) = Self::upsert_leaf(
                            &mut self.upper_arena,
                            &mut cursor,
                            &mut self.root,
                            full_path,
                            v,
                            find_result,
                        );
                        match result {
                            UpsertLeafResult::NewChild => {
                                let head = cursor.head().expect("cursor is non-empty");
                                if Self::should_be_subtrie(head.path.len()) {
                                    // The new child itself sits at the subtrie
                                    // boundary — wrap it directly.
                                    self.maybe_wrap_in_subtrie(head.index, &head.path);
                                } else {
                                    // The new child is above the boundary (e.g. a
                                    // split at depth 1 creates children at depth 2).
                                    // Wrap any of its children that land there.
                                    self.maybe_wrap_branch_children(&cursor);
                                }
                            }
                            UpsertLeafResult::NewLeaf => {
                                // A root-level split may create children at the
                                // subtrie boundary depth. Wrap them.
                                self.maybe_wrap_branch_children(&cursor);
                            }
                            UpsertLeafResult::Updated => {}
                        }
                    }
                    LeafUpdate::Changed(_) => {
                        let (result, _deltas) = Self::remove_leaf(
                            &mut self.upper_arena,
                            &mut cursor,
                            &mut self.root,
                            key,
                            full_path,
                            find_result,
                            &mut self.buffers.updates,
                        );
                        match result {
                            RemoveLeafResult::NeedsProof { key, proof_key, min_len } => {
                                proof_required_fn(proof_key, min_len);
                                #[cfg(feature = "trie-debug")]
                                recorded_proof_targets.push((proof_key, min_len));
                                let update =
                                    mem::replace(&mut sorted[update_idx].2, LeafUpdate::Touched);
                                updates.insert(key, update);
                            }
                            RemoveLeafResult::Removed => {
                                // remove_leaf may have called collapse_branch, which
                                // can leave structural invariants violated:
                                // 1. A branch with 0-1 children that needs further collapse or
                                //    removal.
                                // 2. A Subtrie at a depth shallower than UPPER_TRIE_MAX_DEPTH that
                                //    needs unwrapping.
                                // 3. A non-Subtrie node at UPPER_TRIE_MAX_DEPTH that needs
                                //    wrapping.
                                self.maybe_collapse_or_remove_branch(&mut cursor);
                                let head =
                                    cursor.head().expect("cursor always has root after collapse");
                                self.maybe_wrap_in_subtrie(head.index, &head.path);
                            }
                            RemoveLeafResult::NotFound => {}
                        }
                    }
                    LeafUpdate::Touched => {}
                },
            }

            update_idx += 1;
        }

        // Drain remaining cursor entries from the upper-trie walk.
        cursor.drain(&mut self.upper_arena);
        self.buffers.cursor = cursor;

        #[cfg(feature = "metrics")]
        {
            let upper_walk_elapsed = upper_walk_start.elapsed() - inline_subtrie_elapsed;
            self.metrics.update_leaves_upper_walk_latency.record(upper_walk_elapsed);
            self.metrics.update_leaves_inline_subtrie_latency.record(inline_subtrie_elapsed);
            self.metrics.update_leaves_inline_subtrie_count.record(inline_subtrie_count as f64);
            self.metrics.update_leaves_upper_seek_count.record(upper_seek_count as f64);
        }

        if taken.is_empty() {
            #[cfg(feature = "metrics")]
            {
                self.metrics.update_leaves_num_updates.record(num_updates as f64);
                self.metrics.update_leaves_parallel_subtrie_count.record(0.0);
                self.metrics.update_leaves_total_latency.record(total_start.elapsed());
            }

            #[cfg(debug_assertions)]
            self.debug_assert_subtrie_structure();

            #[cfg(feature = "trie-debug")]
            self.debug_recorder.record(RecordedOp::UpdateLeaves {
                updates: recorded_updates,
                proof_targets: recorded_proof_targets,
            });

            return Ok(());
        }

        // Decide parallel vs serial based on total updates across all taken subtries.
        let total_taken_updates: usize =
            taken.iter().map(|(_, _, range)| range.len()).sum();
        let use_parallel = taken.len() > 1
            && total_taken_updates >= self.parallelism_thresholds.min_updates;

        #[cfg(feature = "metrics")]
        let parallel_start = Instant::now();

        if use_parallel {
            // Dispatch to persistent worker pool.
            #[cfg(feature = "metrics")]
            {
                let now = Instant::now();
                for (_, subtrie, _) in &mut taken {
                    subtrie.dispatch_timestamp = now;
                }
            }

            // Split taken into chunks for the worker pool. We process items
            // in pairs: one per worker thread plus remainder on the main thread.
            let num_workers = self.worker_pool.num_workers();

            // Dispatch up to `num_workers` subtries to the pool, process
            // the rest on the main thread.
            let pool_count = taken.len().min(num_workers);

            // SAFETY: we use SendPtr to move raw pointers into Send closures.
            // Each worker gets exclusive access to its own subtrie (disjoint).
            // `sorted` is not modified during execution. Both `taken` and
            // `sorted` outlive the pool.execute call because we block until
            // all workers complete.
            for entry in taken[..pool_count].iter_mut() {
                let subtrie_ptr = SendPtr(&mut *entry.1 as *mut ArenaSparseSubtrie);
                let sorted_ptr = SendPtr(sorted.as_ptr() as *mut (B256, Nibbles, LeafUpdate));
                let sorted_len = sorted.len();
                let range = entry.2.clone();
                self.worker_pool.execute(move || {
                    let subtrie = unsafe { &mut *subtrie_ptr.get() };
                    let sorted =
                        unsafe { core::slice::from_raw_parts(sorted_ptr.get(), sorted_len) };
                    subtrie.update_leaves(&sorted[range]);
                });
            }

            // Process remaining entries on the main thread while workers run.
            for entry in taken[pool_count..].iter_mut() {
                entry.1.update_leaves(&sorted[entry.2.clone()]);
            }

            // Wait for all pool workers to finish.
            self.worker_pool.join();
        } else {
            // Process all taken subtries serially on the main thread.
            for entry in &mut taken {
                entry.1.update_leaves(&sorted[entry.2.clone()]);
            }
        }

        #[cfg(feature = "metrics")]
        {
            self.metrics.update_leaves_parallel_subtrie_latency.record(parallel_start.elapsed());
            self.metrics
                .update_leaves_parallel_subtrie_count
                .record(if use_parallel { taken.len() as f64 } else { 0.0 });

            if use_parallel {
                // Record per-subtrie elapsed times for straggler analysis.
                let mut max_dur = core::time::Duration::ZERO;
                let mut min_dur = core::time::Duration::MAX;
                for (_, subtrie, _) in &taken {
                    let dur = subtrie.last_update_leaves_elapsed;
                    if dur > max_dur {
                        max_dur = dur;
                    }
                    if dur < min_dur {
                        min_dur = dur;
                    }
                    self.metrics
                        .subtrie_update_leaves_schedule_delay
                        .record(subtrie.last_schedule_delay);
                }
                self.metrics.update_leaves_parallel_max_subtrie_latency.record(max_dur);
                self.metrics.update_leaves_parallel_min_subtrie_latency.record(min_dur);
                self.metrics.update_leaves_parallel_straggler_delta.record(max_dur - min_dur);
            }
        }

        // Collect subtrie info before consuming `taken`, then restore subtries and
        // process required proofs.
        let mut taken_info: Vec<TakenInfo> = Vec::with_capacity(taken.len());

        for (child_idx, mut subtrie, range) in taken {
            let is_empty = matches!(subtrie.arena[subtrie.root], ArenaSparseNode::EmptyRoot);
            taken_info.push(TakenInfo { path: subtrie.path, range: range.clone(), is_empty });

            let subtrie_updates = &sorted[range];
            for (target_idx, proof) in subtrie.required_proofs.drain(..) {
                proof_required_fn(proof.key, proof.min_len);
                #[cfg(feature = "trie-debug")]
                recorded_proof_targets.push((proof.key, proof.min_len));
                let (key, _, ref update) = subtrie_updates[target_idx];
                updates.insert(key, update.clone());
            }

            // Restore the subtrie into the upper arena.
            self.upper_arena[child_idx] = ArenaSparseNode::Subtrie(subtrie);
        }

        // Navigate to each taken subtrie via seek to propagate dirty state
        // through intermediate branches, and collapse any EmptyRoot subtries.
        //
        // Before unwrapping, we pre-scan for the multi-subtrie-emptying case:
        // if removing all empty subtries from a parent branch would leave only
        // blinded children, we must request proofs for those blinded children
        // and skip the unwraps (re-inserting affected updates for retry).
        #[cfg(feature = "metrics")]
        let post_parallel_start = Instant::now();
        {
            let mut cursor = mem::take(&mut self.buffers.cursor);
            cursor.reset(&self.upper_arena, self.root, Nibbles::default());

            // Pre-scan: identify parents where unwrapping all empty subtries
            // would cascade onto a blinded sibling. For such parents, collect
            // a proof request for the blinded sibling and skip unwrapping.
            //
            // We group empty subtries by their parent nibble prefix. A subtrie
            // path is 2 nibbles (UPPER_TRIE_MAX_DEPTH); its parent path is
            // the branch's logical path (everything before the child nibble).
            // For each parent, we count how many children would remain after
            // removing all empty subtries, and whether those remaining children
            // include any blinded nodes.
            let skip_unwrap_indices = self.find_unsafe_empty_subtries(
                &taken_info,
                &mut cursor,
                &sorted,
                updates,
                &mut proof_required_fn,
                #[cfg(feature = "trie-debug")]
                &mut recorded_proof_targets,
            );

            // Now walk taken subtries: unwrap empty ones (unless skipped) and
            // propagate dirty state for non-empty ones.
            for (i, info) in taken_info.iter().enumerate() {
                let find_result = cursor.seek(&mut self.upper_arena, &info.path);
                match find_result {
                    SeekResult::RevealedSubtrie => {
                        if info.is_empty && !skip_unwrap_indices.contains(&i) {
                            self.maybe_unwrap_subtrie(&mut cursor);
                            continue;
                        }
                        cursor.pop(&mut self.upper_arena);
                    }
                    _ => {
                        // Subtrie was already unwrapped by a prior collapse; dirty state
                        // was propagated during that collapse. Nothing to do.
                    }
                }
            }

            cursor.drain(&mut self.upper_arena);
            self.buffers.cursor = cursor;
        }
        #[cfg(feature = "metrics")]
        self.metrics.update_leaves_post_parallel_latency.record(post_parallel_start.elapsed());

        #[cfg(feature = "metrics")]
        {
            self.metrics.update_leaves_num_updates.record(num_updates as f64);
            self.metrics.update_leaves_total_latency.record(total_start.elapsed());
        }

        #[cfg(debug_assertions)]
        self.debug_assert_subtrie_structure();

        #[cfg(feature = "trie-debug")]
        self.debug_recorder.record(RecordedOp::UpdateLeaves {
            updates: recorded_updates,
            proof_targets: recorded_proof_targets,
        });

        Ok(())
    }

    #[cfg(feature = "trie-debug")]
    fn take_debug_recorder(&mut self) -> TrieDebugRecorder {
        core::mem::take(&mut self.debug_recorder)
    }

    fn commit_updates(
        &mut self,
        _updated: &HashMap<Nibbles, BranchNodeCompact>,
        _removed: &HashSet<Nibbles>,
    ) {
        // no-op for arena trie
    }
}

#[cfg(test)]
mod tests {
    use super::TRACE_TARGET;
    use crate::{ArenaParallelSparseTrie, ArenaParallelismThresholds, LeafUpdate, SparseTrie};
    use alloy_primitives::{map::B256Map, B256, U256};
    use rand::{seq::SliceRandom, Rng, SeedableRng};
    use reth_trie::test_utils::TrieTestHarness;
    use reth_trie_common::{Nibbles, ProofV2Target};
    use std::collections::BTreeMap;
    use tracing::{info, trace};

    /// Test harness for proptest-based arena sparse trie testing.
    ///
    /// Wraps [`TrieTestHarness`] and adds `ArenaParallelSparseTrie`-specific helpers for
    /// the reveal-update loop and asserting that sparse trie updates match `StorageRoot`.
    struct ArenaTrieTestHarness {
        /// The inner general-purpose harness.
        inner: TrieTestHarness,
    }

    impl std::ops::Deref for ArenaTrieTestHarness {
        type Target = TrieTestHarness;
        fn deref(&self) -> &Self::Target {
            &self.inner
        }
    }

    impl std::ops::DerefMut for ArenaTrieTestHarness {
        fn deref_mut(&mut self) -> &mut Self::Target {
            &mut self.inner
        }
    }

    impl ArenaTrieTestHarness {
        /// Creates a new test harness from a map of hashed storage slots to values.
        fn new(storage: BTreeMap<B256, U256>) -> Self {
            Self { inner: TrieTestHarness::new(storage) }
        }

        /// Computes the new storage root and trie updates after applying the given changes
        /// using both `StorageRoot` and the provided `ArenaParallelSparseTrie`, then asserts
        /// they match.
        fn assert_changes(
            &self,
            apst: &mut ArenaParallelSparseTrie,
            changes: BTreeMap<B256, U256>,
        ) {
            // Compute expected root and trie updates via StorageRoot.
            let (expected_root, mut expected_trie_updates) = if changes.is_empty() {
                (self.original_root(), Default::default())
            } else {
                self.get_root_with_updates(&changes)
            };

            self.minimize_trie_updates(&mut expected_trie_updates);

            // Build leaf updates for the APST: non-zero values are upserts (RLP-encoded),
            // zero values are deletions (empty vec).
            let mut leaf_updates: B256Map<LeafUpdate> = changes
                .iter()
                .map(|(&slot, &value)| {
                    let rlp_value = if value == U256::ZERO {
                        Vec::new()
                    } else {
                        alloy_rlp::encode_fixed_size(&value).to_vec()
                    };
                    (slot, LeafUpdate::Changed(rlp_value))
                })
                .collect();

            // Reveal-update loop: call update_leaves, collect required proofs, fetch them,
            // reveal, and repeat until no more proofs are needed.
            loop {
                let mut targets: Vec<ProofV2Target> = Vec::new();
                apst.update_leaves(&mut leaf_updates, |key, min_len| {
                    targets.push(ProofV2Target::new(key).with_min_len(min_len));
                })
                .expect("update_leaves should succeed");

                if targets.is_empty() {
                    break;
                }

                let (mut proof_nodes, _) = self.proof_v2(&mut targets);
                apst.reveal_nodes(&mut proof_nodes).expect("reveal_nodes should succeed");
            }

            // Compute root and take updates from the APST.
            let actual_root = apst.root();
            let mut actual_updates = apst.take_updates();

            // Minimize sparse updates inline (can't use TrieTestHarness::minimize_sparse_updates
            // due to the crate's SparseTrieUpdates being a different type than reth-trie's copy).
            actual_updates.updated_nodes.retain(|path, node| {
                self.storage_trie_updates().storage_nodes.get(path) != Some(node)
            });
            actual_updates
                .removed_nodes
                .retain(|path| self.storage_trie_updates().storage_nodes.contains_key(path));

            pretty_assertions::assert_eq!(
                expected_trie_updates.storage_nodes.into_iter().collect::<Vec<_>>().sort(),
                actual_updates.updated_nodes.into_iter().collect::<Vec<_>>().sort(),
                "updated nodes mismatch"
            );
            pretty_assertions::assert_eq!(
                expected_trie_updates.removed_nodes.into_iter().collect::<Vec<_>>().sort(),
                actual_updates.removed_nodes.into_iter().collect::<Vec<_>>().sort(),
                "removed nodes mismatch"
            );
            assert_eq!(expected_root, actual_root, "storage root mismatch");
        }
    }

    use proptest::prelude::*;
    use proptest_arbitrary_interop::arb;

    /// Builds a changeset by mixing `new_keys` (fresh insertions) with a fraction of
    /// existing keys from `base` (updates/deletions).
    ///
    /// `overlap_pct` controls how many existing keys are included, and `delete_pct`
    /// controls how many of those become deletions (zero values). The remaining
    /// overlap keys get random non-zero values.
    fn build_changeset(
        base: &BTreeMap<B256, U256>,
        new_keys: BTreeMap<B256, U256>,
        overlap_pct: f64,
        delete_pct: f64,
        rng: &mut rand::rngs::StdRng,
    ) -> BTreeMap<B256, U256> {
        let num_overlap = (base.len() as f64 * overlap_pct) as usize;
        let num_delete = (num_overlap as f64 * delete_pct) as usize;

        let mut all_keys: Vec<B256> = base.keys().copied().collect();
        all_keys.shuffle(rng);
        let overlap_keys = &all_keys[..num_overlap];

        let mut changeset = new_keys;
        for (i, &key) in overlap_keys.iter().enumerate() {
            let value =
                if i < num_delete { U256::ZERO } else { U256::from(rng.random::<u64>() | 1) };
            changeset.entry(key).or_insert(value);
        }
        changeset
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(1000))]
        #[test]
        fn arena_trie_proptest(
            initial in proptest::collection::btree_map(arb::<B256>(), arb::<U256>(), 0..=100usize),
            changeset1_new_keys in proptest::collection::btree_map(arb::<B256>(), arb::<U256>(), 0..=30usize),
            changeset2_new_keys in proptest::collection::btree_map(arb::<B256>(), arb::<U256>(), 0..=30usize),
            overlap_pct in 0.0..=0.5f64,
            delete_pct in 0.0..=0.33f64, // percent of overlapping changeset which are deletes
            shuffle_seed in arb::<u64>(),
        ) {
            reth_tracing::init_test_tracing();
            info!(target: TRACE_TARGET, ?shuffle_seed, "PROPTEST START");

            // Filter out zero-valued entries from the initial dataset (zeros mean "absent").
            let initial: BTreeMap<B256, U256> = initial.into_iter()
                .filter(|(_, v)| *v != U256::ZERO)
                .collect();

            let mut rng = rand::rngs::StdRng::seed_from_u64(shuffle_seed);

            let changeset1 = build_changeset(&initial, changeset1_new_keys, overlap_pct, delete_pct, &mut rng);
            for (i, (k, v)) in changeset1.iter().enumerate() {
                trace!(target: TRACE_TARGET, ?i, ?k, ?v, "Changeset 1 entry");
            }

            let mut harness = ArenaTrieTestHarness::new(initial);

            // Initialize the APST from the harness root node.
            let root_node = harness.root_node();
            let mut apst = ArenaParallelSparseTrie::default().with_parallelism_thresholds(
                ArenaParallelismThresholds {
                    min_dirty_leaves: 3,
                    min_revealed_nodes: 3,
                    min_updates: 3,
                    min_leaves_for_prune: 3,
                },
            );
            apst.set_root(root_node.node, root_node.masks, true).expect("set_root should succeed");

            harness.assert_changes(&mut apst, changeset1.clone());

            // Update the harness base dataset to reflect the first changeset.
            harness.apply_changeset(changeset1);

            // Pick N random keys from the current storage as retained leaves for pruning.
            let mut all_storage_keys: Vec<Nibbles> = harness.storage().keys()
                .map(|k| Nibbles::unpack(*k))
                .collect();
            all_storage_keys.shuffle(&mut rng);
            let num_retain = if all_storage_keys.is_empty() { 0 } else {
                rng.random_range(0..=all_storage_keys.len())
            };
            let retained: Vec<Nibbles> = all_storage_keys[..num_retain].to_vec();
            apst.prune(&retained);

            let changeset2 = build_changeset(harness.storage(), changeset2_new_keys, overlap_pct, delete_pct, &mut rng);
            for (i, (k, v)) in changeset2.iter().enumerate() {
                trace!(target: TRACE_TARGET, ?i, ?k, ?v, "Changeset 2 entry");
            }

            harness.assert_changes(&mut apst, changeset2);
        }
    }

    /// Minimal reproduction of the blinded-sibling collapse panic.
    ///
    /// A parent branch at nibble `0xd` has 3 children: subtries `0xd7`, `0xd8`,
    /// `0xdd` — each containing a single leaf.  When `min_updates = 1` forces the
    /// parallel path, subtries `0xd7` and `0xdd` are both taken and emptied by
    /// all-removal updates.  `check_subtrie_collapse_needs_proof` evaluates each
    /// independently and sees the parent with 3 children (safe), so it lets both
    /// through.  During the post-restore unwrap phase, unwrapping `0xd7` drops the
    /// parent to 2 children, then unwrapping `0xdd` drops it to 1 — the still-
    /// blinded `0xd8` — triggering the panic in `maybe_collapse_or_remove_branch`.
    #[test]
    fn test_blinded_sibling_collapse_with_forced_parallel_path() {
        let key = |b0: u8| {
            let mut k = [0u8; 32];
            k[0] = b0;
            B256::from(k)
        };

        // Three keys under nibble 0xd (each becomes its own subtrie at depth 2).
        // A fourth key under a different nibble so the root is a multi-child branch.
        let initial: BTreeMap<B256, U256> = [
            (key(0xd7), U256::from(1)),
            (key(0xd8), U256::from(2)),
            (key(0xdd), U256::from(3)),
            (key(0xaa), U256::from(4)),
        ]
        .into();

        // Delete the two outer siblings; leave 0xd8 untouched (blinded).
        let changeset: BTreeMap<B256, U256> =
            [(key(0xd7), U256::ZERO), (key(0xdd), U256::ZERO)].into();

        let harness = ArenaTrieTestHarness::new(initial);

        let mut apst = ArenaParallelSparseTrie::default().with_parallelism_thresholds(
            ArenaParallelismThresholds {
                min_dirty_leaves: 1,
                min_revealed_nodes: 1,
                min_updates: 1,
                min_leaves_for_prune: 1,
            },
        );

        let root_node = harness.root_node();
        apst.set_root(root_node.node, root_node.masks, true).expect("set_root should succeed");

        harness.assert_changes(&mut apst, changeset);
    }
}
