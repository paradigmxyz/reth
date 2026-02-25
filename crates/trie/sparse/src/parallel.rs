#[cfg(feature = "trie-debug")]
use crate::debug_recorder::{LeafUpdateRecord, ProofTrieNodeRecord, RecordedOp, TrieDebugRecorder};
use crate::{
    lower::LowerSparseSubtrie, provider::TrieNodeProvider, LeafLookup, LeafLookupError,
    RlpNodeStackItem, SparseNode, SparseNodeState, SparseNodeType, SparseTrie, SparseTrieUpdates,
};
use alloc::{borrow::Cow, boxed::Box, vec, vec::Vec};
use alloy_primitives::{
    map::{Entry, HashMap, HashSet},
    B256, U256,
};
use alloy_rlp::Decodable;
use alloy_trie::{BranchNodeCompact, TrieMask, EMPTY_ROOT_HASH};
use core::cmp::{Ord, Ordering, PartialOrd};
use reth_execution_errors::{SparseTrieError, SparseTrieErrorKind, SparseTrieResult};
#[cfg(feature = "metrics")]
use reth_primitives_traits::FastInstant as Instant;
use reth_trie_common::{
    prefix_set::{PrefixSet, PrefixSetMut},
    BranchNodeMasks, BranchNodeMasksMap, BranchNodeRef, ExtensionNodeRef, LeafNodeRef, Nibbles,
    ProofTrieNodeV2, RlpNode, TrieNodeV2,
};
use smallvec::SmallVec;
use tracing::{instrument, trace};

/// The maximum length of a path, in nibbles, which belongs to the upper subtrie of a
/// [`ParallelSparseTrie`]. All longer paths belong to a lower subtrie.
pub const UPPER_TRIE_MAX_DEPTH: usize = 2;

/// Number of lower subtries which are managed by the [`ParallelSparseTrie`].
pub const NUM_LOWER_SUBTRIES: usize = 16usize.pow(UPPER_TRIE_MAX_DEPTH as u32);

/// Configuration for controlling when parallelism is enabled in [`ParallelSparseTrie`] operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ParallelismThresholds {
    /// Minimum number of nodes to reveal before parallel processing is enabled.
    /// When `reveal_nodes` has fewer nodes than this threshold, they will be processed serially.
    pub min_revealed_nodes: usize,
    /// Minimum number of changed keys (prefix set length) before parallel processing is enabled
    /// for hash updates. When updating subtrie hashes with fewer changed keys than this threshold,
    /// the updates will be processed serially.
    pub min_updated_nodes: usize,
}

/// A revealed sparse trie with subtries that can be updated in parallel.
///
/// ## Structure
///
/// The trie is divided into two tiers for efficient parallel processing:
/// - **Upper subtrie**: Contains nodes with paths shorter than [`UPPER_TRIE_MAX_DEPTH`]
/// - **Lower subtries**: An array of [`NUM_LOWER_SUBTRIES`] subtries, each handling nodes with
///   paths of at least [`UPPER_TRIE_MAX_DEPTH`] nibbles
///
/// Node placement is determined by path depth:
/// - Paths with < [`UPPER_TRIE_MAX_DEPTH`] nibbles go to the upper subtrie
/// - Paths with >= [`UPPER_TRIE_MAX_DEPTH`] nibbles go to lower subtries, indexed by their first
///   [`UPPER_TRIE_MAX_DEPTH`] nibbles.
///
/// Each lower subtrie tracks its root via the `path` field, which represents the shortest path
/// in that subtrie. This path will have at least [`UPPER_TRIE_MAX_DEPTH`] nibbles, but may be
/// longer when an extension node in the upper trie "reaches into" the lower subtrie. For example,
/// if the upper trie has an extension from `0x1` to `0x12345`, then the lower subtrie for prefix
/// `0x12` will have its root at path `0x12345` rather than at `0x12`.
///
/// ## Node Revealing
///
/// The trie uses lazy loading to efficiently handle large state tries. Nodes can be:
/// - **Blind nodes**: Stored as hashes on [`SparseNode::Branch::blinded_hashes`]
/// - **Revealed nodes**: Fully loaded nodes (Branch, Extension, Leaf) with complete structure
///
/// Note: An empty trie contains an `EmptyRoot` node at the root path, rather than no nodes at all.
/// A trie with no nodes is blinded, its root may be `EmptyRoot` or some other node type.
///
/// Revealing is generally done using pre-loaded node data provided to via `reveal_nodes`. In
/// certain cases, such as edge-cases when updating/removing leaves, nodes are revealed on-demand.
///
/// ## Leaf Operations
///
/// **Update**: When updating a leaf, the new value is stored in the appropriate subtrie's values
/// map. If the leaf is new, the trie structure is updated by walking to the leaf from the root,
/// creating necessary intermediate branch nodes.
///
/// **Removal**: Leaf removal may require parent node modifications. The algorithm walks up the
/// trie, removing nodes that become empty and converting single-child branches to extensions.
///
/// During leaf operations the overall structure of the trie may change, causing nodes to be moved
/// from the upper to lower trie or vice-versa.
///
/// The `prefix_set` is modified during both leaf updates and removals to track changed leaf paths.
///
/// ## Root Hash Calculation
///
/// Root hash computation follows a bottom-up approach:
/// 1. Update hashes for all modified lower subtries (can be done in parallel)
/// 2. Update hashes for the upper subtrie (which may reference lower subtrie hashes)
/// 3. Calculate the final root hash from the upper subtrie's root node
///
/// The `prefix_set` tracks which paths have been modified, enabling incremental updates instead of
/// recalculating the entire trie.
///
/// ## Invariants
///
/// - Each leaf entry in the `subtries` and `upper_trie` collection must have a corresponding entry
///   in `values` collection. If the root node is a leaf, it must also have an entry in `values`.
/// - All keys in `values` collection are full leaf paths.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ParallelSparseTrie {
    /// This contains the trie nodes for the upper part of the trie.
    upper_subtrie: Box<SparseSubtrie>,
    /// An array containing the subtries at the second level of the trie.
    lower_subtries: Box<[LowerSparseSubtrie; NUM_LOWER_SUBTRIES]>,
    /// Set of prefixes (key paths) that have been marked as updated.
    /// This is used to track which parts of the trie need to be recalculated.
    prefix_set: PrefixSetMut,
    /// Optional tracking of trie updates for later use.
    updates: Option<SparseTrieUpdates>,
    /// Branch node masks containing `tree_mask` and `hash_mask` for each path.
    /// - `tree_mask`: When a bit is set, the corresponding child subtree is stored in the
    ///   database.
    /// - `hash_mask`: When a bit is set, the corresponding child is stored as a hash in the
    ///   database.
    branch_node_masks: BranchNodeMasksMap,
    /// Reusable buffer pool used for collecting [`SparseTrieUpdatesAction`]s during hash
    /// computations.
    update_actions_buffers: Vec<Vec<SparseTrieUpdatesAction>>,
    /// Thresholds controlling when parallelism is enabled for different operations.
    parallelism_thresholds: ParallelismThresholds,
    /// Tracks heat of lower subtries for smart pruning decisions.
    /// Hot subtries are skipped during pruning to keep frequently-used data revealed.
    subtrie_heat: SubtrieModifications,
    /// Metrics for the parallel sparse trie.
    #[cfg(feature = "metrics")]
    metrics: crate::metrics::ParallelSparseTrieMetrics,
    /// Debug recorder for tracking mutating operations.
    #[cfg(feature = "trie-debug")]
    debug_recorder: TrieDebugRecorder,
}

impl Default for ParallelSparseTrie {
    fn default() -> Self {
        Self {
            upper_subtrie: Box::new(SparseSubtrie {
                nodes: HashMap::from_iter([(Nibbles::default(), SparseNode::Empty)]),
                ..Default::default()
            }),
            lower_subtries: Box::new(
                [const { LowerSparseSubtrie::Blind(None) }; NUM_LOWER_SUBTRIES],
            ),
            prefix_set: PrefixSetMut::default(),
            updates: None,
            branch_node_masks: BranchNodeMasksMap::default(),
            update_actions_buffers: Vec::default(),
            parallelism_thresholds: Default::default(),
            subtrie_heat: SubtrieModifications::default(),
            #[cfg(feature = "metrics")]
            metrics: Default::default(),
            #[cfg(feature = "trie-debug")]
            debug_recorder: Default::default(),
        }
    }
}

impl SparseTrie for ParallelSparseTrie {
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

        // A fresh/cleared `ParallelSparseTrie` has a `SparseNode::Empty` at its root in the upper
        // subtrie. Delete that so we can reveal the new root node.
        let path = Nibbles::default();
        let _removed_root = self.upper_subtrie.nodes.remove(&path).expect("root node should exist");
        debug_assert_eq!(_removed_root, SparseNode::Empty);

        self.set_updates(retain_updates);

        if let Some(masks) = masks {
            let branch_path = if let TrieNodeV2::Branch(branch) = &root {
                branch.key
            } else {
                Nibbles::default()
            };

            self.branch_node_masks.insert(branch_path, masks);
        }

        self.reveal_upper_node(Nibbles::default(), &root, masks)
    }

    fn set_updates(&mut self, retain_updates: bool) {
        self.updates = retain_updates.then(Default::default);
    }

    fn reveal_nodes(&mut self, nodes: &mut [ProofTrieNodeV2]) -> SparseTrieResult<()> {
        if nodes.is_empty() {
            return Ok(())
        }

        #[cfg(feature = "trie-debug")]
        self.debug_recorder.record(RecordedOp::RevealNodes {
            nodes: nodes.iter().map(ProofTrieNodeRecord::from_proof_trie_node_v2).collect(),
        });

        // Sort nodes first by their subtrie, and secondarily by their path. This allows for
        // grouping nodes by their subtrie using `chunk_by`.
        nodes.sort_unstable_by(
            |ProofTrieNodeV2 { path: path_a, .. }, ProofTrieNodeV2 { path: path_b, .. }| {
                let subtrie_type_a = SparseSubtrieType::from_path(path_a);
                let subtrie_type_b = SparseSubtrieType::from_path(path_b);
                subtrie_type_a.cmp(&subtrie_type_b).then_with(|| path_a.cmp(path_b))
            },
        );

        // Update the top-level branch node masks. This is simple and can't be done in parallel.
        self.branch_node_masks.reserve(nodes.len());
        for ProofTrieNodeV2 { path, masks, node } in nodes.iter() {
            if let Some(branch_masks) = masks {
                // Use proper path for branch nodes by combining path and extension key.
                let path = if let TrieNodeV2::Branch(branch) = node &&
                    !branch.key.is_empty()
                {
                    let mut path = *path;
                    path.extend(&branch.key);
                    path
                } else {
                    *path
                };
                self.branch_node_masks.insert(path, *branch_masks);
            }
        }

        // Due to the sorting all upper subtrie nodes will be at the front of the slice. We split
        // them off from the rest to be handled specially by
        // `ParallelSparseTrie::reveal_upper_node`.
        let num_upper_nodes = nodes
            .iter()
            .position(|n| !SparseSubtrieType::path_len_is_upper(n.path.len()))
            .unwrap_or(nodes.len());
        let (upper_nodes, lower_nodes) = nodes.split_at(num_upper_nodes);

        // Reserve the capacity of the upper subtrie's `nodes` HashMap before iterating, so we don't
        // end up making many small capacity changes as we loop.
        self.upper_subtrie.nodes.reserve(upper_nodes.len());
        for node in upper_nodes {
            self.reveal_upper_node(node.path, &node.node, node.masks)?;
        }

        let reachable_subtries = self.reachable_subtries();

        // For boundary nodes that are blinded in upper subtrie, unset the blinded bit and remember
        // the hash to pass into `reveal_node`.
        let hashes_from_upper = nodes
            .iter()
            .filter_map(|node| {
                if node.path.len() == UPPER_TRIE_MAX_DEPTH &&
                    reachable_subtries.get(path_subtrie_index_unchecked(&node.path)) &&
                    let SparseNode::Branch { blinded_mask, blinded_hashes, .. } = self
                        .upper_subtrie
                        .nodes
                        .get_mut(&node.path.slice(0..UPPER_TRIE_MAX_DEPTH - 1))
                        .unwrap()
                {
                    let nibble = node.path.last().unwrap();
                    blinded_mask.is_bit_set(nibble).then(|| {
                        blinded_mask.unset_bit(nibble);
                        (node.path, blinded_hashes[nibble as usize])
                    })
                } else {
                    None
                }
            })
            .collect::<HashMap<_, _>>();

        if !self.is_reveal_parallelism_enabled(lower_nodes.len()) {
            for node in lower_nodes {
                let idx = path_subtrie_index_unchecked(&node.path);
                if !reachable_subtries.get(idx) {
                    trace!(
                        target: "trie::parallel_sparse",
                        reveal_path = ?node.path,
                        "Node's lower subtrie is not reachable, skipping",
                    );
                    continue;
                }
                // For boundary leaves, check reachability from upper subtrie's parent branch
                if node.path.len() == UPPER_TRIE_MAX_DEPTH &&
                    !Self::is_boundary_leaf_reachable(
                        &self.upper_subtrie.nodes,
                        &node.path,
                        &node.node,
                    )
                {
                    trace!(
                        target: "trie::parallel_sparse",
                        path = ?node.path,
                        "Boundary leaf not reachable from upper subtrie, skipping",
                    );
                    continue;
                }
                self.lower_subtries[idx].reveal(&node.path);
                self.subtrie_heat.mark_modified(idx);
                self.lower_subtries[idx].as_revealed_mut().expect("just revealed").reveal_node(
                    node.path,
                    &node.node,
                    node.masks,
                    hashes_from_upper.get(&node.path).copied(),
                )?;
            }
            return Ok(())
        }

        #[cfg(not(feature = "std"))]
        unreachable!("nostd is checked by is_reveal_parallelism_enabled");

        #[cfg(feature = "std")]
        // Reveal lower subtrie nodes in parallel
        {
            use rayon::iter::{IntoParallelIterator, ParallelIterator};
            use tracing::Span;

            // Capture the current span so it can be propagated to rayon worker threads
            let parent_span = Span::current();

            // Capture reference to upper subtrie nodes for boundary leaf reachability checks
            let upper_nodes = &self.upper_subtrie.nodes;

            // Group the nodes by lower subtrie.
            let results = lower_nodes
                .chunk_by(|node_a, node_b| {
                    SparseSubtrieType::from_path(&node_a.path) ==
                        SparseSubtrieType::from_path(&node_b.path)
                })
                // Filter out chunks for unreachable subtries.
                .filter_map(|nodes| {
                    let mut nodes = nodes
                        .iter()
                        .filter(|node| {
                            // For boundary leaves, check reachability from upper subtrie's parent
                            // branch.
                            if node.path.len() == UPPER_TRIE_MAX_DEPTH &&
                                !Self::is_boundary_leaf_reachable(
                                    upper_nodes,
                                    &node.path,
                                    &node.node,
                                )
                            {
                                trace!(
                                    target: "trie::parallel_sparse",
                                    path = ?node.path,
                                    "Boundary leaf not reachable from upper subtrie, skipping",
                                );
                                false
                            } else {
                                true
                            }
                        })
                        .peekable();

                    let node = nodes.peek()?;
                    let idx =
                        SparseSubtrieType::from_path(&node.path).lower_index().unwrap_or_else(
                            || panic!("upper subtrie node {node:?} found amongst lower nodes"),
                        );

                    if !reachable_subtries.get(idx) {
                        trace!(
                            target: "trie::parallel_sparse",
                            nodes = ?nodes,
                            "Lower subtrie is not reachable, skipping reveal",
                        );
                        return None;
                    }

                    // due to the nodes being sorted secondarily on their path, and chunk_by keeping
                    // the first element of each group, the `path` here will necessarily be the
                    // shortest path being revealed for each subtrie. Therefore we can reveal the
                    // subtrie itself using this path and retain correct behavior.
                    self.lower_subtries[idx].reveal(&node.path);
                    Some((
                        idx,
                        self.lower_subtries[idx].take_revealed().expect("just revealed"),
                        nodes,
                    ))
                })
                .collect::<Vec<_>>()
                .into_par_iter()
                .map(|(subtrie_idx, mut subtrie, nodes)| {
                    // Enter the parent span to propagate context (e.g., hashed_address for storage
                    // tries) to the worker thread
                    let _guard = parent_span.enter();

                    // reserve space in the HashMap ahead of time; doing it on a node-by-node basis
                    // can cause multiple re-allocations as the hashmap grows.
                    subtrie.nodes.reserve(nodes.size_hint().1.unwrap_or(0));

                    for node in nodes {
                        // Reveal each node in the subtrie, returning early on any errors
                        let res = subtrie.reveal_node(
                            node.path,
                            &node.node,
                            node.masks,
                            hashes_from_upper.get(&node.path).copied(),
                        );
                        if res.is_err() {
                            return (subtrie_idx, subtrie, res.map(|_| ()))
                        }
                    }
                    (subtrie_idx, subtrie, Ok(()))
                })
                .collect::<Vec<_>>();

            // Put subtries back which were processed in the rayon pool, collecting the last
            // seen error in the process and returning that.
            let mut any_err = Ok(());
            for (subtrie_idx, subtrie, res) in results {
                self.lower_subtries[subtrie_idx] = LowerSparseSubtrie::Revealed(subtrie);
                if res.is_err() {
                    any_err = res;
                }
            }

            any_err
        }
    }

    fn update_leaf<P: TrieNodeProvider>(
        &mut self,
        full_path: Nibbles,
        value: Vec<u8>,
        _provider: P,
    ) -> SparseTrieResult<()> {
        debug_assert_eq!(
            full_path.len(),
            B256::len_bytes() * 2,
            "update_leaf full_path must be 64 nibbles (32 bytes), got {} nibbles",
            full_path.len()
        );

        trace!(
            target: "trie::parallel_sparse",
            ?full_path,
            value_len = value.len(),
            "Updating leaf",
        );

        // Check if the value already exists - if so, just update it (no structural changes needed)
        if self.upper_subtrie.inner.values.contains_key(&full_path) {
            self.prefix_set.insert(full_path);
            self.upper_subtrie.inner.values.insert(full_path, value);
            return Ok(());
        }
        // Also check lower subtries for existing value
        if let Some(subtrie) = self.lower_subtrie_for_path(&full_path) &&
            subtrie.inner.values.contains_key(&full_path)
        {
            self.prefix_set.insert(full_path);
            self.lower_subtrie_for_path_mut(&full_path)
                .expect("subtrie exists")
                .inner
                .values
                .insert(full_path, value);
            return Ok(());
        }

        // Insert value into upper subtrie temporarily. We'll move it to the correct subtrie
        // during traversal, or clean it up if we error.
        self.upper_subtrie.inner.values.insert(full_path, value.clone());

        // Start at the root, traversing until we find either the node to update or a subtrie to
        // update.
        //
        // We first traverse the upper subtrie for two levels, and moving any created nodes to a
        // lower subtrie if necessary.
        //
        // We use `next` to keep track of the next node that we need to traverse to, and
        // `new_nodes` to keep track of any nodes that were created during the traversal.
        let mut new_nodes = Vec::new();
        let mut next = Some(Nibbles::default());

        // Traverse the upper subtrie to find the node to update or the subtrie to update.
        //
        // We stop when the next node to traverse would be in a lower subtrie, or if there are no
        // more nodes to traverse.
        while let Some(current) =
            next.as_mut().filter(|next| SparseSubtrieType::path_len_is_upper(next.len()))
        {
            // Traverse the next node, keeping track of any changed nodes and the next step in the
            // trie. If traversal fails, clean up the value we inserted and propagate the error.
            let step_result = self.upper_subtrie.update_next_node(current, &full_path);

            if step_result.is_err() {
                self.upper_subtrie.inner.values.remove(&full_path);
                return step_result.map(|_| ());
            }

            match step_result? {
                LeafUpdateStep::Continue => {}
                LeafUpdateStep::Complete { inserted_nodes } => {
                    new_nodes.extend(inserted_nodes);
                    next = None;
                }
                LeafUpdateStep::NodeNotFound => {
                    next = None;
                }
            }
        }

        // Move nodes from upper subtrie to lower subtries
        for node_path in &new_nodes {
            // Skip nodes that belong in the upper subtrie
            if SparseSubtrieType::path_len_is_upper(node_path.len()) {
                continue
            }

            let node =
                self.upper_subtrie.nodes.remove(node_path).expect("node belongs to upper subtrie");

            // If it's a leaf node, extract its value before getting mutable reference to subtrie.
            let leaf_value = if let SparseNode::Leaf { key, .. } = &node {
                let mut leaf_full_path = *node_path;
                leaf_full_path.extend(key);
                Some((
                    leaf_full_path,
                    self.upper_subtrie
                        .inner
                        .values
                        .remove(&leaf_full_path)
                        .expect("leaf nodes have associated values entries"),
                ))
            } else {
                None
            };

            // Get or create the subtrie with the exact node path (not truncated to 2 nibbles).
            let subtrie = self.subtrie_for_path_mut(node_path);

            // Insert the leaf value if we have one
            if let Some((leaf_full_path, value)) = leaf_value {
                subtrie.inner.values.insert(leaf_full_path, value);
            }

            // Insert the node into the lower subtrie
            subtrie.nodes.insert(*node_path, node);
        }

        // If we reached the max depth of the upper trie, we may have had more nodes to insert.
        if let Some(next_path) = next.filter(|n| !SparseSubtrieType::path_len_is_upper(n.len())) {
            // The value was inserted into the upper subtrie's `values` at the top of this method.
            // At this point we know the value is not in the upper subtrie, and the call to
            // `update_leaf` below will insert it into the lower subtrie. So remove it from the
            // upper subtrie.
            self.upper_subtrie.inner.values.remove(&full_path);

            // Use subtrie_for_path to ensure the subtrie has the correct path.
            //
            // The next_path here represents where we need to continue traversal, which may
            // be longer than 2 nibbles if we're following an extension node.
            let subtrie = self.subtrie_for_path_mut(&next_path);

            // Create an empty root at the subtrie path if the subtrie is empty
            if subtrie.nodes.is_empty() {
                subtrie.nodes.insert(subtrie.path, SparseNode::Empty);
            }

            // If we didn't update the target leaf, we need to call update_leaf on the subtrie
            // to ensure that the leaf is updated correctly.
            if let Err(e) = subtrie.update_leaf(full_path, value) {
                // Clean up: remove the value from lower subtrie if it was inserted
                if let Some(lower) = self.lower_subtrie_for_path_mut(&full_path) {
                    lower.inner.values.remove(&full_path);
                }
                return Err(e);
            }
        }

        // Insert into prefix_set only after all operations succeed
        self.prefix_set.insert(full_path);

        Ok(())
    }

    fn remove_leaf<P: TrieNodeProvider>(
        &mut self,
        full_path: &Nibbles,
        _provider: P,
    ) -> SparseTrieResult<()> {
        debug_assert_eq!(
            full_path.len(),
            B256::len_bytes() * 2,
            "remove_leaf full_path must be 64 nibbles (32 bytes), got {} nibbles",
            full_path.len()
        );

        trace!(
            target: "trie::parallel_sparse",
            ?full_path,
            "Removing leaf",
        );

        // When removing a leaf node it's possibly necessary to modify its parent node, and possibly
        // the parent's parent node. It is not ever necessary to descend further than that; once an
        // extension node is hit it must terminate in a branch or the root, which won't need further
        // updates. So the situation with maximum updates is:
        //
        // - Leaf
        // - Branch with 2 children, one being this leaf
        // - Extension
        //
        // ...which will result in just a leaf or extension, depending on what the branch's other
        // child is.
        //
        // Therefore, first traverse the trie in order to find the leaf node and at most its parent
        // and grandparent.

        let leaf_path;
        let leaf_subtrie_type;

        let mut branch_parent_path: Option<Nibbles> = None;
        let mut branch_parent_node: Option<SparseNode> = None;

        let mut ext_grandparent_path: Option<Nibbles> = None;
        let mut ext_grandparent_node: Option<SparseNode> = None;

        let mut curr_path = Nibbles::new(); // start traversal from root
        let mut curr_subtrie_type = SparseSubtrieType::Upper;

        // List of node paths which need to be marked dirty
        let mut paths_to_mark_dirty = Vec::new();

        loop {
            let curr_subtrie = match curr_subtrie_type {
                SparseSubtrieType::Upper => &mut self.upper_subtrie,
                SparseSubtrieType::Lower(idx) => {
                    self.lower_subtries[idx].as_revealed_mut().expect("lower subtrie is revealed")
                }
            };
            let curr_node = curr_subtrie.nodes.get_mut(&curr_path).unwrap();

            match Self::find_next_to_leaf(&curr_path, curr_node, full_path) {
                FindNextToLeafOutcome::NotFound => return Ok(()), // leaf isn't in the trie
                FindNextToLeafOutcome::BlindedNode(path) => {
                    return Err(SparseTrieErrorKind::BlindedNode(path).into())
                }
                FindNextToLeafOutcome::Found => {
                    // this node is the target leaf
                    leaf_path = curr_path;
                    leaf_subtrie_type = curr_subtrie_type;
                    break;
                }
                FindNextToLeafOutcome::ContinueFrom(next_path) => {
                    // Any branches/extensions along the path to the leaf will have their `hash`
                    // field unset, as it will no longer be valid once the leaf is removed.
                    match curr_node {
                        SparseNode::Branch { .. } => {
                            paths_to_mark_dirty
                                .push((SparseSubtrieType::from_path(&curr_path), curr_path));

                            // If there is already an extension leading into a branch, then that
                            // extension is no longer relevant.
                            match (&branch_parent_path, &ext_grandparent_path) {
                                (Some(branch), Some(ext)) if branch.len() > ext.len() => {
                                    ext_grandparent_path = None;
                                    ext_grandparent_node = None;
                                }
                                _ => (),
                            };
                            branch_parent_path = Some(curr_path);
                            branch_parent_node = Some(curr_node.clone());
                        }
                        SparseNode::Extension { .. } => {
                            paths_to_mark_dirty
                                .push((SparseSubtrieType::from_path(&curr_path), curr_path));

                            // We can assume a new branch node will be found after the extension, so
                            // there's no need to modify branch_parent_path/node even if it's
                            // already set.
                            ext_grandparent_path = Some(curr_path);
                            ext_grandparent_node = Some(curr_node.clone());
                        }
                        SparseNode::Empty | SparseNode::Leaf { .. } => {
                            unreachable!(
                                "find_next_to_leaf only continues to a branch or extension"
                            )
                        }
                    }

                    curr_path = next_path;

                    // Update subtrie type if we're crossing into the lower trie.
                    let next_subtrie_type = SparseSubtrieType::from_path(&curr_path);
                    if matches!(curr_subtrie_type, SparseSubtrieType::Upper) &&
                        matches!(next_subtrie_type, SparseSubtrieType::Lower(_))
                    {
                        curr_subtrie_type = next_subtrie_type;
                    }
                }
            };
        }

        // Before mutating, check if branch collapse would require revealing a blinded node.
        // This ensures remove_leaf is atomic: if it errors, the trie is unchanged.
        if let (Some(branch_path), Some(SparseNode::Branch { state_mask, blinded_mask, .. })) =
            (&branch_parent_path, &branch_parent_node)
        {
            let mut check_mask = *state_mask;
            let child_nibble = leaf_path.get_unchecked(branch_path.len());
            check_mask.unset_bit(child_nibble);

            if check_mask.count_bits() == 1 {
                let remaining_nibble =
                    check_mask.first_set_bit_index().expect("state mask is not empty");

                if blinded_mask.is_bit_set(remaining_nibble) {
                    let mut path = *branch_path;
                    path.push_unchecked(remaining_nibble);
                    return Err(SparseTrieErrorKind::BlindedNode(path).into());
                }
            }
        }

        // We've traversed to the leaf and collected its ancestors as necessary. Remove the leaf
        // from its SparseSubtrie and reset the hashes of the nodes along the path.
        self.prefix_set.insert(*full_path);
        let leaf_subtrie = match leaf_subtrie_type {
            SparseSubtrieType::Upper => &mut self.upper_subtrie,
            SparseSubtrieType::Lower(idx) => {
                self.lower_subtries[idx].as_revealed_mut().expect("lower subtrie is revealed")
            }
        };
        leaf_subtrie.inner.values.remove(full_path);
        for (subtrie_type, path) in paths_to_mark_dirty {
            let node = match subtrie_type {
                SparseSubtrieType::Upper => self.upper_subtrie.nodes.get_mut(&path),
                SparseSubtrieType::Lower(idx) => self.lower_subtries[idx]
                    .as_revealed_mut()
                    .expect("lower subtrie is revealed")
                    .nodes
                    .get_mut(&path),
            }
            .expect("node exists");

            match node {
                SparseNode::Extension { state, .. } | SparseNode::Branch { state, .. } => {
                    *state = SparseNodeState::Dirty
                }
                SparseNode::Empty | SparseNode::Leaf { .. } => {
                    unreachable!(
                        "only branch and extension nodes can be marked dirty when removing a leaf"
                    )
                }
            }
        }
        self.remove_node(&leaf_path);

        // If the leaf was at the root replace its node with the empty value. We can stop execution
        // here, all remaining logic is related to the ancestors of the leaf.
        if leaf_path.is_empty() {
            self.upper_subtrie.nodes.insert(leaf_path, SparseNode::Empty);
            return Ok(())
        }

        // If there is a parent branch node (very likely, unless the leaf is at the root) execute
        // any required changes for that node, relative to the removed leaf.
        if let (
            Some(branch_path),
            &Some(SparseNode::Branch { mut state_mask, blinded_mask, ref blinded_hashes, .. }),
        ) = (&branch_parent_path, &branch_parent_node)
        {
            let child_nibble = leaf_path.get_unchecked(branch_path.len());
            state_mask.unset_bit(child_nibble);

            let new_branch_node = if state_mask.count_bits() == 1 {
                // If only one child is left set in the branch node, we need to collapse it. Get
                // full path of the only child node left.
                let remaining_child_nibble =
                    state_mask.first_set_bit_index().expect("state mask is not empty");
                let mut remaining_child_path = *branch_path;
                remaining_child_path.push_unchecked(remaining_child_nibble);

                trace!(
                    target: "trie::parallel_sparse",
                    ?leaf_path,
                    ?branch_path,
                    ?remaining_child_path,
                    "Branch node has only one child",
                );

                // If the remaining child node is not yet revealed then we have to reveal it here,
                // otherwise it's not possible to know how to collapse the branch.
                if blinded_mask.is_bit_set(remaining_child_nibble) {
                    return Err(SparseTrieErrorKind::BlindedNode(remaining_child_path).into());
                }

                let remaining_child_node = self
                    .subtrie_for_path_mut(&remaining_child_path)
                    .nodes
                    .get(&remaining_child_path)
                    .unwrap();

                let (new_branch_node, remove_child) = Self::branch_changes_on_leaf_removal(
                    branch_path,
                    &remaining_child_path,
                    remaining_child_node,
                );

                if remove_child {
                    self.move_value_on_leaf_removal(
                        branch_path,
                        &new_branch_node,
                        &remaining_child_path,
                    );
                    self.remove_node(&remaining_child_path);
                }

                if let Some(updates) = self.updates.as_mut() {
                    updates.updated_nodes.remove(branch_path);
                    updates.removed_nodes.insert(*branch_path);
                }

                new_branch_node
            } else {
                // If more than one child is left set in the branch, we just re-insert it with the
                // updated state_mask.
                SparseNode::Branch {
                    state_mask,
                    blinded_mask,
                    blinded_hashes: blinded_hashes.clone(),
                    state: SparseNodeState::Dirty,
                }
            };

            let branch_subtrie = self.subtrie_for_path_mut(branch_path);
            branch_subtrie.nodes.insert(*branch_path, new_branch_node.clone());
            branch_parent_node = Some(new_branch_node);
        };

        // If there is a grandparent extension node then there will necessarily be a parent branch
        // node. Execute any required changes for the extension node, relative to the (possibly now
        // replaced with a leaf or extension) branch node.
        if let (Some(ext_path), Some(SparseNode::Extension { key: shortkey, .. })) =
            (ext_grandparent_path, &ext_grandparent_node)
        {
            let ext_subtrie = self.subtrie_for_path_mut(&ext_path);
            let branch_path = branch_parent_path.as_ref().unwrap();

            if let Some(new_ext_node) = Self::extension_changes_on_leaf_removal(
                &ext_path,
                shortkey,
                branch_path,
                branch_parent_node.as_ref().unwrap(),
            ) {
                ext_subtrie.nodes.insert(ext_path, new_ext_node.clone());
                self.move_value_on_leaf_removal(&ext_path, &new_ext_node, branch_path);
                self.remove_node(branch_path);
            }
        }

        Ok(())
    }

    #[instrument(level = "trace", target = "trie::sparse::parallel", skip(self))]
    fn root(&mut self) -> B256 {
        trace!(target: "trie::parallel_sparse", "Calculating trie root hash");

        #[cfg(feature = "trie-debug")]
        self.debug_recorder.record(RecordedOp::Root);

        if self.prefix_set.is_empty() &&
            let Some(rlp_node) = self
                .upper_subtrie
                .nodes
                .get(&Nibbles::default())
                .and_then(|node| node.cached_rlp_node())
        {
            return rlp_node
                .as_hash()
                .expect("RLP-encoding of the root node cannot be less than 32 bytes")
        }

        // Update all lower subtrie hashes
        self.update_subtrie_hashes();

        // Update hashes for the upper subtrie using our specialized function
        // that can access both upper and lower subtrie nodes
        let mut prefix_set = core::mem::take(&mut self.prefix_set).freeze();
        let root_rlp = self.update_upper_subtrie_hashes(&mut prefix_set);

        // Return the root hash
        root_rlp.as_hash().unwrap_or(EMPTY_ROOT_HASH)
    }

    fn is_root_cached(&self) -> bool {
        self.prefix_set.is_empty() &&
            self.upper_subtrie
                .nodes
                .get(&Nibbles::default())
                .is_some_and(|node| node.cached_rlp_node().is_some())
    }

    #[instrument(level = "trace", target = "trie::sparse::parallel", skip(self))]
    fn update_subtrie_hashes(&mut self) {
        trace!(target: "trie::parallel_sparse", "Updating subtrie hashes");

        #[cfg(feature = "trie-debug")]
        self.debug_recorder.record(RecordedOp::UpdateSubtrieHashes);

        // Take changed subtries according to the prefix set
        let mut prefix_set = core::mem::take(&mut self.prefix_set).freeze();
        let num_changed_keys = prefix_set.len();
        let (mut changed_subtries, unchanged_prefix_set) =
            self.take_changed_lower_subtries(&mut prefix_set);

        // update metrics
        #[cfg(feature = "metrics")]
        self.metrics.subtries_updated.record(changed_subtries.len() as f64);

        // Update the prefix set with the keys that didn't have matching subtries
        self.prefix_set = unchanged_prefix_set;

        // Update subtrie hashes serially parallelism is not enabled
        if !self.is_update_parallelism_enabled(num_changed_keys) {
            for changed_subtrie in &mut changed_subtries {
                changed_subtrie.subtrie.update_hashes(
                    &mut changed_subtrie.prefix_set,
                    &mut changed_subtrie.update_actions_buf,
                    &self.branch_node_masks,
                );
            }

            self.insert_changed_subtries(changed_subtries);
            return
        }

        #[cfg(not(feature = "std"))]
        unreachable!("nostd is checked by is_update_parallelism_enabled");

        #[cfg(feature = "std")]
        // Update subtrie hashes in parallel
        {
            use rayon::prelude::*;

            changed_subtries.par_iter_mut().for_each(|changed_subtrie| {
                #[cfg(feature = "metrics")]
                let start = Instant::now();
                changed_subtrie.subtrie.update_hashes(
                    &mut changed_subtrie.prefix_set,
                    &mut changed_subtrie.update_actions_buf,
                    &self.branch_node_masks,
                );
                #[cfg(feature = "metrics")]
                self.metrics.subtrie_hash_update_latency.record(start.elapsed());
            });

            self.insert_changed_subtries(changed_subtries);
        }
    }

    fn get_leaf_value(&self, full_path: &Nibbles) -> Option<&Vec<u8>> {
        // `subtrie_for_path` is intended for a node path, but here we are using a full key path. So
        // we need to check if the subtrie that the key might belong to has any nodes; if not then
        // the key's portion of the trie doesn't have enough depth to reach into the subtrie, and
        // the key will be in the upper subtrie
        if let Some(subtrie) = self.subtrie_for_path(full_path) &&
            !subtrie.is_empty()
        {
            return subtrie.inner.values.get(full_path);
        }

        self.upper_subtrie.inner.values.get(full_path)
    }

    fn updates_ref(&self) -> Cow<'_, SparseTrieUpdates> {
        self.updates.as_ref().map_or(Cow::Owned(SparseTrieUpdates::default()), Cow::Borrowed)
    }

    fn take_updates(&mut self) -> SparseTrieUpdates {
        match self.updates.take() {
            Some(updates) => {
                // NOTE: we need to preserve Some case
                self.updates = Some(SparseTrieUpdates::with_capacity(
                    updates.updated_nodes.len(),
                    updates.removed_nodes.len(),
                ));
                updates
            }
            None => SparseTrieUpdates::default(),
        }
    }

    fn wipe(&mut self) {
        self.upper_subtrie.wipe();
        for trie in &mut *self.lower_subtries {
            trie.wipe();
        }
        self.prefix_set = PrefixSetMut::all();
        self.updates = self.updates.is_some().then(SparseTrieUpdates::wiped);
        self.subtrie_heat.clear();
    }

    fn clear(&mut self) {
        self.upper_subtrie.clear();
        self.upper_subtrie.nodes.insert(Nibbles::default(), SparseNode::Empty);
        for subtrie in &mut *self.lower_subtries {
            subtrie.clear();
        }
        self.prefix_set.clear();
        self.updates = None;
        self.branch_node_masks.clear();
        self.subtrie_heat.clear();
        #[cfg(feature = "trie-debug")]
        self.debug_recorder.reset();
        // `update_actions_buffers` doesn't need to be cleared; we want to reuse the Vecs it has
        // buffered, and all of those are already inherently cleared when they get used.
    }

    fn find_leaf(
        &self,
        full_path: &Nibbles,
        expected_value: Option<&Vec<u8>>,
    ) -> Result<LeafLookup, LeafLookupError> {
        // Inclusion proof
        //
        // First, do a quick check if the value exists in either the upper or lower subtrie's values
        // map. We assume that if there exists a leaf node, then its value will be in the `values`
        // map.
        if let Some(actual_value) = core::iter::once(self.upper_subtrie.as_ref())
            .chain(self.lower_subtrie_for_path(full_path))
            .filter_map(|subtrie| subtrie.inner.values.get(full_path))
            .next()
        {
            // We found the leaf, check if the value matches (if expected value was provided)
            return expected_value
                .is_none_or(|v| v == actual_value)
                .then_some(LeafLookup::Exists)
                .ok_or_else(|| LeafLookupError::ValueMismatch {
                    path: *full_path,
                    expected: expected_value.cloned(),
                    actual: actual_value.clone(),
                })
        }

        // If the value does not exist in the `values` map, then this means that the leaf either:
        // - Does not exist in the trie
        // - Is missing from the witness
        // We traverse the trie to find the location where this leaf would have been, showing
        // that it is not in the trie. Or we find a blinded node, showing that the witness is
        // not complete.
        let mut curr_path = Nibbles::new(); // start traversal from root
        let mut curr_subtrie = self.upper_subtrie.as_ref();
        let mut curr_subtrie_is_upper = true;

        loop {
            match curr_subtrie.nodes.get(&curr_path).unwrap() {
                SparseNode::Empty => return Ok(LeafLookup::NonExistent),
                SparseNode::Leaf { key, .. } => {
                    let mut found_full_path = curr_path;
                    found_full_path.extend(key);
                    assert!(&found_full_path != full_path, "target leaf {full_path:?} found, even though value wasn't in values hashmap");
                    return Ok(LeafLookup::NonExistent)
                }
                SparseNode::Extension { key, .. } => {
                    if full_path.len() == curr_path.len() {
                        return Ok(LeafLookup::NonExistent)
                    }
                    curr_path.extend(key);
                    if !full_path.starts_with(&curr_path) {
                        return Ok(LeafLookup::NonExistent)
                    }
                }
                SparseNode::Branch { state_mask, blinded_mask, blinded_hashes, .. } => {
                    if full_path.len() == curr_path.len() {
                        return Ok(LeafLookup::NonExistent)
                    }
                    let nibble = full_path.get_unchecked(curr_path.len());
                    if !state_mask.is_bit_set(nibble) {
                        return Ok(LeafLookup::NonExistent)
                    }
                    curr_path.push_unchecked(nibble);
                    if blinded_mask.is_bit_set(nibble) {
                        return Err(LeafLookupError::BlindedNode {
                            path: curr_path,
                            hash: blinded_hashes[nibble as usize],
                        })
                    }
                }
            }

            // If we were previously looking at the upper trie, and the new path is in the
            // lower trie, we need to pull out a ref to the lower trie.
            if curr_subtrie_is_upper &&
                let Some(lower_subtrie) = self.lower_subtrie_for_path(&curr_path)
            {
                curr_subtrie = lower_subtrie;
                curr_subtrie_is_upper = false;
            }
        }
    }

    fn shrink_nodes_to(&mut self, size: usize) {
        // Distribute the capacity across upper and lower subtries
        //
        // Always include upper subtrie, plus any lower subtries
        let total_subtries = 1 + NUM_LOWER_SUBTRIES;
        let size_per_subtrie = size / total_subtries;

        // Shrink the upper subtrie
        self.upper_subtrie.shrink_nodes_to(size_per_subtrie);

        // Shrink lower subtries (works for both revealed and blind with allocation)
        for subtrie in &mut *self.lower_subtries {
            subtrie.shrink_nodes_to(size_per_subtrie);
        }

        // shrink masks map
        self.branch_node_masks.shrink_to(size);
    }

    fn shrink_values_to(&mut self, size: usize) {
        // Distribute the capacity across upper and lower subtries
        //
        // Always include upper subtrie, plus any lower subtries
        let total_subtries = 1 + NUM_LOWER_SUBTRIES;
        let size_per_subtrie = size / total_subtries;

        // Shrink the upper subtrie
        self.upper_subtrie.shrink_values_to(size_per_subtrie);

        // Shrink lower subtries (works for both revealed and blind with allocation)
        for subtrie in &mut *self.lower_subtries {
            subtrie.shrink_values_to(size_per_subtrie);
        }
    }

    /// O(1) size hint based on total node count (including hash stubs).
    fn size_hint(&self) -> usize {
        let upper_count = self.upper_subtrie.nodes.len();
        let lower_count: usize = self
            .lower_subtries
            .iter()
            .filter_map(|s| s.as_revealed_ref())
            .map(|s| s.nodes.len())
            .sum();
        upper_count + lower_count
    }

    fn prune(&mut self, max_depth: usize) -> usize {
        #[cfg(feature = "trie-debug")]
        self.debug_recorder.reset();

        // Decay heat for subtries not modified this cycle
        self.subtrie_heat.decay_and_reset();

        // DFS traversal to find nodes at max_depth that can be pruned.
        // Collects "effective pruned roots" - children of nodes at max_depth with computed hashes.
        // We replace nodes with Hash stubs inline during traversal.
        let mut effective_pruned_roots = Vec::<Nibbles>::new();
        let mut stack: SmallVec<[(Nibbles, usize); 32]> = SmallVec::new();
        stack.push((Nibbles::default(), 0));

        // DFS traversal: pop path and depth, skip if subtrie or node not found.
        while let Some((path, depth)) = stack.pop() {
            // Skip traversal into hot lower subtries beyond max_depth.
            // At max_depth, we still need to process the node to convert children to hashes.
            // This keeps frequently-modified subtries revealed to avoid expensive re-reveals.
            if depth > max_depth &&
                let SparseSubtrieType::Lower(idx) = SparseSubtrieType::from_path(&path) &&
                self.subtrie_heat.is_hot(idx)
            {
                continue;
            }

            let Some(subtrie) = self.subtrie_for_path_mut_untracked(&path) else { continue };
            let Some(node) = subtrie.nodes.get_mut(&path) else { continue };

            match node {
                SparseNode::Empty | SparseNode::Leaf { .. } => {}
                SparseNode::Extension { key, state, .. } => {
                    // For extension nodes at max depth, collapse both extension and its child
                    // branch to preserve invariant of all extension nodes children being revealed.
                    if depth == max_depth {
                        let Some(hash) = state.cached_hash() else { continue };
                        subtrie.nodes.remove(&path);

                        let parent_path = path.slice(0..path.len() - 1);
                        let SparseNode::Branch { blinded_mask, blinded_hashes, .. } =
                            subtrie.nodes.get_mut(&parent_path).unwrap()
                        else {
                            panic!("expected branch node at path {parent_path:?}");
                        };

                        let nibble = path.last().unwrap();
                        blinded_mask.set_bit(nibble);
                        blinded_hashes[nibble as usize] = hash;

                        effective_pruned_roots.push(path);
                    } else {
                        let mut child = path;
                        child.extend(key);
                        stack.push((child, depth + 1));
                    }
                }
                SparseNode::Branch { state_mask, blinded_mask, blinded_hashes, .. } => {
                    // For branch nodes at max depth, collapse all children onto them,
                    if depth == max_depth {
                        let mut blinded_mask = *blinded_mask;
                        let mut blinded_hashes = blinded_hashes.clone();
                        for nibble in state_mask.iter() {
                            if blinded_mask.is_bit_set(nibble) {
                                continue;
                            }
                            let mut child = path;
                            child.push_unchecked(nibble);

                            let Entry::Occupied(entry) = self
                                .subtrie_for_path_mut_untracked(&child)
                                .unwrap()
                                .nodes
                                .entry(child)
                            else {
                                panic!("expected node at path {child:?}");
                            };

                            let Some(hash) = entry.get().cached_hash() else {
                                continue;
                            };
                            entry.remove();
                            blinded_mask.set_bit(nibble);
                            blinded_hashes[nibble as usize] = hash;
                            effective_pruned_roots.push(child);
                        }

                        let SparseNode::Branch {
                            blinded_mask: old_blinded_mask,
                            blinded_hashes: old_blinded_hashes,
                            ..
                        } = self
                            .subtrie_for_path_mut_untracked(&path)
                            .unwrap()
                            .nodes
                            .get_mut(&path)
                            .unwrap()
                        else {
                            unreachable!("expected branch node at path {path:?}");
                        };
                        *old_blinded_mask = blinded_mask;
                        *old_blinded_hashes = blinded_hashes;
                    } else {
                        for nibble in state_mask.iter() {
                            if blinded_mask.is_bit_set(nibble) {
                                continue;
                            }
                            let mut child = path;
                            child.push_unchecked(nibble);
                            stack.push((child, depth + 1));
                        }
                    }
                }
            }
        }

        if effective_pruned_roots.is_empty() {
            return 0;
        }

        let nodes_converted = effective_pruned_roots.len();

        // Sort roots by subtrie type (upper first), then by path for efficient partitioning.
        effective_pruned_roots.sort_unstable_by(|path_a, path_b| {
            let subtrie_type_a = SparseSubtrieType::from_path(path_a);
            let subtrie_type_b = SparseSubtrieType::from_path(path_b);
            subtrie_type_a.cmp(&subtrie_type_b).then(path_a.cmp(path_b))
        });

        // Split off upper subtrie roots (they come first due to sorting)
        let num_upper_roots = effective_pruned_roots
            .iter()
            .position(|p| !SparseSubtrieType::path_len_is_upper(p.len()))
            .unwrap_or(effective_pruned_roots.len());

        let roots_upper = &effective_pruned_roots[..num_upper_roots];
        let roots_lower = &effective_pruned_roots[num_upper_roots..];

        debug_assert!(
            {
                let mut all_roots: Vec<_> = effective_pruned_roots.clone();
                all_roots.sort_unstable();
                all_roots.windows(2).all(|w| !w[1].starts_with(&w[0]))
            },
            "prune roots must be prefix-free"
        );

        // Upper prune roots that are prefixes of lower subtrie root paths cause the entire
        // subtrie to be cleared (preserving allocations for reuse).
        if !roots_upper.is_empty() {
            for subtrie in &mut *self.lower_subtries {
                let should_clear = subtrie.as_revealed_ref().is_some_and(|s| {
                    let search_idx = roots_upper.partition_point(|root| root <= &s.path);
                    search_idx > 0 && s.path.starts_with(&roots_upper[search_idx - 1])
                });
                if should_clear {
                    subtrie.clear();
                }
            }
        }

        // Upper subtrie: prune nodes and values
        self.upper_subtrie.nodes.retain(|p, _| !is_strict_descendant_in(roots_upper, p));
        self.upper_subtrie.inner.values.retain(|p, _| {
            !starts_with_pruned_in(roots_upper, p) && !starts_with_pruned_in(roots_lower, p)
        });

        // Process lower subtries using chunk_by to group roots by subtrie
        for roots_group in roots_lower.chunk_by(|path_a, path_b| {
            SparseSubtrieType::from_path(path_a) == SparseSubtrieType::from_path(path_b)
        }) {
            let subtrie_idx = path_subtrie_index_unchecked(&roots_group[0]);

            // Skip unrevealed/blinded subtries - nothing to prune
            let Some(subtrie) = self.lower_subtries[subtrie_idx].as_revealed_mut() else {
                continue;
            };

            // Retain only nodes/values not descended from any pruned root.
            subtrie.nodes.retain(|p, _| !is_strict_descendant_in(roots_group, p));
            subtrie.inner.values.retain(|p, _| !starts_with_pruned_in(roots_group, p));
        }

        // Branch node masks pruning
        self.branch_node_masks.retain(|p, _| {
            if SparseSubtrieType::path_len_is_upper(p.len()) {
                !starts_with_pruned_in(roots_upper, p)
            } else {
                !starts_with_pruned_in(roots_lower, p) && !starts_with_pruned_in(roots_upper, p)
            }
        });

        nodes_converted
    }

    fn update_leaves(
        &mut self,
        updates: &mut alloy_primitives::map::B256Map<crate::LeafUpdate>,
        mut proof_required_fn: impl FnMut(B256, u8),
    ) -> SparseTrieResult<()> {
        use crate::{provider::NoRevealProvider, LeafUpdate};

        #[cfg(feature = "trie-debug")]
        let recorded_updates: Vec<_> =
            updates.iter().map(|(k, v)| (*k, LeafUpdateRecord::from(v))).collect();
        #[cfg(feature = "trie-debug")]
        let mut recorded_proof_targets: Vec<(B256, u8)> = Vec::new();

        // Drain updates to avoid cloning keys while preserving the map's allocation.
        // On success, entries remain removed; on blinded node failure, they're re-inserted.
        let drained: Vec<_> = updates.drain().collect();

        for (key, update) in drained {
            let full_path = Nibbles::unpack(key);

            match update {
                LeafUpdate::Changed(value) => {
                    if value.is_empty() {
                        // Removal: remove_leaf with NoRevealProvider is atomic - returns a
                        // retriable error before any mutations (via pre_validate_reveal_chain).
                        match self.remove_leaf(&full_path, NoRevealProvider) {
                            Ok(()) => {}
                            Err(e) => {
                                if let Some(path) = Self::get_retriable_path(&e) {
                                    let (target_key, min_len) =
                                        Self::proof_target_for_path(key, &full_path, &path);
                                    proof_required_fn(target_key, min_len);
                                    #[cfg(feature = "trie-debug")]
                                    recorded_proof_targets.push((target_key, min_len));
                                    updates.insert(key, LeafUpdate::Changed(value));
                                } else {
                                    return Err(e);
                                }
                            }
                        }
                    } else {
                        // Update/insert: update_leaf is atomic - cleans up on error.
                        if let Err(e) = self.update_leaf(full_path, value.clone(), NoRevealProvider)
                        {
                            if let Some(path) = Self::get_retriable_path(&e) {
                                let (target_key, min_len) =
                                    Self::proof_target_for_path(key, &full_path, &path);
                                proof_required_fn(target_key, min_len);
                                #[cfg(feature = "trie-debug")]
                                recorded_proof_targets.push((target_key, min_len));
                                updates.insert(key, LeafUpdate::Changed(value));
                            } else {
                                return Err(e);
                            }
                        }
                    }
                }
                LeafUpdate::Touched => {
                    // Touched is read-only: check if path is accessible, request proof if blinded.
                    match self.find_leaf(&full_path, None) {
                        Err(LeafLookupError::BlindedNode { path, .. }) => {
                            let (target_key, min_len) =
                                Self::proof_target_for_path(key, &full_path, &path);
                            proof_required_fn(target_key, min_len);
                            #[cfg(feature = "trie-debug")]
                            recorded_proof_targets.push((target_key, min_len));
                            updates.insert(key, LeafUpdate::Touched);
                        }
                        // Path is fully revealed (exists or proven non-existent), no action needed.
                        Ok(_) | Err(LeafLookupError::ValueMismatch { .. }) => {}
                    }
                }
            }
        }

        #[cfg(feature = "trie-debug")]
        self.debug_recorder.record(RecordedOp::UpdateLeaves {
            updates: recorded_updates,
            remaining_keys: updates.keys().copied().collect(),
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
        updated: &HashMap<Nibbles, BranchNodeCompact>,
        removed: &HashSet<Nibbles>,
    ) {
        // Sync branch_node_masks with what's being committed to DB.
        // This ensures that on subsequent root() calls, the masks reflect the actual
        // DB state, which is needed for correct removal detection.
        self.branch_node_masks.reserve(updated.len());
        for (path, node) in updated {
            self.branch_node_masks.insert(
                *path,
                BranchNodeMasks { tree_mask: node.tree_mask, hash_mask: node.hash_mask },
            );
        }
        for path in removed {
            self.branch_node_masks.remove(path);
        }
    }
}

impl ParallelSparseTrie {
    /// Sets the thresholds that control when parallelism is used during operations.
    pub const fn with_parallelism_thresholds(mut self, thresholds: ParallelismThresholds) -> Self {
        self.parallelism_thresholds = thresholds;
        self
    }

    /// Returns true if retaining updates is enabled for the overall trie.
    const fn updates_enabled(&self) -> bool {
        self.updates.is_some()
    }

    /// Returns true if parallelism should be enabled for revealing the given number of nodes.
    /// Will always return false in nostd builds.
    const fn is_reveal_parallelism_enabled(&self, num_nodes: usize) -> bool {
        #[cfg(not(feature = "std"))]
        {
            let _ = num_nodes;
            return false;
        }

        #[cfg(feature = "std")]
        {
            num_nodes >= self.parallelism_thresholds.min_revealed_nodes
        }
    }

    /// Returns true if parallelism should be enabled for updating hashes with the given number
    /// of changed keys. Will always return false in nostd builds.
    const fn is_update_parallelism_enabled(&self, num_changed_keys: usize) -> bool {
        #[cfg(not(feature = "std"))]
        {
            let _ = num_changed_keys;
            return false;
        }

        #[cfg(feature = "std")]
        {
            num_changed_keys >= self.parallelism_thresholds.min_updated_nodes
        }
    }

    /// Checks if an error is retriable (`BlindedNode` or `NodeNotFoundInProvider`) and extracts
    /// the path if so.
    ///
    /// Both error types indicate that a node needs to be revealed before the operation can
    /// succeed. `BlindedNode` occurs when traversing to a Hash node, while `NodeNotFoundInProvider`
    /// occurs when `retain_updates` is enabled and an extension node's child needs revealing.
    const fn get_retriable_path(e: &SparseTrieError) -> Option<Nibbles> {
        match e.kind() {
            SparseTrieErrorKind::BlindedNode(path) |
            SparseTrieErrorKind::NodeNotFoundInProvider { path } => Some(*path),
            _ => None,
        }
    }

    /// Converts a nibbles path to a B256, right-padding with zeros to 64 nibbles.
    fn nibbles_to_padded_b256(path: &Nibbles) -> B256 {
        let mut bytes = [0u8; 32];
        path.pack_to(&mut bytes);
        B256::from(bytes)
    }

    /// Computes the proof target key and `min_len` for a blinded node error.
    ///
    /// Returns `(target_key, min_len)` where:
    /// - `target_key` is `full_key` if `path` is a prefix of `full_path`, otherwise the padded path
    /// - `min_len` is always based on `path.len()`
    fn proof_target_for_path(full_key: B256, full_path: &Nibbles, path: &Nibbles) -> (B256, u8) {
        let min_len = (path.len() as u8).min(64);
        let target_key =
            if full_path.starts_with(path) { full_key } else { Self::nibbles_to_padded_b256(path) };
        (target_key, min_len)
    }

    /// Creates a new revealed sparse trie from the given root node.
    ///
    /// This function initializes the internal structures and then reveals the root.
    /// It is a convenient method to create a trie when you already have the root node available.
    ///
    /// # Arguments
    ///
    /// * `root` - The root node of the trie
    /// * `masks` - Trie masks for root branch node
    /// * `retain_updates` - Whether to track updates
    ///
    /// # Returns
    ///
    /// Self if successful, or an error if revealing fails.
    pub fn from_root(
        root: TrieNodeV2,
        masks: Option<BranchNodeMasks>,
        retain_updates: bool,
    ) -> SparseTrieResult<Self> {
        Self::default().with_root(root, masks, retain_updates)
    }

    /// Returns a reference to the lower `SparseSubtrie` for the given path, or None if the
    /// path belongs to the upper trie, or if the lower subtrie for the path doesn't exist or is
    /// blinded.
    fn lower_subtrie_for_path(&self, path: &Nibbles) -> Option<&SparseSubtrie> {
        match SparseSubtrieType::from_path(path) {
            SparseSubtrieType::Upper => None,
            SparseSubtrieType::Lower(idx) => self.lower_subtries[idx].as_revealed_ref(),
        }
    }

    /// Returns a mutable reference to the lower `SparseSubtrie` for the given path, or None if the
    /// path belongs to the upper trie.
    ///
    /// This method will create/reveal a new lower subtrie for the given path if one isn't already.
    /// If one does exist, but its path field is longer than the given path, then the field will be
    /// set to the given path.
    fn lower_subtrie_for_path_mut(&mut self, path: &Nibbles) -> Option<&mut SparseSubtrie> {
        match SparseSubtrieType::from_path(path) {
            SparseSubtrieType::Upper => None,
            SparseSubtrieType::Lower(idx) => {
                self.lower_subtries[idx].reveal(path);
                self.subtrie_heat.mark_modified(idx);
                Some(self.lower_subtries[idx].as_revealed_mut().expect("just revealed"))
            }
        }
    }

    /// Returns a reference to either the lower or upper `SparseSubtrie` for the given path,
    /// depending on the path's length.
    ///
    /// Returns `None` if a lower subtrie does not exist for the given path.
    fn subtrie_for_path(&self, path: &Nibbles) -> Option<&SparseSubtrie> {
        if SparseSubtrieType::path_len_is_upper(path.len()) {
            Some(&self.upper_subtrie)
        } else {
            self.lower_subtrie_for_path(path)
        }
    }

    /// Returns a mutable reference to either the lower or upper `SparseSubtrie` for the given path,
    /// depending on the path's length.
    ///
    /// This method will create/reveal a new lower subtrie for the given path if one isn't already.
    /// If one does exist, but its path field is longer than the given path, then the field will be
    /// set to the given path.
    fn subtrie_for_path_mut(&mut self, path: &Nibbles) -> &mut SparseSubtrie {
        // We can't just call `lower_subtrie_for_path` and return `upper_subtrie` if it returns
        // None, because Rust complains about double mutable borrowing `self`.
        if SparseSubtrieType::path_len_is_upper(path.len()) {
            &mut self.upper_subtrie
        } else {
            self.lower_subtrie_for_path_mut(path).unwrap()
        }
    }

    /// Returns a mutable reference to a subtrie without marking it as modified.
    /// Used for internal operations like pruning that shouldn't affect heat tracking.
    fn subtrie_for_path_mut_untracked(&mut self, path: &Nibbles) -> Option<&mut SparseSubtrie> {
        if SparseSubtrieType::path_len_is_upper(path.len()) {
            Some(&mut self.upper_subtrie)
        } else {
            match SparseSubtrieType::from_path(path) {
                SparseSubtrieType::Upper => None,
                SparseSubtrieType::Lower(idx) => self.lower_subtries[idx].as_revealed_mut(),
            }
        }
    }

    /// Returns the next node in the traversal path from the given path towards the leaf for the
    /// given full leaf path, or an error if any node along the traversal path is not revealed.
    ///
    ///
    /// ## Panics
    ///
    /// If `from_path` is not a prefix of `leaf_full_path`.
    fn find_next_to_leaf(
        from_path: &Nibbles,
        from_node: &SparseNode,
        leaf_full_path: &Nibbles,
    ) -> FindNextToLeafOutcome {
        debug_assert!(leaf_full_path.len() >= from_path.len());
        debug_assert!(leaf_full_path.starts_with(from_path));

        match from_node {
            // If empty node is found it means the subtrie doesn't have any nodes in it, let alone
            // the target leaf.
            SparseNode::Empty => FindNextToLeafOutcome::NotFound,
            SparseNode::Leaf { key, .. } => {
                let mut found_full_path = *from_path;
                found_full_path.extend(key);

                if &found_full_path == leaf_full_path {
                    return FindNextToLeafOutcome::Found
                }
                FindNextToLeafOutcome::NotFound
            }
            SparseNode::Extension { key, .. } => {
                if leaf_full_path.len() == from_path.len() {
                    return FindNextToLeafOutcome::NotFound
                }

                let mut child_path = *from_path;
                child_path.extend(key);

                if !leaf_full_path.starts_with(&child_path) {
                    return FindNextToLeafOutcome::NotFound
                }
                FindNextToLeafOutcome::ContinueFrom(child_path)
            }
            SparseNode::Branch { state_mask, blinded_mask, .. } => {
                if leaf_full_path.len() == from_path.len() {
                    return FindNextToLeafOutcome::NotFound
                }

                let nibble = leaf_full_path.get_unchecked(from_path.len());
                if !state_mask.is_bit_set(nibble) {
                    return FindNextToLeafOutcome::NotFound
                }

                let mut child_path = *from_path;
                child_path.push_unchecked(nibble);

                if blinded_mask.is_bit_set(nibble) {
                    return FindNextToLeafOutcome::BlindedNode(child_path);
                }

                FindNextToLeafOutcome::ContinueFrom(child_path)
            }
        }
    }

    /// Called when a child node has collapsed into its parent as part of `remove_leaf`. If the
    /// new parent node is a leaf, then the previous child also was, and if the previous child was
    /// on a lower subtrie while the parent is on an upper then the leaf value needs to be moved to
    /// the upper.
    fn move_value_on_leaf_removal(
        &mut self,
        parent_path: &Nibbles,
        new_parent_node: &SparseNode,
        prev_child_path: &Nibbles,
    ) {
        // If the parent path isn't in the upper then it doesn't matter what the new node is,
        // there's no situation where a leaf value needs to be moved.
        if SparseSubtrieType::from_path(parent_path).lower_index().is_some() {
            return;
        }

        if let SparseNode::Leaf { key, .. } = new_parent_node {
            let Some(prev_child_subtrie) = self.lower_subtrie_for_path_mut(prev_child_path) else {
                return;
            };

            let mut leaf_full_path = *parent_path;
            leaf_full_path.extend(key);

            let val = prev_child_subtrie.inner.values.remove(&leaf_full_path).expect("ParallelSparseTrie is in an inconsistent state, expected value on subtrie which wasn't found");
            self.upper_subtrie.inner.values.insert(leaf_full_path, val);
        }
    }

    /// Used by `remove_leaf` to ensure that when a node is removed from a lower subtrie that any
    /// externalities are handled. These can include:
    /// - Removing the lower subtrie completely, if it is now empty.
    /// - Updating the `path` field of the lower subtrie to indicate that its root node has changed.
    ///
    /// This method assumes that the caller will deal with putting all other nodes in the trie into
    /// a consistent state after the removal of this one.
    ///
    /// ## Panics
    ///
    /// - If the removed node was not a leaf or extension.
    fn remove_node(&mut self, path: &Nibbles) {
        let subtrie = self.subtrie_for_path_mut(path);
        let node = subtrie.nodes.remove(path);

        let Some(idx) = SparseSubtrieType::from_path(path).lower_index() else {
            // When removing a node from the upper trie there's nothing special we need to do to fix
            // its path field; the upper trie's path is always empty.
            return;
        };

        match node {
            Some(SparseNode::Leaf { .. }) => {
                // If the leaf was the final node in its lower subtrie then we can blind the
                // subtrie, effectively marking it as empty.
                if subtrie.nodes.is_empty() {
                    self.lower_subtries[idx].clear();
                }
            }
            Some(SparseNode::Extension { key, .. }) => {
                // If the removed extension was the root node of a lower subtrie then the lower
                // subtrie's `path` needs to be updated to be whatever node the extension used to
                // point to.
                if &subtrie.path == path {
                    subtrie.path.extend(&key);
                }
            }
            _ => panic!("Expected to remove a leaf or extension, but removed {node:?}"),
        }
    }

    /// Given the path to a parent branch node and a child node which is the sole remaining child on
    /// that branch after removing a leaf, returns a node to replace the parent branch node and a
    /// boolean indicating if the child should be deleted.
    ///
    /// ## Panics
    ///
    /// - If either parent or child node is not already revealed.
    /// - If parent's path is not a prefix of the child's path.
    fn branch_changes_on_leaf_removal(
        parent_path: &Nibbles,
        remaining_child_path: &Nibbles,
        remaining_child_node: &SparseNode,
    ) -> (SparseNode, bool) {
        debug_assert!(remaining_child_path.len() > parent_path.len());
        debug_assert!(remaining_child_path.starts_with(parent_path));

        let remaining_child_nibble = remaining_child_path.get_unchecked(parent_path.len());

        // If we swap the branch node out either an extension or leaf, depending on
        // what its remaining child is.
        match remaining_child_node {
            SparseNode::Empty => {
                panic!("remaining child must have been revealed already")
            }
            // If the only child is a leaf node, we downgrade the branch node into a
            // leaf node, prepending the nibble to the key, and delete the old
            // child.
            SparseNode::Leaf { key, .. } => {
                let mut new_key = Nibbles::from_nibbles_unchecked([remaining_child_nibble]);
                new_key.extend(key);
                (SparseNode::new_leaf(new_key), true)
            }
            // If the only child node is an extension node, we downgrade the branch
            // node into an even longer extension node, prepending the nibble to the
            // key, and delete the old child.
            SparseNode::Extension { key, .. } => {
                let mut new_key = Nibbles::from_nibbles_unchecked([remaining_child_nibble]);
                new_key.extend(key);
                (SparseNode::new_ext(new_key), true)
            }
            // If the only child is a branch node, we downgrade the current branch
            // node into a one-nibble extension node.
            SparseNode::Branch { .. } => (
                SparseNode::new_ext(Nibbles::from_nibbles_unchecked([remaining_child_nibble])),
                false,
            ),
        }
    }

    /// Given the path to a parent extension and its key, and a child node (not necessarily on this
    /// subtrie), returns an optional replacement parent node. If a replacement is returned then the
    /// child node should be deleted.
    ///
    /// ## Panics
    ///
    /// - If either parent or child node is not already revealed.
    /// - If parent's path is not a prefix of the child's path.
    fn extension_changes_on_leaf_removal(
        parent_path: &Nibbles,
        parent_key: &Nibbles,
        child_path: &Nibbles,
        child: &SparseNode,
    ) -> Option<SparseNode> {
        debug_assert!(child_path.len() > parent_path.len());
        debug_assert!(child_path.starts_with(parent_path));

        // If the parent node is an extension node, we need to look at its child to see
        // if we need to merge it.
        match child {
            SparseNode::Empty => {
                panic!("child must be revealed")
            }
            // For a leaf node, we collapse the extension node into a leaf node,
            // extending the key. While it's impossible to encounter an extension node
            // followed by a leaf node in a complete trie, it's possible here because we
            // could have downgraded the extension node's child into a leaf node from a
            // branch in a previous call to `branch_changes_on_leaf_removal`.
            SparseNode::Leaf { key, .. } => {
                let mut new_key = *parent_key;
                new_key.extend(key);
                Some(SparseNode::new_leaf(new_key))
            }
            // Similar to the leaf node, for an extension node, we collapse them into one
            // extension node, extending the key.
            SparseNode::Extension { key, .. } => {
                let mut new_key = *parent_key;
                new_key.extend(key);
                Some(SparseNode::new_ext(new_key))
            }
            // For a branch node, we just leave the extension node as-is.
            SparseNode::Branch { .. } => None,
        }
    }

    /// Drains any [`SparseTrieUpdatesAction`]s from the given subtrie, and applies each action to
    /// the given `updates` set. If the given set is None then this is a no-op.
    #[instrument(level = "trace", target = "trie::parallel_sparse", skip_all)]
    fn apply_subtrie_update_actions(
        &mut self,
        update_actions: impl Iterator<Item = SparseTrieUpdatesAction>,
    ) {
        if let Some(updates) = self.updates.as_mut() {
            let additional = update_actions.size_hint().0;
            updates.updated_nodes.reserve(additional);
            updates.removed_nodes.reserve(additional);
            for action in update_actions {
                match action {
                    SparseTrieUpdatesAction::InsertRemoved(path) => {
                        updates.updated_nodes.remove(&path);
                        updates.removed_nodes.insert(path);
                    }
                    SparseTrieUpdatesAction::RemoveUpdated(path) => {
                        updates.updated_nodes.remove(&path);
                    }
                    SparseTrieUpdatesAction::InsertUpdated(path, branch_node) => {
                        updates.updated_nodes.insert(path, branch_node);
                        updates.removed_nodes.remove(&path);
                    }
                }
            }
        };
    }

    /// Updates hashes for the upper subtrie, using nodes from both upper and lower subtries.
    #[instrument(level = "trace", target = "trie::parallel_sparse", skip_all, ret)]
    fn update_upper_subtrie_hashes(&mut self, prefix_set: &mut PrefixSet) -> RlpNode {
        trace!(target: "trie::parallel_sparse", "Updating upper subtrie hashes");

        debug_assert!(self.upper_subtrie.inner.buffers.path_stack.is_empty());
        self.upper_subtrie.inner.buffers.path_stack.push(RlpNodePathStackItem {
            path: Nibbles::default(), // Start from root
            is_in_prefix_set: None,
        });

        #[cfg(feature = "metrics")]
        let start = Instant::now();

        let mut update_actions_buf =
            self.updates_enabled().then(|| self.update_actions_buffers.pop().unwrap_or_default());

        while let Some(stack_item) = self.upper_subtrie.inner.buffers.path_stack.pop() {
            let path = stack_item.path;
            let node = if path.len() < UPPER_TRIE_MAX_DEPTH {
                self.upper_subtrie.nodes.get_mut(&path).expect("upper subtrie node must exist")
            } else {
                let index = path_subtrie_index_unchecked(&path);
                let node = self.lower_subtries[index]
                    .as_revealed_mut()
                    .expect("lower subtrie must exist")
                    .nodes
                    .get_mut(&path)
                    .expect("lower subtrie node must exist");
                // Lower subtrie root node RLP nodes must be computed before updating upper subtrie
                // hashes
                debug_assert!(
                    node.cached_rlp_node().is_some(),
                    "Lower subtrie root node {node:?} at path {path:?} has no cached RLP node"
                );
                node
            };

            // Calculate the RLP node for the current node using upper subtrie
            self.upper_subtrie.inner.rlp_node(
                prefix_set,
                &mut update_actions_buf,
                stack_item,
                node,
                &self.branch_node_masks,
            );
        }

        // If there were any branch node updates as a result of calculating the RLP node for the
        // upper trie then apply them to the top-level set.
        if let Some(mut update_actions_buf) = update_actions_buf {
            self.apply_subtrie_update_actions(
                #[allow(clippy::iter_with_drain)]
                update_actions_buf.drain(..),
            );
            self.update_actions_buffers.push(update_actions_buf);
        }

        #[cfg(feature = "metrics")]
        self.metrics.subtrie_upper_hash_latency.record(start.elapsed());

        debug_assert_eq!(self.upper_subtrie.inner.buffers.rlp_node_stack.len(), 1);
        self.upper_subtrie.inner.buffers.rlp_node_stack.pop().unwrap().rlp_node
    }

    /// Returns:
    /// 1. List of lower [subtries](SparseSubtrie) that have changed according to the provided
    ///    [prefix set](PrefixSet). See documentation of [`ChangedSubtrie`] for more details. Lower
    ///    subtries whose root node is missing a hash will also be returned; this is required to
    ///    handle cases where extensions/leafs get shortened and therefore moved from the upper to a
    ///    lower subtrie.
    /// 2. Prefix set of keys that do not belong to any lower subtrie.
    ///
    /// This method helps optimize hash recalculations by identifying which specific
    /// lower subtries need to be updated. Each lower subtrie can then be updated in parallel.
    ///
    /// IMPORTANT: The method removes the subtries from `lower_subtries`, and the caller is
    /// responsible for returning them back into the array.
    #[instrument(level = "trace", target = "trie::parallel_sparse", skip_all, fields(prefix_set_len = prefix_set.len()))]
    fn take_changed_lower_subtries(
        &mut self,
        prefix_set: &mut PrefixSet,
    ) -> (Vec<ChangedSubtrie>, PrefixSetMut) {
        // Fast-path: If the prefix set is empty then no subtries can have been changed. Just return
        // empty values.
        if prefix_set.is_empty() {
            return Default::default();
        }

        // Clone the prefix set to iterate over its keys. Cloning is cheap, it's just an Arc.
        let prefix_set_clone = prefix_set.clone();
        let mut prefix_set_iter = prefix_set_clone.into_iter().copied().peekable();
        let mut changed_subtries = Vec::new();
        let mut unchanged_prefix_set = PrefixSetMut::default();
        let updates_enabled = self.updates_enabled();

        for (index, subtrie) in self.lower_subtries.iter_mut().enumerate() {
            if let Some(subtrie) = subtrie.take_revealed_if(|subtrie| {
                prefix_set.contains(&subtrie.path) ||
                    subtrie
                        .nodes
                        .get(&subtrie.path)
                        .is_some_and(|n| n.cached_rlp_node().is_none())
            }) {
                let prefix_set = if prefix_set.all() {
                    unchanged_prefix_set = PrefixSetMut::all();
                    PrefixSetMut::all()
                } else {
                    // Take those keys from the original prefix set that start with the subtrie path
                    //
                    // Subtries are stored in the order of their paths, so we can use the same
                    // prefix set iterator.
                    let mut new_prefix_set = Vec::new();
                    while let Some(key) = prefix_set_iter.peek() {
                        if key.starts_with(&subtrie.path) {
                            // If the key starts with the subtrie path, add it to the new prefix set
                            new_prefix_set.push(prefix_set_iter.next().unwrap());
                        } else if new_prefix_set.is_empty() && key < &subtrie.path {
                            // If we didn't yet have any keys that belong to this subtrie, and the
                            // current key is still less than the subtrie path, add it to the
                            // unchanged prefix set
                            unchanged_prefix_set.insert(prefix_set_iter.next().unwrap());
                        } else {
                            // If we're past the subtrie path, we're done with this subtrie. Do not
                            // advance the iterator, the next key will be processed either by the
                            // next subtrie or inserted into the unchanged prefix set.
                            break
                        }
                    }
                    PrefixSetMut::from(new_prefix_set)
                }
                .freeze();

                // We need the full path of root node of the lower subtrie to the unchanged prefix
                // set, so that we don't skip it when calculating hashes for the upper subtrie.
                match subtrie.nodes.get(&subtrie.path) {
                    Some(SparseNode::Extension { key, .. } | SparseNode::Leaf { key, .. }) => {
                        unchanged_prefix_set.insert(subtrie.path.join(key));
                    }
                    Some(SparseNode::Branch { .. }) => {
                        unchanged_prefix_set.insert(subtrie.path);
                    }
                    _ => {}
                }

                let update_actions_buf =
                    updates_enabled.then(|| self.update_actions_buffers.pop().unwrap_or_default());

                changed_subtries.push(ChangedSubtrie {
                    index,
                    subtrie,
                    prefix_set,
                    update_actions_buf,
                });
            }
        }

        // Extend the unchanged prefix set with the remaining keys that are not part of any subtries
        unchanged_prefix_set.extend_keys(prefix_set_iter);

        (changed_subtries, unchanged_prefix_set)
    }

    /// Returns an iterator over all nodes in the trie in no particular order.
    #[cfg(test)]
    fn all_nodes(&self) -> impl IntoIterator<Item = (&Nibbles, &SparseNode)> {
        let mut nodes = vec![];
        for subtrie in self.lower_subtries.iter().filter_map(LowerSparseSubtrie::as_revealed_ref) {
            nodes.extend(subtrie.nodes.iter())
        }
        nodes.extend(self.upper_subtrie.nodes.iter());
        nodes
    }

    /// Reveals a trie node in the upper trie if it has not been revealed before. When revealing
    /// branch/extension nodes this may recurse into a lower trie to reveal a child.
    ///
    /// This function decodes a trie node and inserts it into the trie structure. It handles
    /// different node types (leaf, extension, branch) by appropriately adding them to the trie and
    /// recursively revealing their children.
    ///
    /// # Arguments
    ///
    /// * `path` - The path where the node should be revealed
    /// * `node` - The trie node to reveal
    /// * `masks` - Branch node masks if known
    ///
    /// # Returns
    ///
    /// `Ok(())` if successful, or an error if the node was not revealed.
    fn reveal_upper_node(
        &mut self,
        path: Nibbles,
        node: &TrieNodeV2,
        masks: Option<BranchNodeMasks>,
    ) -> SparseTrieResult<()> {
        // Only reveal nodes that can be reached given the current state of the upper trie. If they
        // can't be reached, it means that they were removed.
        if !self.is_path_reachable_from_upper(&path) {
            return Ok(())
        }

        // Exit early if the node was already revealed before.
        if !self.upper_subtrie.reveal_node(path, node, masks, None)? {
            if let TrieNodeV2::Branch(branch) = node {
                if branch.key.is_empty() {
                    return Ok(());
                }

                // We might still potentially need to reveal a child branch node in the lower
                // subtrie, even if the upper subtrie already knew about the extension node.
                if SparseSubtrieType::path_len_is_upper(path.len() + branch.key.len()) {
                    return Ok(())
                }
            } else {
                return Ok(());
            }
        }

        // The previous upper_trie.reveal_node call will not have revealed any child nodes via
        // reveal_node_or_hash if the child node would be found on a lower subtrie. We handle that
        // here by manually checking the specific cases where this could happen, and calling
        // reveal_node_or_hash for each.
        match node {
            TrieNodeV2::Branch(branch) => {
                let mut branch_path = path;
                branch_path.extend(&branch.key);

                // If only the parent extension belongs to the upper trie, we need to reveal the
                // actual branch node in the corresponding lower subtrie.
                if !SparseSubtrieType::path_len_is_upper(branch_path.len()) {
                    self.lower_subtrie_for_path_mut(&branch_path)
                        .expect("branch_path must have a lower subtrie")
                        .reveal_branch(
                            branch_path,
                            branch.state_mask,
                            &branch.stack,
                            masks,
                            branch.branch_rlp_node.clone(),
                        )?
                } else if !SparseSubtrieType::path_len_is_upper(branch_path.len() + 1) {
                    // If a branch is at the cutoff level of the trie then it will be in the upper
                    // trie, but all of its children will be in a lower trie.
                    // Check if a child node would be in the lower subtrie, and
                    // reveal accordingly.
                    for (stack_ptr, idx) in branch.state_mask.iter().enumerate() {
                        let mut child_path = branch_path;
                        child_path.push_unchecked(idx);
                        let child = &branch.stack[stack_ptr];

                        // Only reveal children that are not hashes. Hashes are stored on branch
                        // nodes directly.
                        if !child.is_hash() {
                            self.lower_subtrie_for_path_mut(&child_path)
                                .expect("child_path must have a lower subtrie")
                                .reveal_node(
                                    child_path,
                                    &TrieNodeV2::decode(&mut branch.stack[stack_ptr].as_ref())?,
                                    None,
                                    None,
                                )?;
                        }
                    }
                }
            }
            TrieNodeV2::Extension(ext) => {
                let mut child_path = path;
                child_path.extend(&ext.key);
                if let Some(subtrie) = self.lower_subtrie_for_path_mut(&child_path) {
                    subtrie.reveal_node(
                        child_path,
                        &TrieNodeV2::decode(&mut ext.child.as_ref())?,
                        None,
                        None,
                    )?;
                }
            }
            TrieNodeV2::EmptyRoot | TrieNodeV2::Leaf(_) => (),
        }

        Ok(())
    }

    /// Return updated subtries back to the trie after executing any actions required on the
    /// top-level `SparseTrieUpdates`.
    #[instrument(level = "trace", target = "trie::parallel_sparse", skip_all)]
    fn insert_changed_subtries(
        &mut self,
        changed_subtries: impl IntoIterator<Item = ChangedSubtrie>,
    ) {
        for ChangedSubtrie { index, subtrie, update_actions_buf, .. } in changed_subtries {
            if let Some(mut update_actions_buf) = update_actions_buf {
                self.apply_subtrie_update_actions(
                    #[allow(clippy::iter_with_drain)]
                    update_actions_buf.drain(..),
                );
                self.update_actions_buffers.push(update_actions_buf);
            }

            self.lower_subtries[index] = LowerSparseSubtrie::Revealed(subtrie);
            self.subtrie_heat.mark_modified(index);
        }
    }

    /// Returns a heuristic for the in-memory size of this trie in bytes.
    ///
    /// This is an approximation that accounts for:
    /// - The upper subtrie nodes and values
    /// - All revealed lower subtries nodes and values
    /// - The prefix set keys
    /// - The branch node masks map
    /// - Updates if retained
    /// - Update action buffers
    ///
    /// Note: Heap allocations for hash maps may be larger due to load factor overhead.
    pub fn memory_size(&self) -> usize {
        let mut size = core::mem::size_of::<Self>();

        // Upper subtrie
        size += self.upper_subtrie.memory_size();

        // Lower subtries (both Revealed and Blind with allocation)
        for subtrie in self.lower_subtries.iter() {
            size += subtrie.memory_size();
        }

        // Prefix set keys
        size += self.prefix_set.len() * core::mem::size_of::<Nibbles>();

        // Branch node masks map
        size += self.branch_node_masks.len() *
            (core::mem::size_of::<Nibbles>() + core::mem::size_of::<BranchNodeMasks>());

        // Updates if present
        if let Some(updates) = &self.updates {
            size += updates.updated_nodes.len() *
                (core::mem::size_of::<Nibbles>() + core::mem::size_of::<BranchNodeCompact>());
            size += updates.removed_nodes.len() * core::mem::size_of::<Nibbles>();
        }

        // Update actions buffers
        for buf in &self.update_actions_buffers {
            size += buf.capacity() * core::mem::size_of::<SparseTrieUpdatesAction>();
        }

        size
    }

    /// Determines if the given path can be directly reached from the upper trie.
    fn is_path_reachable_from_upper(&self, path: &Nibbles) -> bool {
        let mut current = Nibbles::default();
        while current.len() < path.len() {
            let Some(node) = self.upper_subtrie.nodes.get(&current) else { return false };
            match node {
                SparseNode::Branch { state_mask, .. } => {
                    if !state_mask.is_bit_set(path.get_unchecked(current.len())) {
                        return false
                    }

                    current.push_unchecked(path.get_unchecked(current.len()));
                }
                SparseNode::Extension { key, .. } => {
                    if *key != path.slice(current.len()..current.len() + key.len()) {
                        return false
                    }
                    current.extend(key);
                }
                SparseNode::Empty | SparseNode::Leaf { .. } => return false,
            }
        }
        true
    }

    /// Checks if a boundary leaf (at `path.len() == UPPER_TRIE_MAX_DEPTH`) is reachable from its
    /// parent branch in the upper subtrie.
    ///
    /// This is used for leaves that sit at the upper/lower subtrie boundary, where the leaf is
    /// in a lower subtrie but its parent branch is in the upper subtrie.
    fn is_boundary_leaf_reachable(
        upper_nodes: &HashMap<Nibbles, SparseNode>,
        path: &Nibbles,
        node: &TrieNodeV2,
    ) -> bool {
        debug_assert_eq!(path.len(), UPPER_TRIE_MAX_DEPTH);

        if !matches!(node, TrieNodeV2::Leaf(_)) {
            return true
        }

        let parent_path = path.slice(..path.len() - 1);
        let leaf_nibble = path.get_unchecked(path.len() - 1);

        match upper_nodes.get(&parent_path) {
            Some(SparseNode::Branch { state_mask, .. }) => state_mask.is_bit_set(leaf_nibble),
            _ => false,
        }
    }

    /// Returns a bitset of all subtries that are reachable from the upper trie. If subtrie is not
    /// reachable it means that it does not exist.
    fn reachable_subtries(&self) -> SubtriesBitmap {
        let mut reachable = SubtriesBitmap::default();

        let mut stack = Vec::new();
        stack.push(Nibbles::default());

        while let Some(current) = stack.pop() {
            let Some(node) = self.upper_subtrie.nodes.get(&current) else { continue };
            match node {
                SparseNode::Branch { state_mask, .. } => {
                    for idx in state_mask.iter() {
                        let mut next = current;
                        next.push_unchecked(idx);
                        if next.len() >= UPPER_TRIE_MAX_DEPTH {
                            reachable.set(path_subtrie_index_unchecked(&next));
                        } else {
                            stack.push(next);
                        }
                    }
                }
                SparseNode::Extension { key, .. } => {
                    let mut next = current;
                    next.extend(key);
                    if next.len() >= UPPER_TRIE_MAX_DEPTH {
                        reachable.set(path_subtrie_index_unchecked(&next));
                    } else {
                        stack.push(next);
                    }
                }
                SparseNode::Empty | SparseNode::Leaf { .. } => {}
            };
        }

        reachable
    }
}

/// Bitset tracking which of the 256 lower subtries were modified in the current cycle.
#[derive(Clone, Default, PartialEq, Eq, Debug)]
struct SubtriesBitmap(U256);

impl SubtriesBitmap {
    /// Marks a subtrie index as modified.
    #[inline]
    fn set(&mut self, idx: usize) {
        debug_assert!(idx < NUM_LOWER_SUBTRIES);
        self.0.set_bit(idx, true);
    }

    /// Returns whether a subtrie index is marked as modified.
    #[inline]
    fn get(&self, idx: usize) -> bool {
        debug_assert!(idx < NUM_LOWER_SUBTRIES);
        self.0.bit(idx)
    }

    /// Clears all modification flags.
    #[inline]
    const fn clear(&mut self) {
        self.0 = U256::ZERO;
    }
}

/// Tracks heat (modification frequency) for each of the 256 lower subtries.
///
/// Heat is used to avoid pruning frequently-modified subtries, which would cause
/// expensive re-reveal operations on subsequent updates.
///
/// - Heat is incremented by 2 when a subtrie is modified
/// - Heat decays by 1 each prune cycle for subtries not modified that cycle
/// - Subtries with heat > 0 are considered "hot" and skipped during pruning
#[derive(Clone, PartialEq, Eq, Debug)]
struct SubtrieModifications {
    /// Heat level (0-255) for each of the 256 lower subtries.
    heat: [u8; NUM_LOWER_SUBTRIES],
    /// Tracks which subtries were modified in the current cycle.
    modified: SubtriesBitmap,
}

impl Default for SubtrieModifications {
    fn default() -> Self {
        Self { heat: [0; NUM_LOWER_SUBTRIES], modified: SubtriesBitmap::default() }
    }
}

impl SubtrieModifications {
    /// Marks a subtrie as modified, incrementing its heat by 1.
    #[inline]
    fn mark_modified(&mut self, idx: usize) {
        debug_assert!(idx < NUM_LOWER_SUBTRIES);
        self.modified.set(idx);
        self.heat[idx] = self.heat[idx].saturating_add(1);
    }

    /// Returns whether a subtrie is currently hot (heat > 0).
    #[inline]
    fn is_hot(&self, idx: usize) -> bool {
        debug_assert!(idx < NUM_LOWER_SUBTRIES);
        self.heat[idx] > 0
    }

    /// Decays heat for subtries not modified this cycle and resets modification tracking.
    /// Called at the start of each prune cycle.
    fn decay_and_reset(&mut self) {
        for (idx, heat) in self.heat.iter_mut().enumerate() {
            if !self.modified.get(idx) {
                *heat = heat.saturating_sub(1);
            }
        }
        self.modified.clear();
    }

    /// Clears all heat tracking state.
    const fn clear(&mut self) {
        self.heat = [0; NUM_LOWER_SUBTRIES];
        self.modified.clear();
    }
}

/// This is a subtrie of the [`ParallelSparseTrie`] that contains a map from path to sparse trie
/// nodes.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct SparseSubtrie {
    /// The root path of this subtrie.
    ///
    /// This is the _full_ path to this subtrie, meaning it includes the first
    /// [`UPPER_TRIE_MAX_DEPTH`] nibbles that we also use for indexing subtries in the
    /// [`ParallelSparseTrie`].
    ///
    /// There should be a node for this path in `nodes` map.
    pub(crate) path: Nibbles,
    /// The map from paths to sparse trie nodes within this subtrie.
    nodes: HashMap<Nibbles, SparseNode>,
    /// Subset of fields for mutable access while `nodes` field is also being mutably borrowed.
    inner: SparseSubtrieInner,
}

/// Returned by the `find_next_to_leaf` method to indicate either that the leaf has been found,
/// traversal should be continued from the given path, or the leaf is not in the trie.
enum FindNextToLeafOutcome {
    /// `Found` indicates that the leaf was found at the given path.
    Found,
    /// `ContinueFrom` indicates that traversal should continue from the given path.
    ContinueFrom(Nibbles),
    /// `NotFound` indicates that there is no way to traverse to the leaf, as it is not in the
    /// trie.
    NotFound,
    /// `BlindedNode` indicates that the node is blinded with the contained hash and cannot be
    /// traversed.
    BlindedNode(Nibbles),
}

impl SparseSubtrie {
    /// Creates a new empty subtrie with the specified root path.
    pub(crate) fn new(path: Nibbles) -> Self {
        Self { path, ..Default::default() }
    }

    /// Returns true if this subtrie has any nodes, false otherwise.
    pub(crate) fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Returns true if the current path and its child are both found in the same level.
    fn is_child_same_level(current_path: &Nibbles, child_path: &Nibbles) -> bool {
        let current_level = core::mem::discriminant(&SparseSubtrieType::from_path(current_path));
        let child_level = core::mem::discriminant(&SparseSubtrieType::from_path(child_path));
        current_level == child_level
    }

    /// Checks if a leaf node at the given path is reachable from its parent branch node.
    ///
    /// Returns `true` if:
    /// - The path is at the root (no parent to check)
    /// - The parent branch node has the corresponding `state_mask` bit set for this leaf
    ///
    /// Returns `false` if the parent is a branch node that doesn't have the `state_mask` bit set
    /// for this leaf's nibble, meaning the leaf is not reachable.
    fn is_leaf_reachable_from_parent(&self, path: &Nibbles) -> bool {
        if path.is_empty() {
            return true
        }

        let parent_path = path.slice(..path.len() - 1);
        let leaf_nibble = path.get_unchecked(path.len() - 1);

        match self.nodes.get(&parent_path) {
            Some(SparseNode::Branch { state_mask, .. }) => state_mask.is_bit_set(leaf_nibble),
            _ => false,
        }
    }

    /// Updates or inserts a leaf node at the specified key path with the provided RLP-encoded
    /// value.
    ///
    /// If the leaf did not previously exist, this method adjusts the trie structure by inserting
    /// new leaf nodes, splitting branch nodes, or collapsing extension nodes as needed.
    ///
    /// # Returns
    ///
    /// Returns the path and masks of any blinded node revealed as a result of updating the leaf.
    ///
    /// If an update requires revealing a blinded node, an error is returned if the blinded
    /// provider returns an error.
    ///
    /// This method is atomic: if an error occurs during structural changes, all modifications
    /// are rolled back and the trie state is unchanged.
    pub fn update_leaf(&mut self, full_path: Nibbles, value: Vec<u8>) -> SparseTrieResult<()> {
        debug_assert!(full_path.starts_with(&self.path));

        // Check if value already exists - if so, just update it (no structural changes needed)
        if let Entry::Occupied(mut e) = self.inner.values.entry(full_path) {
            e.insert(value);
            return Ok(())
        }

        // Here we are starting at the root of the subtrie, and traversing from there.
        let mut current = Some(self.path);

        while let Some(current_path) = current.as_mut() {
            match self.update_next_node(current_path, &full_path)? {
                LeafUpdateStep::Continue => {}
                LeafUpdateStep::NodeNotFound | LeafUpdateStep::Complete { .. } => break,
            }
        }

        // Only insert the value after all structural changes succeed
        self.inner.values.insert(full_path, value);

        Ok(())
    }

    /// Processes the current node, returning what to do next in the leaf update process.
    ///
    /// This will add or update any nodes in the trie as necessary.
    ///
    /// Returns a `LeafUpdateStep` containing the next node to process (if any) and
    /// the paths of nodes that were inserted during this step.
    fn update_next_node(
        &mut self,
        current: &mut Nibbles,
        path: &Nibbles,
    ) -> SparseTrieResult<LeafUpdateStep> {
        debug_assert!(path.starts_with(&self.path));
        debug_assert!(current.starts_with(&self.path));
        debug_assert!(path.starts_with(current));
        let Some(node) = self.nodes.get_mut(current) else {
            return Ok(LeafUpdateStep::NodeNotFound);
        };

        match node {
            SparseNode::Empty => {
                // We need to insert the node with a different path and key depending on the path of
                // the subtrie.
                let path = path.slice(self.path.len()..);
                *node = SparseNode::new_leaf(path);
                Ok(LeafUpdateStep::complete_with_insertions(vec![*current]))
            }
            SparseNode::Leaf { key: current_key, .. } => {
                current.extend(current_key);

                // this leaf is being updated
                debug_assert!(current != path, "we already checked leaf presence in the beginning");

                // find the common prefix
                let common = current.common_prefix_length(path);

                // update existing node
                let new_ext_key = current.slice(current.len() - current_key.len()..common);
                *node = SparseNode::new_ext(new_ext_key);

                // create a branch node and corresponding leaves
                self.nodes.reserve(3);
                let branch_path = current.slice(..common);
                let new_leaf_path = path.slice(..=common);
                let existing_leaf_path = current.slice(..=common);

                self.nodes.insert(
                    branch_path,
                    SparseNode::new_split_branch(
                        current.get_unchecked(common),
                        path.get_unchecked(common),
                    ),
                );
                self.nodes.insert(new_leaf_path, SparseNode::new_leaf(path.slice(common + 1..)));
                self.nodes
                    .insert(existing_leaf_path, SparseNode::new_leaf(current.slice(common + 1..)));

                Ok(LeafUpdateStep::complete_with_insertions(vec![
                    branch_path,
                    new_leaf_path,
                    existing_leaf_path,
                ]))
            }
            SparseNode::Extension { key, .. } => {
                current.extend(key);

                if !path.starts_with(current) {
                    // find the common prefix
                    let common = current.common_prefix_length(path);
                    *key = current.slice(current.len() - key.len()..common);

                    // create state mask for new branch node
                    // NOTE: this might overwrite the current extension node
                    self.nodes.reserve(3);
                    let branch_path = current.slice(..common);
                    let new_leaf_path = path.slice(..=common);
                    let branch = SparseNode::new_split_branch(
                        current.get_unchecked(common),
                        path.get_unchecked(common),
                    );

                    self.nodes.insert(branch_path, branch);

                    // create new leaf
                    let new_leaf = SparseNode::new_leaf(path.slice(common + 1..));
                    self.nodes.insert(new_leaf_path, new_leaf);

                    let mut inserted_nodes = vec![branch_path, new_leaf_path];

                    // recreate extension to previous child if needed
                    let key = current.slice(common + 1..);
                    if !key.is_empty() {
                        let ext_path = current.slice(..=common);
                        self.nodes.insert(ext_path, SparseNode::new_ext(key));
                        inserted_nodes.push(ext_path);
                    }

                    return Ok(LeafUpdateStep::complete_with_insertions(inserted_nodes))
                }

                Ok(LeafUpdateStep::Continue)
            }
            SparseNode::Branch { state_mask, blinded_mask, .. } => {
                let nibble = path.get_unchecked(current.len());
                current.push_unchecked(nibble);

                if !state_mask.is_bit_set(nibble) {
                    state_mask.set_bit(nibble);
                    let new_leaf = SparseNode::new_leaf(path.slice(current.len()..));
                    self.nodes.insert(*current, new_leaf);
                    return Ok(LeafUpdateStep::complete_with_insertions(vec![*current]))
                }

                if blinded_mask.is_bit_set(nibble) {
                    return Err(SparseTrieErrorKind::BlindedNode(*current).into());
                }

                // If the nibble is set, we can continue traversing the branch.
                Ok(LeafUpdateStep::Continue)
            }
        }
    }

    /// Reveals a branch node at the given path.
    fn reveal_branch(
        &mut self,
        path: Nibbles,
        state_mask: TrieMask,
        children: &[RlpNode],
        masks: Option<BranchNodeMasks>,
        rlp_node: Option<RlpNode>,
    ) -> SparseTrieResult<()> {
        match self.nodes.entry(path) {
            Entry::Occupied(_) => {
                // Branch already revealed, do nothing
                return Ok(());
            }
            Entry::Vacant(entry) => {
                let state =
                    match rlp_node.as_ref() {
                        Some(rlp_node) => SparseNodeState::Cached {
                            rlp_node: rlp_node.clone(),
                            store_in_db_trie: Some(masks.is_some_and(|m| {
                                !m.hash_mask.is_empty() || !m.tree_mask.is_empty()
                            })),
                        },
                        None => SparseNodeState::Dirty,
                    };

                let mut blinded_mask = TrieMask::default();
                let mut blinded_hashes = Box::new([B256::ZERO; 16]);

                for (stack_ptr, idx) in state_mask.iter().enumerate() {
                    let mut child_path = path;
                    child_path.push_unchecked(idx);
                    let child = &children[stack_ptr];

                    if let Some(hash) = child.as_hash() {
                        blinded_mask.set_bit(idx);
                        blinded_hashes[idx as usize] = hash;
                    }
                }

                entry.insert(SparseNode::Branch {
                    state_mask,
                    state,
                    blinded_mask,
                    blinded_hashes,
                });
            }
        }

        // For a branch node, iterate over all children. This must happen second so leaf
        // children can check connectivity with parent branch.
        for (stack_ptr, idx) in state_mask.iter().enumerate() {
            let mut child_path = path;
            child_path.push_unchecked(idx);
            let child = &children[stack_ptr];
            if !child.is_hash() && Self::is_child_same_level(&path, &child_path) {
                // Reveal each child node or hash it has, but only if the child is on
                // the same level as the parent.
                self.reveal_node(
                    child_path,
                    &TrieNodeV2::decode(&mut child.as_ref())?,
                    None,
                    None,
                )?;
            }
        }

        Ok(())
    }

    /// Internal implementation of the method of the same name on `ParallelSparseTrie`.
    ///
    /// This accepts `hash_from_upper` to handle cases when boundary nodes revealed in lower subtrie
    /// but its blinded hash is known from the upper subtrie.
    fn reveal_node(
        &mut self,
        path: Nibbles,
        node: &TrieNodeV2,
        masks: Option<BranchNodeMasks>,
        hash_from_upper: Option<B256>,
    ) -> SparseTrieResult<bool> {
        debug_assert!(path.starts_with(&self.path));

        // If the node is already revealed, do nothing.
        if self.nodes.contains_key(&path) {
            return Ok(false);
        }

        // If the hash is provided from the upper subtrie, use it. Otherwise, find the parent branch
        // node, unset its blinded bit and use the hash.
        let hash = if let Some(hash) = hash_from_upper {
            Some(hash)
        } else if path.len() != UPPER_TRIE_MAX_DEPTH && !path.is_empty() {
            let Some(SparseNode::Branch { state_mask, blinded_mask, blinded_hashes, .. }) =
                self.nodes.get_mut(&path.slice(0..path.len() - 1))
            else {
                return Ok(false);
            };
            let nibble = path.last().unwrap();
            if !state_mask.is_bit_set(nibble) {
                return Ok(false);
            }

            blinded_mask.is_bit_set(nibble).then(|| {
                blinded_mask.unset_bit(nibble);
                blinded_hashes[nibble as usize]
            })
        } else {
            None
        };

        trace!(
            target: "trie::parallel_sparse",
            ?path,
            ?node,
            ?masks,
            "Revealing node",
        );

        match node {
            TrieNodeV2::EmptyRoot => {
                // For an empty root, ensure that we are at the root path, and at the upper subtrie.
                debug_assert!(path.is_empty());
                debug_assert!(self.path.is_empty());
                self.nodes.insert(path, SparseNode::Empty);
            }
            TrieNodeV2::Branch(branch) => {
                if branch.key.is_empty() {
                    self.reveal_branch(
                        path,
                        branch.state_mask,
                        &branch.stack,
                        masks,
                        hash.as_ref().map(RlpNode::word_rlp),
                    )?;
                    return Ok(true);
                }

                self.nodes.insert(
                    path,
                    SparseNode::Extension {
                        key: branch.key,
                        state: hash
                            .as_ref()
                            .map(|hash| SparseNodeState::Cached {
                                rlp_node: RlpNode::word_rlp(hash),
                                // Inherit `store_in_db_trie` from the child branch
                                // node masks so that the memoized hash can be used
                                // without needing to fetch the child branch.
                                store_in_db_trie: Some(masks.is_some_and(|m| {
                                    !m.hash_mask.is_empty() || !m.tree_mask.is_empty()
                                })),
                            })
                            .unwrap_or(SparseNodeState::Dirty),
                    },
                );

                let mut branch_path = path;
                branch_path.extend(&branch.key);

                // Exit early if the actual branch node does not belong to this subtrie.
                if !Self::is_child_same_level(&path, &branch_path) {
                    return Ok(true);
                }

                // Reveal the actual branch node.
                self.reveal_branch(
                    branch_path,
                    branch.state_mask,
                    &branch.stack,
                    masks,
                    branch.branch_rlp_node.clone(),
                )?;
            }
            TrieNodeV2::Extension(_) => unreachable!(),
            TrieNodeV2::Leaf(leaf) => {
                // Skip the reachability check when path.len() == UPPER_TRIE_MAX_DEPTH because
                // at that boundary the leaf is in the lower subtrie but its parent branch is in
                // the upper subtrie. The subtrie cannot check connectivity across the upper/lower
                // boundary, so that check happens in `reveal_nodes` instead.
                if path.len() != UPPER_TRIE_MAX_DEPTH && !self.is_leaf_reachable_from_parent(&path)
                {
                    trace!(
                        target: "trie::parallel_sparse",
                        ?path,
                        "Leaf not reachable from parent branch, skipping",
                    );
                    return Ok(false)
                }

                let mut full_key = path;
                full_key.extend(&leaf.key);

                match self.inner.values.entry(full_key) {
                    Entry::Occupied(_) => {
                        trace!(
                            target: "trie::parallel_sparse",
                            ?path,
                            ?full_key,
                            "Leaf full key value already present, skipping",
                        );
                        return Ok(false)
                    }
                    Entry::Vacant(entry) => {
                        entry.insert(leaf.value.clone());
                    }
                }

                self.nodes.insert(
                    path,
                    SparseNode::Leaf {
                        key: leaf.key,
                        state: hash
                            .as_ref()
                            .map(|hash| SparseNodeState::Cached {
                                rlp_node: RlpNode::word_rlp(hash),
                                store_in_db_trie: Some(false),
                            })
                            .unwrap_or(SparseNodeState::Dirty),
                    },
                );
            }
        }

        Ok(true)
    }

    /// Recalculates and updates the RLP hashes for the changed nodes in this subtrie.
    ///
    /// The function starts from the subtrie root, traverses down to leaves, and then calculates
    /// the hashes from leaves back up to the root. It uses a stack from [`SparseSubtrieBuffers`] to
    /// track the traversal and accumulate RLP encodings.
    ///
    /// # Parameters
    ///
    /// - `prefix_set`: The set of trie paths whose nodes have changed.
    /// - `update_actions`: A buffer which `SparseTrieUpdatesAction`s will be written to in the
    ///   event that any changes to the top-level updates are required. If None then update
    ///   retention is disabled.
    /// - `branch_node_masks`: The tree and hash masks for branch nodes.
    ///
    /// # Returns
    ///
    /// A tuple containing the root node of the updated subtrie.
    ///
    /// # Panics
    ///
    /// If the node at the root path does not exist.
    #[instrument(level = "trace", target = "trie::parallel_sparse", skip_all, fields(root = ?self.path), ret)]
    fn update_hashes(
        &mut self,
        prefix_set: &mut PrefixSet,
        update_actions: &mut Option<Vec<SparseTrieUpdatesAction>>,
        branch_node_masks: &BranchNodeMasksMap,
    ) -> RlpNode {
        trace!(target: "trie::parallel_sparse", "Updating subtrie hashes");

        debug_assert!(prefix_set.iter().all(|path| path.starts_with(&self.path)));

        debug_assert!(self.inner.buffers.path_stack.is_empty());
        self.inner
            .buffers
            .path_stack
            .push(RlpNodePathStackItem { path: self.path, is_in_prefix_set: None });

        while let Some(stack_item) = self.inner.buffers.path_stack.pop() {
            let path = stack_item.path;
            let node = self
                .nodes
                .get_mut(&path)
                .unwrap_or_else(|| panic!("node at path {path:?} does not exist"));

            self.inner.rlp_node(prefix_set, update_actions, stack_item, node, branch_node_masks);
        }

        debug_assert_eq!(self.inner.buffers.rlp_node_stack.len(), 1);
        self.inner.buffers.rlp_node_stack.pop().unwrap().rlp_node
    }

    /// Removes all nodes and values from the subtrie, resetting it to a blank state
    /// with only an empty root node. This is used when a storage root is deleted.
    fn wipe(&mut self) {
        self.nodes.clear();
        self.nodes.insert(Nibbles::default(), SparseNode::Empty);
        self.inner.clear();
    }

    /// Clears the subtrie, keeping the data structures allocated.
    pub(crate) fn clear(&mut self) {
        self.nodes.clear();
        self.inner.clear();
    }

    /// Shrinks the capacity of the subtrie's node storage.
    pub(crate) fn shrink_nodes_to(&mut self, size: usize) {
        self.nodes.shrink_to(size);
    }

    /// Shrinks the capacity of the subtrie's value storage.
    pub(crate) fn shrink_values_to(&mut self, size: usize) {
        self.inner.values.shrink_to(size);
    }

    /// Returns a heuristic for the in-memory size of this subtrie in bytes.
    pub(crate) fn memory_size(&self) -> usize {
        let mut size = core::mem::size_of::<Self>();

        // Nodes map: key (Nibbles) + value (SparseNode)
        for (path, node) in &self.nodes {
            size += core::mem::size_of::<Nibbles>();
            size += path.len(); // Nibbles heap allocation
            size += node.memory_size();
        }

        // Values map: key (Nibbles) + value (Vec<u8>)
        for (path, value) in &self.inner.values {
            size += core::mem::size_of::<Nibbles>();
            size += path.len(); // Nibbles heap allocation
            size += core::mem::size_of::<Vec<u8>>() + value.capacity();
        }

        // Buffers
        size += self.inner.buffers.memory_size();

        size
    }
}

/// Helper type for [`SparseSubtrie`] to mutably access only a subset of fields from the original
/// struct.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
struct SparseSubtrieInner {
    /// Map from leaf key paths to their values.
    /// All values are stored here instead of directly in leaf nodes.
    values: HashMap<Nibbles, Vec<u8>>,
    /// Reusable buffers for [`SparseSubtrie::update_hashes`].
    buffers: SparseSubtrieBuffers,
}

impl SparseSubtrieInner {
    /// Computes the RLP encoding and its hash for a single (trie node)[`SparseNode`].
    ///
    /// # Deferred Processing
    ///
    /// When an extension or a branch node depends on child nodes that haven't been computed yet,
    /// the function pushes the current node back onto the path stack along with its children,
    /// then returns early. This allows the iterative algorithm to process children first before
    /// retrying the parent.
    ///
    /// # Parameters
    ///
    /// - `prefix_set`: Set of prefixes (key paths) that have been marked as updated
    /// - `update_actions`: A buffer which `SparseTrieUpdatesAction`s will be written to in the
    ///   event that any changes to the top-level updates are required. If None then update
    ///   retention is disabled.
    /// - `stack_item`: The stack item to process
    /// - `node`: The sparse node to process (will be mutated to update hash)
    /// - `branch_node_masks`: The tree and hash masks for branch nodes.
    ///
    /// # Side Effects
    ///
    /// - Updates the node's hash field after computing RLP
    /// - Pushes nodes to [`SparseSubtrieBuffers::path_stack`] to manage traversal
    /// - May push items onto the path stack for deferred processing
    ///
    /// # Exit condition
    ///
    /// Once all nodes have been processed and all RLPs and hashes calculated, pushes the root node
    /// onto the [`SparseSubtrieBuffers::rlp_node_stack`] and exits.
    fn rlp_node(
        &mut self,
        prefix_set: &mut PrefixSet,
        update_actions: &mut Option<Vec<SparseTrieUpdatesAction>>,
        mut stack_item: RlpNodePathStackItem,
        node: &mut SparseNode,
        branch_node_masks: &BranchNodeMasksMap,
    ) {
        let path = stack_item.path;
        trace!(
            target: "trie::parallel_sparse",
            ?path,
            ?node,
            "Calculating node RLP"
        );

        // Check if the path is in the prefix set.
        // First, check the cached value. If it's `None`, then check the prefix set, and update
        // the cached value.
        let mut prefix_set_contains = |path: &Nibbles| {
            *stack_item.is_in_prefix_set.get_or_insert_with(|| prefix_set.contains(path))
        };

        let (rlp_node, node_type) = match node {
            SparseNode::Empty => (RlpNode::word_rlp(&EMPTY_ROOT_HASH), SparseNodeType::Empty),
            SparseNode::Leaf { key, state } => {
                let mut path = path;
                path.extend(key);
                let value = self.values.get(&path);

                // Check if we should use cached RLP:
                // - If there's a cached RLP and the path is not in prefix_set, use cached
                // - If the value is not in this subtrie's values (e.g., lower subtrie leaf being
                //   processed via upper subtrie), we must use cached RLP
                let cached_rlp_node = state.cached_rlp_node();
                let use_cached =
                    cached_rlp_node.is_some() && (!prefix_set_contains(&path) || value.is_none());

                if let Some(rlp_node) = use_cached.then(|| cached_rlp_node.unwrap()) {
                    // Return the cached RLP
                    (rlp_node.clone(), SparseNodeType::Leaf)
                } else {
                    // Encode the leaf node and update its RlpNode
                    let value = value.expect("leaf value must exist in subtrie");
                    self.buffers.rlp_buf.clear();
                    let rlp_node = LeafNodeRef { key, value }.rlp(&mut self.buffers.rlp_buf);
                    *state = SparseNodeState::Cached {
                        rlp_node: rlp_node.clone(),
                        store_in_db_trie: Some(false),
                    };
                    trace!(
                        target: "trie::parallel_sparse",
                        ?path,
                        ?key,
                        value = %alloy_primitives::hex::encode(value),
                        ?rlp_node,
                        "Calculated leaf RLP node",
                    );
                    (rlp_node, SparseNodeType::Leaf)
                }
            }
            SparseNode::Extension { key, state } => {
                let mut child_path = path;
                child_path.extend(key);
                if let Some((rlp_node, store_in_db_trie)) = state
                    .cached_rlp_node()
                    .zip(state.store_in_db_trie())
                    .filter(|_| !prefix_set_contains(&path))
                {
                    // If the node is already computed, and the node path is not in
                    // the prefix set, return the pre-computed node
                    (
                        rlp_node.clone(),
                        SparseNodeType::Extension { store_in_db_trie: Some(store_in_db_trie) },
                    )
                } else if self.buffers.rlp_node_stack.last().is_some_and(|e| e.path == child_path) {
                    // Top of the stack has the child node, we can encode the extension node and
                    // update its hash
                    let RlpNodeStackItem { path: _, rlp_node: child, node_type: child_node_type } =
                        self.buffers.rlp_node_stack.pop().unwrap();
                    self.buffers.rlp_buf.clear();
                    let rlp_node =
                        ExtensionNodeRef::new(key, &child).rlp(&mut self.buffers.rlp_buf);

                    let store_in_db_trie_value = child_node_type.store_in_db_trie();

                    trace!(
                        target: "trie::parallel_sparse",
                        ?path,
                        ?child_path,
                        ?child_node_type,
                        "Extension node"
                    );

                    *state = SparseNodeState::Cached {
                        rlp_node: rlp_node.clone(),
                        store_in_db_trie: store_in_db_trie_value,
                    };

                    (
                        rlp_node,
                        SparseNodeType::Extension {
                            // Inherit the `store_in_db_trie` flag from the child node, which is
                            // always the branch node
                            store_in_db_trie: store_in_db_trie_value,
                        },
                    )
                } else {
                    // Need to defer processing until child is computed, on the next
                    // invocation update the node's hash.
                    self.buffers.path_stack.extend([
                        RlpNodePathStackItem {
                            path,
                            is_in_prefix_set: Some(prefix_set_contains(&path)),
                        },
                        RlpNodePathStackItem { path: child_path, is_in_prefix_set: None },
                    ]);
                    return
                }
            }
            SparseNode::Branch { state_mask, state, blinded_mask, blinded_hashes } => {
                if let Some((rlp_node, store_in_db_trie)) = state
                    .cached_rlp_node()
                    .zip(state.store_in_db_trie())
                    .filter(|_| !prefix_set_contains(&path))
                {
                    let node_type =
                        SparseNodeType::Branch { store_in_db_trie: Some(store_in_db_trie) };

                    trace!(
                        target: "trie::parallel_sparse",
                        ?path,
                        ?node_type,
                        ?rlp_node,
                        "Adding node to RLP node stack (cached branch)"
                    );

                    // If the node hash is already computed, and the node path is not in
                    // the prefix set, return the pre-computed hash
                    self.buffers.rlp_node_stack.push(RlpNodeStackItem {
                        path,
                        rlp_node: rlp_node.clone(),
                        node_type,
                    });
                    return
                }

                let retain_updates = update_actions.is_some() && prefix_set_contains(&path);

                self.buffers.branch_child_buf.clear();
                // Walk children in a reverse order from `f` to `0`, so we pop the `0` first
                // from the stack and keep walking in the sorted order.
                for bit in state_mask.iter().rev() {
                    let mut child = path;
                    child.push_unchecked(bit);

                    if !blinded_mask.is_bit_set(bit) {
                        self.buffers.branch_child_buf.push(child);
                    }
                }

                self.buffers.branch_value_stack_buf.resize(state_mask.len(), Default::default());

                let mut tree_mask = TrieMask::default();
                let mut hash_mask = TrieMask::default();
                let mut hashes = Vec::new();

                // Lazy lookup for branch node masks - shared across loop iterations
                let mut path_masks_storage = None;
                let mut path_masks =
                    || *path_masks_storage.get_or_insert_with(|| branch_node_masks.get(&path));

                for (i, child_nibble) in state_mask.iter().enumerate().rev() {
                    let mut child_path = path;
                    child_path.push_unchecked(child_nibble);

                    let (child, child_node_type) = if blinded_mask.is_bit_set(child_nibble) {
                        (
                            RlpNode::word_rlp(&blinded_hashes[child_nibble as usize]),
                            SparseNodeType::Hash,
                        )
                    } else if self
                        .buffers
                        .rlp_node_stack
                        .last()
                        .is_some_and(|e| e.path == child_path)
                    {
                        let RlpNodeStackItem { path: _, rlp_node, node_type } =
                            self.buffers.rlp_node_stack.pop().unwrap();

                        (rlp_node, node_type)
                    } else {
                        // Need to defer processing until children are computed, on the next
                        // invocation update the node's hash.
                        self.buffers.path_stack.push(RlpNodePathStackItem {
                            path,
                            is_in_prefix_set: Some(prefix_set_contains(&path)),
                        });
                        self.buffers.path_stack.extend(
                            self.buffers
                                .branch_child_buf
                                .drain(..)
                                .map(|path| RlpNodePathStackItem { path, is_in_prefix_set: None }),
                        );
                        return
                    };

                    // Update the masks only if we need to retain trie updates
                    if retain_updates {
                        // Determine whether we need to set trie mask bit.
                        let should_set_tree_mask_bit =
                            if let Some(store_in_db_trie) = child_node_type.store_in_db_trie() {
                                // A branch or an extension node explicitly set the
                                // `store_in_db_trie` flag
                                store_in_db_trie
                            } else {
                                // A blinded node has the tree mask bit set
                                child_node_type.is_hash() &&
                                    path_masks().is_some_and(|masks| {
                                        masks.tree_mask.is_bit_set(child_nibble)
                                    })
                            };
                        if should_set_tree_mask_bit {
                            tree_mask.set_bit(child_nibble);
                        }
                        // Set the hash mask. If a child node is a revealed branch node OR
                        // is a blinded node that has its hash mask bit set according to the
                        // database, set the hash mask bit and save the hash.
                        let hash = child.as_hash().filter(|_| {
                            child_node_type.is_branch() ||
                                (child_node_type.is_hash() &&
                                    path_masks().is_some_and(|masks| {
                                        masks.hash_mask.is_bit_set(child_nibble)
                                    }))
                        });
                        if let Some(hash) = hash {
                            hash_mask.set_bit(child_nibble);
                            hashes.push(hash);
                        }
                    }

                    // Insert children in the resulting buffer in a normal order,
                    // because initially we iterated in reverse.
                    // SAFETY: i < len and len is never 0
                    self.buffers.branch_value_stack_buf[i] = child;
                }

                trace!(
                    target: "trie::parallel_sparse",
                    ?path,
                    ?tree_mask,
                    ?hash_mask,
                    "Branch node masks"
                );

                // Top of the stack has all children node, we can encode the branch node and
                // update its hash
                self.buffers.rlp_buf.clear();
                let branch_node_ref =
                    BranchNodeRef::new(&self.buffers.branch_value_stack_buf, *state_mask);
                let rlp_node = branch_node_ref.rlp(&mut self.buffers.rlp_buf);

                // Save a branch node update only if it's not a root node, and we need to
                // persist updates.
                let store_in_db_trie_value = if let Some(update_actions) =
                    update_actions.as_mut().filter(|_| retain_updates && !path.is_empty())
                {
                    let store_in_db_trie = !tree_mask.is_empty() || !hash_mask.is_empty();
                    if store_in_db_trie {
                        // Store in DB trie if there are either any children that are stored in
                        // the DB trie, or any children represent hashed values
                        hashes.reverse();
                        let branch_node =
                            BranchNodeCompact::new(*state_mask, tree_mask, hash_mask, hashes, None);
                        update_actions
                            .push(SparseTrieUpdatesAction::InsertUpdated(path, branch_node));
                    } else {
                        // New tree and hash masks are empty - check previous state
                        let prev_had_masks = path_masks()
                            .is_some_and(|m| !m.tree_mask.is_empty() || !m.hash_mask.is_empty());
                        if prev_had_masks {
                            // Previously had masks, now empty - mark as removed
                            update_actions.push(SparseTrieUpdatesAction::InsertRemoved(path));
                        } else {
                            // Previously empty too - just remove the update
                            update_actions.push(SparseTrieUpdatesAction::RemoveUpdated(path));
                        }
                    }

                    store_in_db_trie
                } else {
                    false
                };

                *state = SparseNodeState::Cached {
                    rlp_node: rlp_node.clone(),
                    store_in_db_trie: Some(store_in_db_trie_value),
                };

                (
                    rlp_node,
                    SparseNodeType::Branch { store_in_db_trie: Some(store_in_db_trie_value) },
                )
            }
        };

        trace!(
            target: "trie::parallel_sparse",
            ?path,
            ?node_type,
            ?rlp_node,
            "Adding node to RLP node stack"
        );
        self.buffers.rlp_node_stack.push(RlpNodeStackItem { path, rlp_node, node_type });
    }

    /// Clears the subtrie, keeping the data structures allocated.
    fn clear(&mut self) {
        self.values.clear();
        self.buffers.clear();
    }
}

/// Represents the outcome of processing a node during leaf insertion
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum LeafUpdateStep {
    /// Continue traversing to the next node
    Continue,
    /// Update is complete with nodes inserted
    Complete {
        /// The node paths that were inserted during this step
        inserted_nodes: Vec<Nibbles>,
    },
    /// The node was not found
    #[default]
    NodeNotFound,
}

impl LeafUpdateStep {
    /// Creates a step indicating completion with inserted nodes
    pub const fn complete_with_insertions(inserted_nodes: Vec<Nibbles>) -> Self {
        Self::Complete { inserted_nodes }
    }
}

/// Sparse Subtrie Type.
///
/// Used to determine the type of subtrie a certain path belongs to:
/// - Paths in the range `0x..=0xf` belong to the upper subtrie.
/// - Paths in the range `0x00..` belong to one of the lower subtries. The index of the lower
///   subtrie is determined by the first [`UPPER_TRIE_MAX_DEPTH`] nibbles of the path.
///
/// There can be at most [`NUM_LOWER_SUBTRIES`] lower subtries.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SparseSubtrieType {
    /// Upper subtrie with paths in the range `0x..=0xf`
    Upper,
    /// Lower subtrie with paths in the range `0x00..`. Includes the index of the subtrie,
    /// according to the path prefix.
    Lower(usize),
}

impl SparseSubtrieType {
    /// Returns true if a node at a path of the given length would be placed in the upper subtrie.
    ///
    /// Nodes with paths shorter than [`UPPER_TRIE_MAX_DEPTH`] nibbles belong to the upper subtrie,
    /// while longer paths belong to the lower subtries.
    pub const fn path_len_is_upper(len: usize) -> bool {
        len < UPPER_TRIE_MAX_DEPTH
    }

    /// Returns the type of subtrie based on the given path.
    pub fn from_path(path: &Nibbles) -> Self {
        if Self::path_len_is_upper(path.len()) {
            Self::Upper
        } else {
            Self::Lower(path_subtrie_index_unchecked(path))
        }
    }

    /// Returns the index of the lower subtrie, if it exists.
    pub const fn lower_index(&self) -> Option<usize> {
        match self {
            Self::Upper => None,
            Self::Lower(index) => Some(*index),
        }
    }
}

impl Ord for SparseSubtrieType {
    /// Orders two [`SparseSubtrieType`]s such that `Upper` is less than `Lower(_)`, and `Lower`s
    /// are ordered by their index.
    fn cmp(&self, other: &Self) -> Ordering {
        match (self, other) {
            (Self::Upper, Self::Upper) => Ordering::Equal,
            (Self::Upper, Self::Lower(_)) => Ordering::Less,
            (Self::Lower(_), Self::Upper) => Ordering::Greater,
            (Self::Lower(idx_a), Self::Lower(idx_b)) if idx_a == idx_b => Ordering::Equal,
            (Self::Lower(idx_a), Self::Lower(idx_b)) => idx_a.cmp(idx_b),
        }
    }
}

impl PartialOrd for SparseSubtrieType {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Collection of reusable buffers for calculating subtrie hashes.
///
/// These buffers reduce allocations when computing RLP representations during trie updates.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct SparseSubtrieBuffers {
    /// Stack of RLP node paths
    path_stack: Vec<RlpNodePathStackItem>,
    /// Stack of RLP nodes
    rlp_node_stack: Vec<RlpNodeStackItem>,
    /// Reusable branch child path
    branch_child_buf: Vec<Nibbles>,
    /// Reusable branch value stack
    branch_value_stack_buf: Vec<RlpNode>,
    /// Reusable RLP buffer
    rlp_buf: Vec<u8>,
}

impl SparseSubtrieBuffers {
    /// Clears all buffers.
    fn clear(&mut self) {
        self.path_stack.clear();
        self.rlp_node_stack.clear();
        self.branch_child_buf.clear();
        self.branch_value_stack_buf.clear();
        self.rlp_buf.clear();
    }

    /// Returns a heuristic for the in-memory size of these buffers in bytes.
    const fn memory_size(&self) -> usize {
        let mut size = core::mem::size_of::<Self>();

        size += self.path_stack.capacity() * core::mem::size_of::<RlpNodePathStackItem>();
        size += self.rlp_node_stack.capacity() * core::mem::size_of::<RlpNodeStackItem>();
        size += self.branch_child_buf.capacity() * core::mem::size_of::<Nibbles>();
        size += self.branch_value_stack_buf.capacity() * core::mem::size_of::<RlpNode>();
        size += self.rlp_buf.capacity();

        size
    }
}

/// RLP node path stack item.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RlpNodePathStackItem {
    /// Path to the node.
    pub path: Nibbles,
    /// Whether the path is in the prefix set. If [`None`], then unknown yet.
    pub is_in_prefix_set: Option<bool>,
}

/// Changed subtrie.
#[derive(Debug)]
struct ChangedSubtrie {
    /// Lower subtrie index in the range [0, [`NUM_LOWER_SUBTRIES`]).
    index: usize,
    /// Changed subtrie
    subtrie: Box<SparseSubtrie>,
    /// Prefix set of keys that belong to the subtrie.
    prefix_set: PrefixSet,
    /// Reusable buffer for collecting [`SparseTrieUpdatesAction`]s during computations. Will be
    /// None if update retention is disabled.
    update_actions_buf: Option<Vec<SparseTrieUpdatesAction>>,
}

/// Convert first [`UPPER_TRIE_MAX_DEPTH`] nibbles of the path into a lower subtrie index in the
/// range [0, [`NUM_LOWER_SUBTRIES`]).
///
/// # Panics
///
/// If the path is shorter than [`UPPER_TRIE_MAX_DEPTH`] nibbles.
fn path_subtrie_index_unchecked(path: &Nibbles) -> usize {
    debug_assert_eq!(UPPER_TRIE_MAX_DEPTH, 2);
    let idx = path.get_byte_unchecked(0) as usize;
    // SAFETY: always true.
    unsafe { core::hint::assert_unchecked(idx < NUM_LOWER_SUBTRIES) };
    idx
}

/// Checks if `path` is a strict descendant of any root in a sorted slice.
///
/// Uses binary search to find the candidate root that could be an ancestor.
/// Returns `true` if `path` starts with a root and is longer (strict descendant).
fn is_strict_descendant_in(roots: &[Nibbles], path: &Nibbles) -> bool {
    if roots.is_empty() {
        return false;
    }
    debug_assert!(roots.windows(2).all(|w| w[0] <= w[1]), "roots must be sorted by path");
    let idx = roots.partition_point(|root| root <= path);
    if idx > 0 {
        let candidate = &roots[idx - 1];
        if path.starts_with(candidate) && path.len() > candidate.len() {
            return true;
        }
    }
    false
}

/// Checks if `path` starts with any root in a sorted slice (inclusive).
///
/// Uses binary search to find the candidate root that could be a prefix.
/// Returns `true` if `path` starts with a root (including exact match).
fn starts_with_pruned_in(roots: &[Nibbles], path: &Nibbles) -> bool {
    if roots.is_empty() {
        return false;
    }
    debug_assert!(roots.windows(2).all(|w| w[0] <= w[1]), "roots must be sorted by path");
    let idx = roots.partition_point(|root| root <= path);
    if idx > 0 {
        let candidate = &roots[idx - 1];
        if path.starts_with(candidate) {
            return true;
        }
    }
    false
}

/// Used by lower subtries to communicate updates to the top-level [`SparseTrieUpdates`] set.
#[derive(Clone, Debug, Eq, PartialEq)]
enum SparseTrieUpdatesAction {
    /// Remove the path from the `updated_nodes`, if it was present, and add it to `removed_nodes`.
    InsertRemoved(Nibbles),
    /// Remove the path from the `updated_nodes`, if it was present, leaving `removed_nodes`
    /// unaffected.
    RemoveUpdated(Nibbles),
    /// Insert the branch node into `updated_nodes`.
    InsertUpdated(Nibbles, BranchNodeCompact),
}

#[cfg(test)]
mod tests {
    use super::{
        path_subtrie_index_unchecked, LowerSparseSubtrie, ParallelSparseTrie, SparseSubtrie,
        SparseSubtrieType,
    };
    use crate::{
        parallel::ChangedSubtrie,
        provider::{DefaultTrieNodeProvider, NoRevealProvider},
        trie::SparseNodeState,
        LeafLookup, LeafLookupError, SparseNode, SparseTrie, SparseTrieUpdates,
    };
    use alloy_primitives::{
        b256, hex,
        map::{B256Set, HashMap},
        B256, U256,
    };
    use alloy_rlp::{Decodable, Encodable};
    use alloy_trie::{proof::AddedRemovedKeys, BranchNodeCompact, Nibbles};
    use assert_matches::assert_matches;
    use itertools::Itertools;
    use proptest::{prelude::*, sample::SizeRange};
    use proptest_arbitrary_interop::arb;
    use reth_execution_errors::SparseTrieErrorKind;
    use reth_primitives_traits::Account;
    use reth_provider::{
        test_utils::create_test_provider_factory, StorageSettingsCache, TrieWriter,
    };
    use reth_trie::{
        hashed_cursor::{noop::NoopHashedCursor, HashedPostStateCursor},
        node_iter::{TrieElement, TrieNodeIter},
        trie_cursor::{noop::NoopAccountTrieCursor, TrieCursor, TrieCursorFactory},
        walker::TrieWalker,
        HashedPostState,
    };
    use reth_trie_common::{
        prefix_set::PrefixSetMut,
        proof::{ProofNodes, ProofRetainer},
        updates::TrieUpdates,
        BranchNodeMasks, BranchNodeMasksMap, BranchNodeRef, BranchNodeV2, ExtensionNode,
        HashBuilder, LeafNode, ProofTrieNodeV2, RlpNode, TrieMask, TrieNode, TrieNodeV2,
        EMPTY_ROOT_HASH,
    };
    use reth_trie_db::DatabaseTrieCursorFactory;
    use std::collections::{BTreeMap, BTreeSet};

    /// Pad nibbles to the length of a B256 hash with zeros on the right.
    fn pad_nibbles_right(mut nibbles: Nibbles) -> Nibbles {
        nibbles.extend(&Nibbles::from_nibbles_unchecked(vec![
            0;
            B256::len_bytes() * 2 - nibbles.len()
        ]));
        nibbles
    }

    /// Create a leaf key (suffix) for a leaf at a given position depth.
    /// `suffix` contains the non-zero nibbles, padded with zeros to reach `total_len`.
    fn leaf_key(suffix: impl AsRef<[u8]>, total_len: usize) -> Nibbles {
        let suffix = suffix.as_ref();
        let mut nibbles = Nibbles::from_nibbles(suffix);
        nibbles.extend(&Nibbles::from_nibbles_unchecked(vec![0; total_len - suffix.len()]));
        nibbles
    }

    fn create_account(nonce: u64) -> Account {
        Account { nonce, ..Default::default() }
    }

    fn large_account_value() -> Vec<u8> {
        let account = Account {
            nonce: 0x123456789abcdef,
            balance: U256::from(0x123456789abcdef0123456789abcdef_u128),
            ..Default::default()
        };
        let mut buf = Vec::new();
        account.into_trie_account(EMPTY_ROOT_HASH).encode(&mut buf);
        buf
    }

    fn encode_account_value(nonce: u64) -> Vec<u8> {
        let account = Account { nonce, ..Default::default() };
        let trie_account = account.into_trie_account(EMPTY_ROOT_HASH);
        let mut buf = Vec::new();
        trie_account.encode(&mut buf);
        buf
    }

    /// Test context that provides helper methods for trie testing
    #[derive(Default)]
    struct ParallelSparseTrieTestContext;

    impl ParallelSparseTrieTestContext {
        /// Assert that a lower subtrie exists at the given path
        fn assert_subtrie_exists(&self, trie: &ParallelSparseTrie, path: &Nibbles) {
            let idx = path_subtrie_index_unchecked(path);
            assert!(
                trie.lower_subtries[idx].as_revealed_ref().is_some(),
                "Expected lower subtrie at path {path:?} to exist",
            );
        }

        /// Get a lower subtrie, panicking if it doesn't exist
        fn get_subtrie<'a>(
            &self,
            trie: &'a ParallelSparseTrie,
            path: &Nibbles,
        ) -> &'a SparseSubtrie {
            let idx = path_subtrie_index_unchecked(path);
            trie.lower_subtries[idx]
                .as_revealed_ref()
                .unwrap_or_else(|| panic!("Lower subtrie at path {path:?} should exist"))
        }

        /// Assert that a lower subtrie has a specific path field value
        fn assert_subtrie_path(
            &self,
            trie: &ParallelSparseTrie,
            subtrie_prefix: impl AsRef<[u8]>,
            expected_path: impl AsRef<[u8]>,
        ) {
            let subtrie_prefix = Nibbles::from_nibbles(subtrie_prefix);
            let expected_path = Nibbles::from_nibbles(expected_path);
            let idx = path_subtrie_index_unchecked(&subtrie_prefix);

            let subtrie = trie.lower_subtries[idx].as_revealed_ref().unwrap_or_else(|| {
                panic!("Lower subtrie at prefix {subtrie_prefix:?} should exist")
            });

            assert_eq!(
                subtrie.path, expected_path,
                "Subtrie at prefix {subtrie_prefix:?} should have path {expected_path:?}, but has {:?}",
                subtrie.path
            );
        }

        /// Create test leaves with consecutive account values
        fn create_test_leaves(&self, paths: &[&[u8]]) -> Vec<(Nibbles, Vec<u8>)> {
            paths
                .iter()
                .enumerate()
                .map(|(i, path)| {
                    (
                        pad_nibbles_right(Nibbles::from_nibbles(path)),
                        encode_account_value(i as u64 + 1),
                    )
                })
                .collect()
        }

        /// Create a single test leaf with the given path and value nonce
        fn create_test_leaf(&self, path: impl AsRef<[u8]>, value_nonce: u64) -> (Nibbles, Vec<u8>) {
            (pad_nibbles_right(Nibbles::from_nibbles(path)), encode_account_value(value_nonce))
        }

        /// Update multiple leaves in the trie
        fn update_leaves(
            &self,
            trie: &mut ParallelSparseTrie,
            leaves: impl IntoIterator<Item = (Nibbles, Vec<u8>)>,
        ) {
            for (path, value) in leaves {
                trie.update_leaf(path, value, DefaultTrieNodeProvider).unwrap();
            }
        }

        /// Create an assertion builder for a subtrie
        fn assert_subtrie<'a>(
            &self,
            trie: &'a ParallelSparseTrie,
            path: Nibbles,
        ) -> SubtrieAssertion<'a> {
            self.assert_subtrie_exists(trie, &path);
            let subtrie = self.get_subtrie(trie, &path);
            SubtrieAssertion::new(subtrie)
        }

        /// Create an assertion builder for the upper subtrie
        fn assert_upper_subtrie<'a>(&self, trie: &'a ParallelSparseTrie) -> SubtrieAssertion<'a> {
            SubtrieAssertion::new(&trie.upper_subtrie)
        }

        /// Assert the root, trie updates, and nodes against the hash builder output.
        fn assert_with_hash_builder(
            &self,
            trie: &mut ParallelSparseTrie,
            hash_builder_root: B256,
            hash_builder_updates: TrieUpdates,
            hash_builder_proof_nodes: ProofNodes,
        ) {
            assert_eq!(trie.root(), hash_builder_root);
            pretty_assertions::assert_eq!(
                BTreeMap::from_iter(trie.updates_ref().updated_nodes.clone()),
                BTreeMap::from_iter(hash_builder_updates.account_nodes)
            );
            assert_eq_parallel_sparse_trie_proof_nodes(trie, hash_builder_proof_nodes);
        }
    }

    /// Assertion builder for subtrie structure
    struct SubtrieAssertion<'a> {
        subtrie: &'a SparseSubtrie,
    }

    impl<'a> SubtrieAssertion<'a> {
        fn new(subtrie: &'a SparseSubtrie) -> Self {
            Self { subtrie }
        }

        fn has_branch(self, path: &Nibbles, expected_mask_bits: &[u8]) -> Self {
            match self.subtrie.nodes.get(path) {
                Some(SparseNode::Branch { state_mask, .. }) => {
                    for bit in expected_mask_bits {
                        assert!(
                            state_mask.is_bit_set(*bit),
                            "Expected branch at {path:?} to have bit {bit} set, instead mask is: {state_mask:?}",
                        );
                    }
                }
                node => panic!("Expected branch node at {path:?}, found {node:?}"),
            }
            self
        }

        fn has_leaf(self, path: &Nibbles, expected_key: &Nibbles) -> Self {
            match self.subtrie.nodes.get(path) {
                Some(SparseNode::Leaf { key, .. }) => {
                    assert_eq!(
                        *key, *expected_key,
                        "Expected leaf at {path:?} to have key {expected_key:?}, found {key:?}",
                    );
                }
                node => panic!("Expected leaf node at {path:?}, found {node:?}"),
            }
            self
        }

        fn has_extension(self, path: &Nibbles, expected_key: &Nibbles) -> Self {
            match self.subtrie.nodes.get(path) {
                Some(SparseNode::Extension { key, .. }) => {
                    assert_eq!(
                        *key, *expected_key,
                        "Expected extension at {path:?} to have key {expected_key:?}, found {key:?}",
                    );
                }
                node => panic!("Expected extension node at {path:?}, found {node:?}"),
            }
            self
        }

        fn has_value(self, path: &Nibbles, expected_value: &[u8]) -> Self {
            let actual = self.subtrie.inner.values.get(path);
            assert_eq!(
                actual.map(|v| v.as_slice()),
                Some(expected_value),
                "Expected value at {path:?} to be {expected_value:?}, found {actual:?}",
            );
            self
        }

        fn has_no_value(self, path: &Nibbles) -> Self {
            let actual = self.subtrie.inner.values.get(path);
            assert!(actual.is_none(), "Expected no value at {path:?}, but found {actual:?}");
            self
        }
    }

    fn create_leaf_node(key: impl AsRef<[u8]>, value_nonce: u64) -> TrieNodeV2 {
        TrieNodeV2::Leaf(LeafNode::new(
            Nibbles::from_nibbles(key),
            encode_account_value(value_nonce),
        ))
    }

    fn create_branch_node(
        key: Nibbles,
        children_indices: &[u8],
        child_hashes: impl IntoIterator<Item = RlpNode>,
    ) -> TrieNodeV2 {
        let mut stack = Vec::new();
        let mut state_mask = TrieMask::default();

        for (&idx, hash) in children_indices.iter().zip(child_hashes.into_iter()) {
            state_mask.set_bit(idx);
            stack.push(hash);
        }

        let branch_rlp_node = if key.is_empty() {
            None
        } else {
            Some(RlpNode::from_rlp(&alloy_rlp::encode(BranchNodeRef::new(&stack, state_mask))))
        };

        TrieNodeV2::Branch(BranchNodeV2::new(key, stack, state_mask, branch_rlp_node))
    }

    fn create_branch_node_with_children(
        children_indices: &[u8],
        child_hashes: impl IntoIterator<Item = RlpNode>,
    ) -> TrieNodeV2 {
        create_branch_node(Nibbles::default(), children_indices, child_hashes)
    }

    /// Calculate the state root by feeding the provided state to the hash builder and retaining the
    /// proofs for the provided targets.
    ///
    /// Returns the state root and the retained proof nodes.
    fn run_hash_builder(
        state: impl IntoIterator<Item = (Nibbles, Account)> + Clone,
        trie_cursor: impl TrieCursor,
        destroyed_accounts: B256Set,
        proof_targets: impl IntoIterator<Item = Nibbles>,
    ) -> (B256, TrieUpdates, ProofNodes, HashMap<Nibbles, TrieMask>, HashMap<Nibbles, TrieMask>)
    {
        let mut account_rlp = Vec::new();

        let mut hash_builder = HashBuilder::default()
            .with_updates(true)
            .with_proof_retainer(ProofRetainer::from_iter(proof_targets).with_added_removed_keys(
                Some(AddedRemovedKeys::default().with_assume_added(true)),
            ));

        let mut prefix_set = PrefixSetMut::default();
        prefix_set.extend_keys(state.clone().into_iter().map(|(nibbles, _)| nibbles));
        prefix_set.extend_keys(destroyed_accounts.iter().map(Nibbles::unpack));
        let walker = TrieWalker::<_>::state_trie(trie_cursor, prefix_set.freeze())
            .with_deletions_retained(true);
        let hashed_post_state = HashedPostState::default()
            .with_accounts(state.into_iter().map(|(nibbles, account)| {
                (nibbles.pack().into_inner().unwrap().into(), Some(account))
            }))
            .into_sorted();
        let mut node_iter = TrieNodeIter::state_trie(
            walker,
            HashedPostStateCursor::new_account(
                NoopHashedCursor::<Account>::default(),
                &hashed_post_state,
            ),
        );

        while let Some(node) = node_iter.try_next().unwrap() {
            match node {
                TrieElement::Branch(branch) => {
                    hash_builder.add_branch(branch.key, branch.value, branch.children_are_in_trie);
                }
                TrieElement::Leaf(key, account) => {
                    let account = account.into_trie_account(EMPTY_ROOT_HASH);
                    account.encode(&mut account_rlp);

                    hash_builder.add_leaf(Nibbles::unpack(key), &account_rlp);
                    account_rlp.clear();
                }
            }
        }
        let root = hash_builder.root();
        let proof_nodes = hash_builder.take_proof_nodes();
        let branch_node_hash_masks = hash_builder
            .updated_branch_nodes
            .clone()
            .unwrap_or_default()
            .iter()
            .map(|(path, node)| (*path, node.hash_mask))
            .collect();
        let branch_node_tree_masks = hash_builder
            .updated_branch_nodes
            .clone()
            .unwrap_or_default()
            .iter()
            .map(|(path, node)| (*path, node.tree_mask))
            .collect();

        let mut trie_updates = TrieUpdates::default();
        let removed_keys = node_iter.walker.take_removed_keys();
        trie_updates.finalize(hash_builder, removed_keys, destroyed_accounts);

        (root, trie_updates, proof_nodes, branch_node_hash_masks, branch_node_tree_masks)
    }

    /// Returns a `ParallelSparseTrie` pre-loaded with the given nodes, as well as leaf values
    /// inferred from any provided leaf nodes.
    fn new_test_trie<Nodes>(nodes: Nodes) -> ParallelSparseTrie
    where
        Nodes: Iterator<Item = (Nibbles, SparseNode)>,
    {
        let mut trie = ParallelSparseTrie::default().with_updates(true);

        for (path, node) in nodes {
            let subtrie = trie.subtrie_for_path_mut(&path);
            if let SparseNode::Leaf { key, .. } = &node {
                let mut full_key = path;
                full_key.extend(key);
                subtrie.inner.values.insert(full_key, "LEAF VALUE".into());
            }
            subtrie.nodes.insert(path, node);
        }
        trie
    }

    fn parallel_sparse_trie_nodes(
        sparse_trie: &ParallelSparseTrie,
    ) -> impl IntoIterator<Item = (&Nibbles, &SparseNode)> {
        let lower_sparse_nodes = sparse_trie
            .lower_subtries
            .iter()
            .filter_map(|subtrie| subtrie.as_revealed_ref())
            .flat_map(|subtrie| subtrie.nodes.iter());

        let upper_sparse_nodes = sparse_trie.upper_subtrie.nodes.iter();

        lower_sparse_nodes.chain(upper_sparse_nodes).sorted_by_key(|(path, _)| *path)
    }

    /// Assert that the parallel sparse trie nodes and the proof nodes from the hash builder are
    /// equal.
    fn assert_eq_parallel_sparse_trie_proof_nodes(
        sparse_trie: &ParallelSparseTrie,
        proof_nodes: ProofNodes,
    ) {
        let proof_nodes = proof_nodes
            .into_nodes_sorted()
            .into_iter()
            .map(|(path, node)| (path, TrieNodeV2::decode(&mut node.as_ref()).unwrap()));

        let all_sparse_nodes = parallel_sparse_trie_nodes(sparse_trie);

        for ((proof_node_path, proof_node), (sparse_node_path, sparse_node)) in
            proof_nodes.zip(all_sparse_nodes)
        {
            assert_eq!(&proof_node_path, sparse_node_path);

            let equals = match (&proof_node, &sparse_node) {
                // Both nodes are empty
                (TrieNodeV2::EmptyRoot, SparseNode::Empty) => true,
                // Both nodes are branches and have the same state mask
                (
                    TrieNodeV2::Branch(BranchNodeV2 { state_mask: proof_state_mask, .. }),
                    SparseNode::Branch { state_mask: sparse_state_mask, .. },
                ) => proof_state_mask == sparse_state_mask,
                // Both nodes are extensions and have the same key
                (
                    TrieNodeV2::Extension(ExtensionNode { key: proof_key, .. }),
                    SparseNode::Extension { key: sparse_key, .. },
                ) |
                // Both nodes are leaves and have the same key
                (
                    TrieNodeV2::Leaf(LeafNode { key: proof_key, .. }),
                    SparseNode::Leaf { key: sparse_key, .. },
                ) => proof_key == sparse_key,
                // Empty and hash nodes are specific to the sparse trie, skip them
                (_, SparseNode::Empty) => continue,
                _ => false,
            };
            assert!(
                equals,
                "path: {proof_node_path:?}\nproof node: {proof_node:?}\nsparse node: {sparse_node:?}"
            );
        }
    }

    #[test]
    fn test_get_changed_subtries_empty() {
        let mut trie = ParallelSparseTrie::default();
        let mut prefix_set = PrefixSetMut::from([Nibbles::default()]).freeze();

        let (subtries, unchanged_prefix_set) = trie.take_changed_lower_subtries(&mut prefix_set);
        assert!(subtries.is_empty());
        assert_eq!(unchanged_prefix_set, PrefixSetMut::from(prefix_set.iter().copied()));
    }

    #[test]
    fn test_get_changed_subtries() {
        // Create a trie with three subtries
        let mut trie = ParallelSparseTrie::default();
        let subtrie_1 = Box::new(SparseSubtrie::new(Nibbles::from_nibbles([0x0, 0x0])));
        let subtrie_1_index = path_subtrie_index_unchecked(&subtrie_1.path);
        let subtrie_2 = Box::new(SparseSubtrie::new(Nibbles::from_nibbles([0x1, 0x0])));
        let subtrie_2_index = path_subtrie_index_unchecked(&subtrie_2.path);
        let subtrie_3 = Box::new(SparseSubtrie::new(Nibbles::from_nibbles([0x3, 0x0])));
        let subtrie_3_index = path_subtrie_index_unchecked(&subtrie_3.path);

        // Add subtries at specific positions
        trie.lower_subtries[subtrie_1_index] = LowerSparseSubtrie::Revealed(subtrie_1.clone());
        trie.lower_subtries[subtrie_2_index] = LowerSparseSubtrie::Revealed(subtrie_2.clone());
        trie.lower_subtries[subtrie_3_index] = LowerSparseSubtrie::Revealed(subtrie_3);

        let unchanged_prefix_set = PrefixSetMut::from([
            Nibbles::from_nibbles([0x0]),
            Nibbles::from_nibbles([0x2, 0x0, 0x0]),
        ]);
        // Create a prefix set with the keys that match only the second subtrie
        let mut prefix_set = PrefixSetMut::from([
            // Match second subtrie
            Nibbles::from_nibbles([0x1, 0x0, 0x0]),
            Nibbles::from_nibbles([0x1, 0x0, 0x1, 0x0]),
        ]);
        prefix_set.extend(unchanged_prefix_set);
        let mut prefix_set = prefix_set.freeze();

        // Second subtrie should be removed and returned
        let (subtries, unchanged_prefix_set) = trie.take_changed_lower_subtries(&mut prefix_set);
        assert_eq!(
            subtries
                .into_iter()
                .map(|ChangedSubtrie { index, subtrie, prefix_set, .. }| {
                    (index, subtrie, prefix_set.iter().copied().collect::<Vec<_>>())
                })
                .collect::<Vec<_>>(),
            vec![(
                subtrie_2_index,
                subtrie_2,
                vec![
                    Nibbles::from_nibbles([0x1, 0x0, 0x0]),
                    Nibbles::from_nibbles([0x1, 0x0, 0x1, 0x0])
                ]
            )]
        );
        assert_eq!(unchanged_prefix_set, unchanged_prefix_set);
        assert!(trie.lower_subtries[subtrie_2_index].as_revealed_ref().is_none());

        // First subtrie should remain unchanged
        assert_eq!(trie.lower_subtries[subtrie_1_index], LowerSparseSubtrie::Revealed(subtrie_1));
    }

    #[test]
    fn test_get_changed_subtries_all() {
        // Create a trie with three subtries
        let mut trie = ParallelSparseTrie::default();
        let subtrie_1 = Box::new(SparseSubtrie::new(Nibbles::from_nibbles([0x0, 0x0])));
        let subtrie_1_index = path_subtrie_index_unchecked(&subtrie_1.path);
        let subtrie_2 = Box::new(SparseSubtrie::new(Nibbles::from_nibbles([0x1, 0x0])));
        let subtrie_2_index = path_subtrie_index_unchecked(&subtrie_2.path);
        let subtrie_3 = Box::new(SparseSubtrie::new(Nibbles::from_nibbles([0x3, 0x0])));
        let subtrie_3_index = path_subtrie_index_unchecked(&subtrie_3.path);

        // Add subtries at specific positions
        trie.lower_subtries[subtrie_1_index] = LowerSparseSubtrie::Revealed(subtrie_1.clone());
        trie.lower_subtries[subtrie_2_index] = LowerSparseSubtrie::Revealed(subtrie_2.clone());
        trie.lower_subtries[subtrie_3_index] = LowerSparseSubtrie::Revealed(subtrie_3.clone());

        // Create a prefix set that matches any key
        let mut prefix_set = PrefixSetMut::all().freeze();

        // All subtries should be removed and returned
        let (subtries, unchanged_prefix_set) = trie.take_changed_lower_subtries(&mut prefix_set);
        assert_eq!(
            subtries
                .into_iter()
                .map(|ChangedSubtrie { index, subtrie, prefix_set, .. }| {
                    (index, subtrie, prefix_set.all())
                })
                .collect::<Vec<_>>(),
            vec![
                (subtrie_1_index, subtrie_1, true),
                (subtrie_2_index, subtrie_2, true),
                (subtrie_3_index, subtrie_3, true)
            ]
        );
        assert_eq!(unchanged_prefix_set, PrefixSetMut::all());

        assert!(trie.lower_subtries.iter().all(|subtrie| subtrie.as_revealed_ref().is_none()));
    }

    #[test]
    fn test_sparse_subtrie_type() {
        assert_eq!(SparseSubtrieType::from_path(&Nibbles::new()), SparseSubtrieType::Upper);
        assert_eq!(
            SparseSubtrieType::from_path(&Nibbles::from_nibbles([0])),
            SparseSubtrieType::Upper
        );
        assert_eq!(
            SparseSubtrieType::from_path(&Nibbles::from_nibbles([15])),
            SparseSubtrieType::Upper
        );
        assert_eq!(
            SparseSubtrieType::from_path(&Nibbles::from_nibbles([0, 0])),
            SparseSubtrieType::Lower(0)
        );
        assert_eq!(
            SparseSubtrieType::from_path(&Nibbles::from_nibbles([0, 0, 0])),
            SparseSubtrieType::Lower(0)
        );
        assert_eq!(
            SparseSubtrieType::from_path(&Nibbles::from_nibbles([0, 1])),
            SparseSubtrieType::Lower(1)
        );
        assert_eq!(
            SparseSubtrieType::from_path(&Nibbles::from_nibbles([0, 1, 0])),
            SparseSubtrieType::Lower(1)
        );
        assert_eq!(
            SparseSubtrieType::from_path(&Nibbles::from_nibbles([0, 15])),
            SparseSubtrieType::Lower(15)
        );
        assert_eq!(
            SparseSubtrieType::from_path(&Nibbles::from_nibbles([15, 0])),
            SparseSubtrieType::Lower(240)
        );
        assert_eq!(
            SparseSubtrieType::from_path(&Nibbles::from_nibbles([15, 1])),
            SparseSubtrieType::Lower(241)
        );
        assert_eq!(
            SparseSubtrieType::from_path(&Nibbles::from_nibbles([15, 15])),
            SparseSubtrieType::Lower(255)
        );
        assert_eq!(
            SparseSubtrieType::from_path(&Nibbles::from_nibbles([15, 15, 15])),
            SparseSubtrieType::Lower(255)
        );
    }

    #[test]
    fn test_reveal_node_leaves() {
        // Reveal leaf in the upper trie. A root branch with child 0x1 makes path [0x1]
        // reachable for the subsequent reveal_nodes call.
        let root_branch =
            create_branch_node_with_children(&[0x1], [RlpNode::word_rlp(&B256::repeat_byte(0xAA))]);
        let mut trie = ParallelSparseTrie::from_root(root_branch, None, false).unwrap();

        {
            let path = Nibbles::from_nibbles([0x1]);
            let node = create_leaf_node([0x2, 0x3], 42);
            let masks = None;

            trie.reveal_nodes(&mut [ProofTrieNodeV2 { path, node, masks }]).unwrap();

            assert_matches!(
                trie.upper_subtrie.nodes.get(&path),
                Some(SparseNode::Leaf { key, state: SparseNodeState::Cached { .. } })
                if key == &Nibbles::from_nibbles([0x2, 0x3])
            );

            let full_path = Nibbles::from_nibbles([0x1, 0x2, 0x3]);
            assert_eq!(
                trie.upper_subtrie.inner.values.get(&full_path),
                Some(&encode_account_value(42))
            );
        }

        // Reveal leaf in a lower trie. A separate trie is needed because the structure at
        // [0x1] conflicts: the upper trie test placed a leaf there, but reaching [0x1, 0x2]
        // requires a branch at [0x1]. A root branch → branch at [0x1] with child 0x2
        // makes path [0x1, 0x2] reachable.
        let root_branch =
            create_branch_node_with_children(&[0x1], [RlpNode::word_rlp(&B256::repeat_byte(0xAA))]);
        let branch_at_1 =
            create_branch_node_with_children(&[0x2], [RlpNode::word_rlp(&B256::repeat_byte(0xBB))]);
        let mut trie = ParallelSparseTrie::from_root(root_branch, None, false).unwrap();
        trie.reveal_nodes(&mut [ProofTrieNodeV2 {
            path: Nibbles::from_nibbles([0x1]),
            node: branch_at_1,
            masks: None,
        }])
        .unwrap();

        {
            let path = Nibbles::from_nibbles([0x1, 0x2]);
            let node = create_leaf_node([0x3, 0x4], 42);
            let masks = None;

            trie.reveal_nodes(&mut [ProofTrieNodeV2 { path, node, masks }]).unwrap();

            // Check that the lower subtrie was created
            let idx = path_subtrie_index_unchecked(&path);
            assert!(trie.lower_subtries[idx].as_revealed_ref().is_some());

            // Check that the lower subtrie's path was correctly set
            let lower_subtrie = trie.lower_subtries[idx].as_revealed_ref().unwrap();
            assert_eq!(lower_subtrie.path, path);

            assert_matches!(
                lower_subtrie.nodes.get(&path),
                Some(SparseNode::Leaf { key, state: SparseNodeState::Cached { .. } })
                if key == &Nibbles::from_nibbles([0x3, 0x4])
            );
        }

        // Reveal leaf in a lower trie with a longer path, shouldn't result in the subtrie's root
        // path changing.
        {
            let path = Nibbles::from_nibbles([0x1, 0x2, 0x3]);
            let node = create_leaf_node([0x4, 0x5], 42);
            let masks = None;

            trie.reveal_nodes(&mut [ProofTrieNodeV2 { path, node, masks }]).unwrap();

            // Check that the lower subtrie's path hasn't changed
            let idx = path_subtrie_index_unchecked(&path);
            let lower_subtrie = trie.lower_subtries[idx].as_revealed_ref().unwrap();
            assert_eq!(lower_subtrie.path, Nibbles::from_nibbles([0x1, 0x2]));
        }
    }

    #[test]
    fn test_reveal_node_branch_all_upper() {
        let path = Nibbles::new();
        let child_hashes = [
            RlpNode::word_rlp(&B256::repeat_byte(0x11)),
            RlpNode::word_rlp(&B256::repeat_byte(0x22)),
        ];
        let node = create_branch_node_with_children(&[0x0, 0x5], child_hashes.clone());
        let masks = None;
        let trie = ParallelSparseTrie::from_root(node, masks, true).unwrap();

        // Branch node should be in upper trie
        assert_eq!(
            trie.upper_subtrie.nodes.get(&path).unwrap(),
            &SparseNode::new_branch(
                0b0000000000100001.into(),
                &[(0, child_hashes[0].as_hash().unwrap()), (5, child_hashes[1].as_hash().unwrap())]
            )
        );

        // Children should not be revealed yet
        let child_path_0 = Nibbles::from_nibbles([0x0]);
        let child_path_5 = Nibbles::from_nibbles([0x5]);
        assert!(!trie.upper_subtrie.nodes.contains_key(&child_path_0));
        assert!(!trie.upper_subtrie.nodes.contains_key(&child_path_5));
    }

    #[test]
    fn test_reveal_node_branch_cross_level() {
        // Set up root branch with nibble 0x1 so path [0x1] is reachable.
        let root_branch =
            create_branch_node_with_children(&[0x1], [RlpNode::word_rlp(&B256::repeat_byte(0xAA))]);
        let mut trie = ParallelSparseTrie::from_root(root_branch, None, false).unwrap();

        let path = Nibbles::from_nibbles([0x1]); // Exactly 1 nibbles - boundary case
        let child_hashes = [
            RlpNode::word_rlp(&B256::repeat_byte(0x33)),
            RlpNode::word_rlp(&B256::repeat_byte(0x44)),
            RlpNode::word_rlp(&B256::repeat_byte(0x55)),
        ];
        let node = create_branch_node_with_children(&[0x0, 0x7, 0xf], child_hashes.clone());
        let masks = None;

        trie.reveal_nodes(&mut [ProofTrieNodeV2 { path, node, masks }]).unwrap();

        // Branch node should be in upper trie, hash is memoized from the previous Hash node
        assert_eq!(
            trie.upper_subtrie.nodes.get(&path).unwrap(),
            &SparseNode::new_branch(
                0b1000000010000001.into(),
                &[
                    (0x0, child_hashes[0].as_hash().unwrap()),
                    (0x7, child_hashes[1].as_hash().unwrap()),
                    (0xf, child_hashes[2].as_hash().unwrap())
                ]
            )
            .with_state(SparseNodeState::Cached {
                rlp_node: RlpNode::word_rlp(&B256::repeat_byte(0xAA)),
                store_in_db_trie: Some(false),
            })
        );

        // All children should be in lower tries since they have paths of length 3
        let child_paths = [
            Nibbles::from_nibbles([0x1, 0x0]),
            Nibbles::from_nibbles([0x1, 0x7]),
            Nibbles::from_nibbles([0x1, 0xf]),
        ];

        let mut children = child_paths
            .iter()
            .map(|path| ProofTrieNodeV2 {
                path: *path,
                node: create_leaf_node([0x0], 1),
                masks: None,
            })
            .collect::<Vec<_>>();

        trie.reveal_nodes(&mut children).unwrap();

        // Branch node should still be in upper trie but without any blinded children
        assert_matches!(
            trie.upper_subtrie.nodes.get(&path),
            Some(&SparseNode::Branch {
                state_mask,
                state: SparseNodeState::Cached { ref rlp_node, store_in_db_trie: Some(false) },
                blinded_mask,
                ..
            }) if state_mask == 0b1000000010000001.into() && blinded_mask.is_empty() && *rlp_node == RlpNode::word_rlp(&B256::repeat_byte(0xAA))
        );

        for (i, child_path) in child_paths.iter().enumerate() {
            let idx = path_subtrie_index_unchecked(child_path);
            let lower_subtrie = trie.lower_subtries[idx].as_revealed_ref().unwrap();
            assert_eq!(&lower_subtrie.path, child_path);
            assert_eq!(
                lower_subtrie.nodes.get(child_path),
                Some(&SparseNode::Leaf {
                    key: Nibbles::from_nibbles([0x0]),
                    state: SparseNodeState::Cached {
                        rlp_node: child_hashes[i].clone(),
                        store_in_db_trie: Some(false)
                    }
                })
            );
        }
    }

    #[test]
    fn test_update_subtrie_hashes_prefix_set_matching() {
        // Create a trie with a root branch that makes paths [0x0, ...] and [0x3, ...]
        // reachable from the upper trie.
        let root_branch = create_branch_node_with_children(
            &[0x0, 0x3],
            [
                RlpNode::word_rlp(&B256::repeat_byte(0xAA)),
                RlpNode::word_rlp(&B256::repeat_byte(0xBB)),
            ],
        );
        let mut trie = ParallelSparseTrie::from_root(root_branch, None, false).unwrap();

        // Create leaf paths.
        let leaf_1_full_path = Nibbles::from_nibbles([0; 64]);
        let leaf_1_path = leaf_1_full_path.slice(..2);
        let leaf_1_key = leaf_1_full_path.slice(2..);
        let leaf_2_full_path = Nibbles::from_nibbles([vec![0, 1], vec![0; 62]].concat());
        let leaf_2_path = leaf_2_full_path.slice(..2);
        let leaf_2_key = leaf_2_full_path.slice(2..);
        let leaf_3_full_path = Nibbles::from_nibbles([vec![0, 2], vec![0; 62]].concat());
        let leaf_1 = create_leaf_node(leaf_1_key.to_vec(), 1);
        let leaf_2 = create_leaf_node(leaf_2_key.to_vec(), 2);

        // Create branch node at [0x0] with only children 0x0 and 0x1.
        // Child 0x2 (leaf_3) will be inserted via update_leaf to create a fresh node
        // with hash: None.
        let child_hashes = [
            RlpNode::word_rlp(&B256::repeat_byte(0x00)),
            RlpNode::word_rlp(&B256::repeat_byte(0x11)),
        ];
        let branch_path = Nibbles::from_nibbles([0x0]);
        let branch_node = create_branch_node_with_children(&[0x0, 0x1], child_hashes);

        // Reveal the existing nodes
        trie.reveal_nodes(&mut [
            ProofTrieNodeV2 { path: branch_path, node: branch_node, masks: None },
            ProofTrieNodeV2 { path: leaf_1_path, node: leaf_1, masks: None },
            ProofTrieNodeV2 { path: leaf_2_path, node: leaf_2, masks: None },
        ])
        .unwrap();

        // Insert leaf_3 via update_leaf. This modifies the branch at [0x0] to add child
        // 0x2 and creates a fresh leaf node with hash: None in the lower subtrie.
        let provider = NoRevealProvider;
        trie.update_leaf(leaf_3_full_path, encode_account_value(3), provider).unwrap();

        // Calculate subtrie indexes
        let subtrie_1_index = SparseSubtrieType::from_path(&leaf_1_path).lower_index().unwrap();
        let subtrie_2_index = SparseSubtrieType::from_path(&leaf_2_path).lower_index().unwrap();
        let leaf_3_path = leaf_3_full_path.slice(..2);
        let subtrie_3_index = SparseSubtrieType::from_path(&leaf_3_path).lower_index().unwrap();

        let mut unchanged_prefix_set = PrefixSetMut::from([
            Nibbles::from_nibbles([0x0]),
            leaf_2_full_path,
            Nibbles::from_nibbles([0x3, 0x0, 0x0]),
        ]);
        // Create a prefix set with the keys that match only the second subtrie
        let mut prefix_set = PrefixSetMut::from([
            // Match second subtrie
            Nibbles::from_nibbles([0x0, 0x1, 0x0]),
            Nibbles::from_nibbles([0x0, 0x1, 0x1, 0x0]),
        ]);
        prefix_set.extend(unchanged_prefix_set.clone());
        trie.prefix_set = prefix_set;

        // Update subtrie hashes
        trie.update_subtrie_hashes();

        // We expect that leaf 3 (0x02) should have been added to the prefix set, because it is
        // missing a hash and is the root node of a lower subtrie, and therefore would need to have
        // that hash calculated by `update_upper_subtrie_hashes`.
        unchanged_prefix_set.insert(leaf_3_full_path);

        // Check that the prefix set was updated
        assert_eq!(
            trie.prefix_set.clone().freeze().into_iter().collect::<Vec<_>>(),
            unchanged_prefix_set.freeze().into_iter().collect::<Vec<_>>()
        );
        // Check that subtries were returned back to the array
        assert!(trie.lower_subtries[subtrie_1_index].as_revealed_ref().is_some());
        assert!(trie.lower_subtries[subtrie_2_index].as_revealed_ref().is_some());
        assert!(trie.lower_subtries[subtrie_3_index].as_revealed_ref().is_some());
    }

    #[test]
    fn test_subtrie_update_hashes() {
        let mut subtrie = Box::new(SparseSubtrie::new(Nibbles::from_nibbles([0x0, 0x0])));

        // Create leaf nodes with paths 0x0...0, 0x00001...0, 0x0010...0
        let leaf_1_full_path = Nibbles::from_nibbles([0; 64]);
        let leaf_1_path = leaf_1_full_path.slice(..5);
        let leaf_1_key = leaf_1_full_path.slice(5..);
        let leaf_2_full_path = Nibbles::from_nibbles([vec![0, 0, 0, 0, 1], vec![0; 59]].concat());
        let leaf_2_path = leaf_2_full_path.slice(..5);
        let leaf_2_key = leaf_2_full_path.slice(5..);
        let leaf_3_full_path = Nibbles::from_nibbles([vec![0, 0, 1], vec![0; 61]].concat());
        let leaf_3_path = leaf_3_full_path.slice(..3);
        let leaf_3_key = leaf_3_full_path.slice(3..);

        let account_1 = create_account(1);
        let account_2 = create_account(2);
        let account_3 = create_account(3);
        let leaf_1 = create_leaf_node(leaf_1_key.to_vec(), account_1.nonce);
        let leaf_2 = create_leaf_node(leaf_2_key.to_vec(), account_2.nonce);
        let leaf_3 = create_leaf_node(leaf_3_key.to_vec(), account_3.nonce);

        // Create bottom branch node
        let extension_path = Nibbles::from_nibbles([0, 0, 0]);
        let branch_1_path = Nibbles::from_nibbles([0, 0, 0, 0]);
        let branch_1 = create_branch_node(
            Nibbles::from_nibbles([0]),
            &[0, 1],
            vec![
                RlpNode::from_rlp(&alloy_rlp::encode(&leaf_1)),
                RlpNode::from_rlp(&alloy_rlp::encode(&leaf_2)),
            ],
        );

        // Create top branch node
        let branch_2_path = Nibbles::from_nibbles([0, 0]);
        let branch_2 = create_branch_node_with_children(
            &[0, 1],
            vec![
                RlpNode::from_rlp(&alloy_rlp::encode(&branch_1)),
                RlpNode::from_rlp(&alloy_rlp::encode(&leaf_3)),
            ],
        );

        // Reveal nodes
        subtrie.reveal_node(branch_2_path, &branch_2, None, None).unwrap();
        subtrie.reveal_node(extension_path, &branch_1, None, None).unwrap();
        subtrie.reveal_node(leaf_1_path, &leaf_1, None, None).unwrap();
        subtrie.reveal_node(leaf_2_path, &leaf_2, None, None).unwrap();
        subtrie.reveal_node(leaf_3_path, &leaf_3, None, None).unwrap();

        // Run hash builder for two leaf nodes
        let (_, _, proof_nodes, _, _) = run_hash_builder(
            [
                (leaf_1_full_path, account_1),
                (leaf_2_full_path, account_2),
                (leaf_3_full_path, account_3),
            ],
            NoopAccountTrieCursor::default(),
            Default::default(),
            [extension_path, branch_2_path, leaf_1_full_path, leaf_2_full_path, leaf_3_full_path],
        );

        // Update hashes for the subtrie
        subtrie.update_hashes(
            &mut PrefixSetMut::from([leaf_1_full_path, leaf_2_full_path, leaf_3_full_path])
                .freeze(),
            &mut None,
            &BranchNodeMasksMap::default(),
        );

        // Compare hashes between hash builder and subtrie
        let hash_builder_branch_1_hash =
            RlpNode::from_rlp(proof_nodes.get(&branch_1_path).unwrap().as_ref()).as_hash().unwrap();
        let subtrie_branch_1_hash =
            subtrie.nodes.get(&branch_1_path).unwrap().cached_hash().unwrap();
        assert_eq!(hash_builder_branch_1_hash, subtrie_branch_1_hash);

        let hash_builder_extension_hash =
            RlpNode::from_rlp(proof_nodes.get(&extension_path).unwrap().as_ref())
                .as_hash()
                .unwrap();
        let subtrie_extension_hash =
            subtrie.nodes.get(&extension_path).unwrap().cached_hash().unwrap();
        assert_eq!(hash_builder_extension_hash, subtrie_extension_hash);

        let hash_builder_branch_2_hash =
            RlpNode::from_rlp(proof_nodes.get(&branch_2_path).unwrap().as_ref()).as_hash().unwrap();
        let subtrie_branch_2_hash =
            subtrie.nodes.get(&branch_2_path).unwrap().cached_hash().unwrap();
        assert_eq!(hash_builder_branch_2_hash, subtrie_branch_2_hash);

        let subtrie_leaf_1_hash = subtrie.nodes.get(&leaf_1_path).unwrap().cached_hash().unwrap();
        let hash_builder_leaf_1_hash =
            RlpNode::from_rlp(proof_nodes.get(&leaf_1_path).unwrap().as_ref()).as_hash().unwrap();
        assert_eq!(hash_builder_leaf_1_hash, subtrie_leaf_1_hash);

        let hash_builder_leaf_2_hash =
            RlpNode::from_rlp(proof_nodes.get(&leaf_2_path).unwrap().as_ref()).as_hash().unwrap();
        let subtrie_leaf_2_hash = subtrie.nodes.get(&leaf_2_path).unwrap().cached_hash().unwrap();
        assert_eq!(hash_builder_leaf_2_hash, subtrie_leaf_2_hash);

        let hash_builder_leaf_3_hash =
            RlpNode::from_rlp(proof_nodes.get(&leaf_3_path).unwrap().as_ref()).as_hash().unwrap();
        let subtrie_leaf_3_hash = subtrie.nodes.get(&leaf_3_path).unwrap().cached_hash().unwrap();
        assert_eq!(hash_builder_leaf_3_hash, subtrie_leaf_3_hash);
    }

    #[test]
    fn test_remove_leaf_branch_becomes_extension() {
        //
        // 0x:      Extension (Key = 5)
        // 0x5:     └── Branch (Mask = 1001)
        // 0x50:        ├── 0 -> Extension (Key = 23)
        // 0x5023:      │        └── Branch (Mask = 0101)
        // 0x50231:     │            ├── 1 -> Leaf
        // 0x50233:     │            └── 3 -> Leaf
        // 0x53:        └── 3 -> Leaf (Key = 7)
        //
        // After removing 0x53, extension+branch+extension become a single extension
        //
        let mut trie = new_test_trie(
            [
                (Nibbles::default(), SparseNode::new_ext(Nibbles::from_nibbles([0x5]))),
                (Nibbles::from_nibbles([0x5]), SparseNode::new_branch(TrieMask::new(0b1001), &[])),
                (
                    Nibbles::from_nibbles([0x5, 0x0]),
                    SparseNode::new_ext(Nibbles::from_nibbles([0x2, 0x3])),
                ),
                (
                    Nibbles::from_nibbles([0x5, 0x0, 0x2, 0x3]),
                    SparseNode::new_branch(TrieMask::new(0b0101), &[]),
                ),
                (
                    Nibbles::from_nibbles([0x5, 0x0, 0x2, 0x3, 0x1]),
                    SparseNode::new_leaf(leaf_key([], 59)),
                ),
                (
                    Nibbles::from_nibbles([0x5, 0x0, 0x2, 0x3, 0x3]),
                    SparseNode::new_leaf(leaf_key([], 59)),
                ),
                (Nibbles::from_nibbles([0x5, 0x3]), SparseNode::new_leaf(leaf_key([0x7], 62))),
            ]
            .into_iter(),
        );

        let provider = NoRevealProvider;

        // Remove the leaf with a full path of 0x537
        let leaf_full_path = pad_nibbles_right(Nibbles::from_nibbles([0x5, 0x3, 0x7]));
        trie.remove_leaf(&leaf_full_path, provider).unwrap();

        let upper_subtrie = &trie.upper_subtrie;
        let lower_subtrie_50 = trie.lower_subtries[0x50].as_revealed_ref().unwrap();

        // Check that the `SparseSubtrie` the leaf was removed from was itself removed, as it is now
        // empty.
        assert_matches!(trie.lower_subtries[0x53].as_revealed_ref(), None);

        // Check that the leaf node was removed, and that its parent/grandparent were modified
        // appropriately.
        assert_matches!(
            upper_subtrie.nodes.get(&Nibbles::from_nibbles([])),
            Some(SparseNode::Extension{ key, ..})
            if key == &Nibbles::from_nibbles([0x5, 0x0, 0x2, 0x3])
        );
        assert_matches!(upper_subtrie.nodes.get(&Nibbles::from_nibbles([0x5])), None);
        assert_matches!(lower_subtrie_50.nodes.get(&Nibbles::from_nibbles([0x5, 0x0])), None);
        assert_matches!(
            lower_subtrie_50.nodes.get(&Nibbles::from_nibbles([0x5, 0x0, 0x2, 0x3])),
            Some(SparseNode::Branch{ state_mask, .. })
            if *state_mask == 0b0101.into()
        );
    }

    #[test]
    fn test_remove_leaf_branch_becomes_leaf() {
        //
        // 0x:      Branch (Mask = 0011)
        // 0x0:     ├── 0 -> Leaf (Key = 12)
        // 0x1:     └── 1 -> Leaf (Key = 34)
        //
        // After removing 0x012, branch becomes a leaf
        //
        let mut trie = new_test_trie(
            [
                (Nibbles::default(), SparseNode::new_branch(TrieMask::new(0b0011), &[])),
                (Nibbles::from_nibbles([0x0]), SparseNode::new_leaf(leaf_key([0x1, 0x2], 63))),
                (Nibbles::from_nibbles([0x1]), SparseNode::new_leaf(leaf_key([0x3, 0x4], 63))),
            ]
            .into_iter(),
        );

        // Add the branch node to updated_nodes to simulate it being modified earlier
        if let Some(updates) = trie.updates.as_mut() {
            updates
                .updated_nodes
                .insert(Nibbles::default(), BranchNodeCompact::new(0b11, 0, 0, vec![], None));
        }

        let provider = NoRevealProvider;

        // Remove the leaf with a full path of 0x012
        let leaf_full_path = pad_nibbles_right(Nibbles::from_nibbles([0x0, 0x1, 0x2]));
        trie.remove_leaf(&leaf_full_path, provider).unwrap();

        let upper_subtrie = &trie.upper_subtrie;

        // Check that the leaf's value was removed
        assert_matches!(upper_subtrie.inner.values.get(&leaf_full_path), None);

        // Check that the branch node collapsed into a leaf node with the remaining child's key
        assert_matches!(
            upper_subtrie.nodes.get(&Nibbles::default()),
            Some(SparseNode::Leaf{ key, ..})
            if key == &pad_nibbles_right(Nibbles::from_nibbles([0x1, 0x3, 0x4]))
        );

        // Check that the remaining child node was removed
        assert_matches!(upper_subtrie.nodes.get(&Nibbles::from_nibbles([0x1])), None);
        // Check that the removed child node was also removed
        assert_matches!(upper_subtrie.nodes.get(&Nibbles::from_nibbles([0x0])), None);

        // Check that updates were tracked correctly when branch collapsed
        let updates = trie.updates.as_ref().unwrap();

        // The branch at root should be marked as removed since it collapsed
        assert!(updates.removed_nodes.contains(&Nibbles::default()));

        // The branch should no longer be in updated_nodes
        assert!(!updates.updated_nodes.contains_key(&Nibbles::default()));
    }

    #[test]
    fn test_remove_leaf_extension_becomes_leaf() {
        //
        // 0x:      Extension (Key = 5)
        // 0x5:     └── Branch (Mask = 0011)
        // 0x50:        ├── 0 -> Leaf (Key = 12)
        // 0x51:        └── 1 -> Leaf (Key = 34)
        //
        // After removing 0x5012, extension+branch becomes a leaf
        //
        let mut trie = new_test_trie(
            [
                (Nibbles::default(), SparseNode::new_ext(Nibbles::from_nibbles([0x5]))),
                (Nibbles::from_nibbles([0x5]), SparseNode::new_branch(TrieMask::new(0b0011), &[])),
                (Nibbles::from_nibbles([0x5, 0x0]), SparseNode::new_leaf(leaf_key([0x1, 0x2], 62))),
                (Nibbles::from_nibbles([0x5, 0x1]), SparseNode::new_leaf(leaf_key([0x3, 0x4], 62))),
            ]
            .into_iter(),
        );

        let provider = NoRevealProvider;

        // Remove the leaf with a full path of 0x5012
        let leaf_full_path = pad_nibbles_right(Nibbles::from_nibbles([0x5, 0x0, 0x1, 0x2]));
        trie.remove_leaf(&leaf_full_path, provider).unwrap();

        let upper_subtrie = &trie.upper_subtrie;

        // Check that both lower subtries were removed. 0x50 should have been removed because
        // removing its leaf made it empty. 0x51 should have been removed after its own leaf was
        // collapsed into the upper trie, leaving it also empty.
        assert_matches!(trie.lower_subtries[0x50].as_revealed_ref(), None);
        assert_matches!(trie.lower_subtries[0x51].as_revealed_ref(), None);

        // Check that the other leaf's value was moved to the upper trie
        let other_leaf_full_value = pad_nibbles_right(Nibbles::from_nibbles([0x5, 0x1, 0x3, 0x4]));
        assert_matches!(upper_subtrie.inner.values.get(&other_leaf_full_value), Some(_));

        // Check that the extension node collapsed into a leaf node
        assert_matches!(
            upper_subtrie.nodes.get(&Nibbles::default()),
            Some(SparseNode::Leaf{ key, ..})
            if key == &pad_nibbles_right(Nibbles::from_nibbles([0x5, 0x1, 0x3, 0x4]))
        );

        // Check that intermediate nodes were removed
        assert_matches!(upper_subtrie.nodes.get(&Nibbles::from_nibbles([0x5])), None);
    }

    #[test]
    fn test_remove_leaf_branch_on_branch() {
        //
        // 0x:      Branch (Mask = 0101)
        // 0x0:     ├── 0 -> Leaf (Key = 12)
        // 0x2:     └── 2 -> Branch (Mask = 0011)
        // 0x20:        ├── 0 -> Leaf (Key = 34)
        // 0x21:        └── 1 -> Leaf (Key = 56)
        //
        // After removing 0x2034, the inner branch becomes a leaf
        //
        let mut trie = new_test_trie(
            [
                (Nibbles::default(), SparseNode::new_branch(TrieMask::new(0b0101), &[])),
                (Nibbles::from_nibbles([0x0]), SparseNode::new_leaf(leaf_key([0x1, 0x2], 63))),
                (Nibbles::from_nibbles([0x2]), SparseNode::new_branch(TrieMask::new(0b0011), &[])),
                (Nibbles::from_nibbles([0x2, 0x0]), SparseNode::new_leaf(leaf_key([0x3, 0x4], 62))),
                (Nibbles::from_nibbles([0x2, 0x1]), SparseNode::new_leaf(leaf_key([0x5, 0x6], 62))),
            ]
            .into_iter(),
        );

        let provider = NoRevealProvider;

        // Remove the leaf with a full path of 0x2034
        let leaf_full_path = pad_nibbles_right(Nibbles::from_nibbles([0x2, 0x0, 0x3, 0x4]));
        trie.remove_leaf(&leaf_full_path, provider).unwrap();

        let upper_subtrie = &trie.upper_subtrie;

        // Check that both lower subtries were removed. 0x20 should have been removed because
        // removing its leaf made it empty. 0x21 should have been removed after its own leaf was
        // collapsed into the upper trie, leaving it also empty.
        assert_matches!(trie.lower_subtries[0x20].as_revealed_ref(), None);
        assert_matches!(trie.lower_subtries[0x21].as_revealed_ref(), None);

        // Check that the other leaf's value was moved to the upper trie
        let other_leaf_full_value = pad_nibbles_right(Nibbles::from_nibbles([0x2, 0x1, 0x5, 0x6]));
        assert_matches!(upper_subtrie.inner.values.get(&other_leaf_full_value), Some(_));

        // Check that the root branch still exists unchanged
        assert_matches!(
            upper_subtrie.nodes.get(&Nibbles::default()),
            Some(SparseNode::Branch{ state_mask, .. })
            if *state_mask == 0b0101.into()
        );

        // Check that the inner branch became an extension
        assert_matches!(
            upper_subtrie.nodes.get(&Nibbles::from_nibbles([0x2])),
            Some(SparseNode::Leaf{ key, ..})
            if key == &leaf_key([0x1, 0x5, 0x6], 63)
        );
    }

    #[test]
    fn test_remove_leaf_lower_subtrie_root_path_update() {
        //
        // 0x:        Extension (Key = 123, root of lower subtrie)
        // 0x123:     └── Branch (Mask = 0011000)
        // 0x1233:        ├── 3 -> Leaf (Key = [])
        // 0x1234:        └── 4 -> Extension (Key = 5)
        // 0x12345:           └── Branch (Mask = 0011)
        // 0x123450:              ├── 0 -> Leaf (Key = [])
        // 0x123451:              └── 1 -> Leaf (Key = [])
        //
        // After removing leaf at 0x1233, the branch at 0x123 becomes an extension to 0x12345, which
        // then gets merged with the root extension at 0x. The lower subtrie's `path` field should
        // be updated from 0x123 to 0x12345.
        //
        let mut trie = new_test_trie(
            [
                (Nibbles::default(), SparseNode::new_ext(Nibbles::from_nibbles([0x1, 0x2, 0x3]))),
                (
                    Nibbles::from_nibbles([0x1, 0x2, 0x3]),
                    SparseNode::new_branch(TrieMask::new(0b0011000), &[]),
                ),
                (
                    Nibbles::from_nibbles([0x1, 0x2, 0x3, 0x3]),
                    SparseNode::new_leaf(leaf_key([], 60)),
                ),
                (
                    Nibbles::from_nibbles([0x1, 0x2, 0x3, 0x4]),
                    SparseNode::new_ext(Nibbles::from_nibbles([0x5])),
                ),
                (
                    Nibbles::from_nibbles([0x1, 0x2, 0x3, 0x4, 0x5]),
                    SparseNode::new_branch(TrieMask::new(0b0011), &[]),
                ),
                (
                    Nibbles::from_nibbles([0x1, 0x2, 0x3, 0x4, 0x5, 0x0]),
                    SparseNode::new_leaf(leaf_key([], 58)),
                ),
                (
                    Nibbles::from_nibbles([0x1, 0x2, 0x3, 0x4, 0x5, 0x1]),
                    SparseNode::new_leaf(leaf_key([], 58)),
                ),
            ]
            .into_iter(),
        );

        let provider = NoRevealProvider;

        // Verify initial state - the lower subtrie's path should be 0x123
        let lower_subtrie_root_path = Nibbles::from_nibbles([0x1, 0x2, 0x3]);
        assert_matches!(
            trie.lower_subtrie_for_path_mut(&lower_subtrie_root_path),
            Some(subtrie)
            if subtrie.path == lower_subtrie_root_path
        );

        // Remove the leaf at 0x1233
        let leaf_full_path = pad_nibbles_right(Nibbles::from_nibbles([0x1, 0x2, 0x3, 0x3]));
        trie.remove_leaf(&leaf_full_path, provider).unwrap();

        // After removal:
        // 1. The branch at 0x123 should become an extension to 0x12345
        // 2. That extension should merge with the root extension at 0x
        // 3. The lower subtrie's path should be updated to 0x12345
        let lower_subtrie = trie.lower_subtries[0x12].as_revealed_ref().unwrap();
        assert_eq!(lower_subtrie.path, Nibbles::from_nibbles([0x1, 0x2, 0x3, 0x4, 0x5]));

        // Verify the root extension now points all the way to 0x12345
        assert_matches!(
            trie.upper_subtrie.nodes.get(&Nibbles::default()),
            Some(SparseNode::Extension { key, .. })
            if key == &Nibbles::from_nibbles([0x1, 0x2, 0x3, 0x4, 0x5])
        );

        // Verify the branch at 0x12345 hasn't been modified
        assert_matches!(
            lower_subtrie.nodes.get(&Nibbles::from_nibbles([0x1, 0x2, 0x3, 0x4, 0x5])),
            Some(SparseNode::Branch { state_mask, .. })
            if state_mask == &TrieMask::new(0b0011)
        );
    }

    #[test]
    fn test_remove_leaf_remaining_child_needs_reveal() {
        //
        // 0x:      Branch (Mask = 0011)
        // 0x0:     ├── 0 -> Leaf (Key = 12)
        // 0x1:     └── 1 -> Hash (blinded leaf)
        //
        // After removing 0x012, the hash node needs to be revealed to collapse the branch
        //
        let mut trie = new_test_trie(
            [
                (
                    Nibbles::default(),
                    SparseNode::new_branch(
                        TrieMask::new(0b0011),
                        &[(0x1, B256::repeat_byte(0xab))],
                    ),
                ),
                (Nibbles::from_nibbles([0x0]), SparseNode::new_leaf(leaf_key([0x1, 0x2], 63))),
            ]
            .into_iter(),
        );

        // Create a mock provider that will reveal the blinded leaf
        let revealed_leaf = create_leaf_node(leaf_key([0x3, 0x4], 63).to_vec(), 42);
        let mut encoded = Vec::new();
        revealed_leaf.encode(&mut encoded);

        // Try removing the leaf with a full path of 0x012, this should fail because the leaf is
        // blinded
        let leaf_full_path = pad_nibbles_right(Nibbles::from_nibbles([0x0, 0x1, 0x2]));
        let Err(err) = trie.remove_leaf(&leaf_full_path, NoRevealProvider) else {
            panic!("expected error");
        };
        assert_matches!(err.kind(), SparseTrieErrorKind::BlindedNode(path) if *path == Nibbles::from_nibbles([0x1]));

        // Now reveal the leaf and try removing it again
        trie.reveal_nodes(&mut [ProofTrieNodeV2 {
            path: Nibbles::from_nibbles([0x1]),
            node: revealed_leaf,
            masks: None,
        }])
        .unwrap();
        trie.remove_leaf(&leaf_full_path, NoRevealProvider).unwrap();

        let upper_subtrie = &trie.upper_subtrie;

        // Check that the leaf value was removed
        assert_matches!(upper_subtrie.inner.values.get(&leaf_full_path), None);

        // Check that the branch node collapsed into a leaf node with the revealed child's key
        assert_matches!(
            upper_subtrie.nodes.get(&Nibbles::default()),
            Some(SparseNode::Leaf{ key, ..})
            if key == &pad_nibbles_right(Nibbles::from_nibbles([0x1, 0x3, 0x4]))
        );

        // Check that the remaining child node was removed (since it was merged)
        assert_matches!(upper_subtrie.nodes.get(&Nibbles::from_nibbles([0x1])), None);
    }

    #[test]
    fn test_remove_leaf_root() {
        //
        // 0x:      Leaf (Key = 123)
        //
        // After removing 0x123, the trie becomes empty
        //
        let mut trie = new_test_trie(core::iter::once((
            Nibbles::default(),
            SparseNode::new_leaf(pad_nibbles_right(Nibbles::from_nibbles([0x1, 0x2, 0x3]))),
        )));

        let provider = NoRevealProvider;

        // Remove the leaf with a full key of 0x123
        let leaf_full_path = pad_nibbles_right(Nibbles::from_nibbles([0x1, 0x2, 0x3]));
        trie.remove_leaf(&leaf_full_path, provider).unwrap();

        let upper_subtrie = &trie.upper_subtrie;

        // Check that the leaf value was removed
        assert_matches!(upper_subtrie.inner.values.get(&leaf_full_path), None);

        // Check that the root node was changed to Empty
        assert_matches!(upper_subtrie.nodes.get(&Nibbles::default()), Some(SparseNode::Empty));
    }

    #[test]
    fn test_remove_leaf_unsets_hash_along_path() {
        //
        // Creates a trie structure:
        // 0x:      Branch (with hash set)
        // 0x0:     ├── Extension (with hash set)
        // 0x01:    │   └── Branch (with hash set)
        // 0x012:   │       ├── Leaf (Key = 34, with hash set)
        // 0x013:   │       ├── Leaf (Key = 56, with hash set)
        // 0x014:   │       └── Leaf (Key = 78, with hash set)
        // 0x1:     └── Leaf (Key = 78, with hash set)
        //
        // When removing leaf at 0x01234, all nodes along the path (root branch,
        // extension at 0x0, branch at 0x01) should have their hash field unset
        //

        let make_revealed = |hash: B256| SparseNodeState::Cached {
            rlp_node: RlpNode::word_rlp(&hash),
            store_in_db_trie: None,
        };
        let mut trie = new_test_trie(
            [
                (
                    Nibbles::default(),
                    SparseNode::Branch {
                        state_mask: TrieMask::new(0b0011),
                        state: make_revealed(B256::repeat_byte(0x10)),
                        blinded_mask: Default::default(),
                        blinded_hashes: Default::default(),
                    },
                ),
                (
                    Nibbles::from_nibbles([0x0]),
                    SparseNode::Extension {
                        key: Nibbles::from_nibbles([0x1]),
                        state: make_revealed(B256::repeat_byte(0x20)),
                    },
                ),
                (
                    Nibbles::from_nibbles([0x0, 0x1]),
                    SparseNode::Branch {
                        state_mask: TrieMask::new(0b11100),
                        state: make_revealed(B256::repeat_byte(0x30)),
                        blinded_mask: Default::default(),
                        blinded_hashes: Default::default(),
                    },
                ),
                (
                    Nibbles::from_nibbles([0x0, 0x1, 0x2]),
                    SparseNode::Leaf {
                        key: leaf_key([0x3, 0x4], 61),
                        state: make_revealed(B256::repeat_byte(0x40)),
                    },
                ),
                (
                    Nibbles::from_nibbles([0x0, 0x1, 0x3]),
                    SparseNode::Leaf {
                        key: leaf_key([0x5, 0x6], 61),
                        state: make_revealed(B256::repeat_byte(0x50)),
                    },
                ),
                (
                    Nibbles::from_nibbles([0x0, 0x1, 0x4]),
                    SparseNode::Leaf {
                        key: leaf_key([0x6, 0x7], 61),
                        state: make_revealed(B256::repeat_byte(0x60)),
                    },
                ),
                (
                    Nibbles::from_nibbles([0x1]),
                    SparseNode::Leaf {
                        key: leaf_key([0x7, 0x8], 63),
                        state: make_revealed(B256::repeat_byte(0x70)),
                    },
                ),
            ]
            .into_iter(),
        );

        let provider = NoRevealProvider;

        // Remove a leaf which does not exist; this should have no effect.
        trie.remove_leaf(
            &pad_nibbles_right(Nibbles::from_nibbles([0x0, 0x1, 0x2, 0x3, 0x4, 0xF])),
            provider,
        )
        .unwrap();
        for (path, node) in trie.all_nodes() {
            assert!(node.cached_hash().is_some(), "path {path:?} should still have a hash");
        }

        // Remove the leaf at path 0x01234
        let leaf_full_path = pad_nibbles_right(Nibbles::from_nibbles([0x0, 0x1, 0x2, 0x3, 0x4]));
        trie.remove_leaf(&leaf_full_path, provider).unwrap();

        let upper_subtrie = &trie.upper_subtrie;
        let lower_subtrie_10 = trie.lower_subtries[0x01].as_revealed_ref().unwrap();

        // Verify that hash fields are unset for all nodes along the path to the removed leaf
        assert_matches!(
            upper_subtrie.nodes.get(&Nibbles::default()),
            Some(SparseNode::Branch { state: SparseNodeState::Dirty, .. })
        );
        assert_matches!(
            upper_subtrie.nodes.get(&Nibbles::from_nibbles([0x0])),
            Some(SparseNode::Extension { state: SparseNodeState::Dirty, .. })
        );
        assert_matches!(
            lower_subtrie_10.nodes.get(&Nibbles::from_nibbles([0x0, 0x1])),
            Some(SparseNode::Branch { state: SparseNodeState::Dirty, .. })
        );

        // Verify that nodes not on the path still have their hashes
        assert_matches!(
            upper_subtrie.nodes.get(&Nibbles::from_nibbles([0x1])),
            Some(SparseNode::Leaf { state: SparseNodeState::Cached { .. }, .. })
        );
        assert_matches!(
            lower_subtrie_10.nodes.get(&Nibbles::from_nibbles([0x0, 0x1, 0x3])),
            Some(SparseNode::Leaf { state: SparseNodeState::Cached { .. }, .. })
        );
        assert_matches!(
            lower_subtrie_10.nodes.get(&Nibbles::from_nibbles([0x0, 0x1, 0x4])),
            Some(SparseNode::Leaf { state: SparseNodeState::Cached { .. }, .. })
        );
    }

    #[test]
    fn test_parallel_sparse_trie_root() {
        // Step 1: Create the trie structure
        // Extension node at 0x with key 0x2 (goes to upper subtrie)
        let extension_path = Nibbles::new();
        let extension_key = Nibbles::from_nibbles([0x2]);

        // Branch node at 0x2 with children 0 and 1 (goes to upper subtrie)
        let branch_path = Nibbles::from_nibbles([0x2]);

        // Leaf nodes at 0x20 and 0x21 (go to lower subtries)
        let leaf_1_path = Nibbles::from_nibbles([0x2, 0x0]);
        let leaf_1_key = Nibbles::from_nibbles(vec![0; 62]); // Remaining key
        let leaf_1_full_path = Nibbles::from_nibbles([vec![0x2, 0x0], vec![0; 62]].concat());

        let leaf_2_path = Nibbles::from_nibbles([0x2, 0x1]);
        let leaf_2_key = Nibbles::from_nibbles(vec![0; 62]); // Remaining key
        let leaf_2_full_path = Nibbles::from_nibbles([vec![0x2, 0x1], vec![0; 62]].concat());

        // Create accounts
        let account_1 = create_account(1);
        let account_2 = create_account(2);

        // Create leaf nodes
        let leaf_1 = create_leaf_node(leaf_1_key.to_vec(), account_1.nonce);
        let leaf_2 = create_leaf_node(leaf_2_key.to_vec(), account_2.nonce);

        // Create branch node with children at indices 0 and 1
        let branch = create_branch_node(
            extension_key,
            &[0, 1],
            vec![
                RlpNode::from_rlp(&alloy_rlp::encode(&leaf_1)),
                RlpNode::from_rlp(&alloy_rlp::encode(&leaf_2)),
            ],
        );

        // Step 2: Reveal nodes in the trie
        let mut trie = ParallelSparseTrie::from_root(branch, None, true).unwrap();
        trie.reveal_nodes(&mut [
            ProofTrieNodeV2 { path: leaf_1_path, node: leaf_1, masks: None },
            ProofTrieNodeV2 { path: leaf_2_path, node: leaf_2, masks: None },
        ])
        .unwrap();

        // Step 3: Reset hashes for all revealed nodes to test actual hash calculation
        // Reset upper subtrie node hashes
        trie.upper_subtrie
            .nodes
            .get_mut(&extension_path)
            .unwrap()
            .set_state(SparseNodeState::Dirty);
        trie.upper_subtrie.nodes.get_mut(&branch_path).unwrap().set_state(SparseNodeState::Dirty);

        // Reset lower subtrie node hashes
        let leaf_1_subtrie_idx = path_subtrie_index_unchecked(&leaf_1_path);
        let leaf_2_subtrie_idx = path_subtrie_index_unchecked(&leaf_2_path);

        trie.lower_subtries[leaf_1_subtrie_idx]
            .as_revealed_mut()
            .unwrap()
            .nodes
            .get_mut(&leaf_1_path)
            .unwrap()
            .set_state(SparseNodeState::Dirty);
        trie.lower_subtries[leaf_2_subtrie_idx]
            .as_revealed_mut()
            .unwrap()
            .nodes
            .get_mut(&leaf_2_path)
            .unwrap()
            .set_state(SparseNodeState::Dirty);

        // Step 4: Add changed leaf node paths to prefix set
        trie.prefix_set.insert(leaf_1_full_path);
        trie.prefix_set.insert(leaf_2_full_path);

        // Step 5: Calculate root using our implementation
        let root = trie.root();

        // Step 6: Calculate root using HashBuilder for comparison
        let (hash_builder_root, _, _proof_nodes, _, _) = run_hash_builder(
            [(leaf_1_full_path, account_1), (leaf_2_full_path, account_2)],
            NoopAccountTrieCursor::default(),
            Default::default(),
            [extension_path, branch_path, leaf_1_full_path, leaf_2_full_path],
        );

        // Step 7: Verify the roots match
        assert_eq!(root, hash_builder_root);

        // Verify hashes were computed
        let leaf_1_subtrie = trie.lower_subtries[leaf_1_subtrie_idx].as_revealed_ref().unwrap();
        let leaf_2_subtrie = trie.lower_subtries[leaf_2_subtrie_idx].as_revealed_ref().unwrap();
        assert!(trie.upper_subtrie.nodes.get(&extension_path).unwrap().cached_hash().is_some());
        assert!(trie.upper_subtrie.nodes.get(&branch_path).unwrap().cached_hash().is_some());
        assert!(leaf_1_subtrie.nodes.get(&leaf_1_path).unwrap().cached_hash().is_some());
        assert!(leaf_2_subtrie.nodes.get(&leaf_2_path).unwrap().cached_hash().is_some());
    }

    #[test]
    fn sparse_trie_empty_update_one() {
        let ctx = ParallelSparseTrieTestContext;

        let key = Nibbles::unpack(B256::with_last_byte(42));
        let value = || Account::default();
        let value_encoded = || {
            let mut account_rlp = Vec::new();
            value().into_trie_account(EMPTY_ROOT_HASH).encode(&mut account_rlp);
            account_rlp
        };

        let (hash_builder_root, hash_builder_updates, hash_builder_proof_nodes, _, _) =
            run_hash_builder(
                [(key, value())],
                NoopAccountTrieCursor::default(),
                Default::default(),
                [key],
            );

        let mut sparse = ParallelSparseTrie::default().with_updates(true);
        ctx.update_leaves(&mut sparse, [(key, value_encoded())]);
        ctx.assert_with_hash_builder(
            &mut sparse,
            hash_builder_root,
            hash_builder_updates,
            hash_builder_proof_nodes,
        );
    }

    #[test]
    fn sparse_trie_empty_update_multiple_lower_nibbles() {
        let ctx = ParallelSparseTrieTestContext;

        let paths = (0..=16).map(|b| Nibbles::unpack(B256::with_last_byte(b))).collect::<Vec<_>>();
        let value = || Account::default();
        let value_encoded = || {
            let mut account_rlp = Vec::new();
            value().into_trie_account(EMPTY_ROOT_HASH).encode(&mut account_rlp);
            account_rlp
        };

        let (hash_builder_root, hash_builder_updates, hash_builder_proof_nodes, _, _) =
            run_hash_builder(
                paths.iter().copied().zip(core::iter::repeat_with(value)),
                NoopAccountTrieCursor::default(),
                Default::default(),
                paths.clone(),
            );

        let mut sparse = ParallelSparseTrie::default().with_updates(true);
        ctx.update_leaves(
            &mut sparse,
            paths.into_iter().zip(core::iter::repeat_with(value_encoded)),
        );

        ctx.assert_with_hash_builder(
            &mut sparse,
            hash_builder_root,
            hash_builder_updates,
            hash_builder_proof_nodes,
        );
    }

    #[test]
    fn sparse_trie_empty_update_multiple_upper_nibbles() {
        let paths = (239..=255).map(|b| Nibbles::unpack(B256::repeat_byte(b))).collect::<Vec<_>>();
        let value = || Account::default();
        let value_encoded = || {
            let mut account_rlp = Vec::new();
            value().into_trie_account(EMPTY_ROOT_HASH).encode(&mut account_rlp);
            account_rlp
        };

        let (hash_builder_root, hash_builder_updates, hash_builder_proof_nodes, _, _) =
            run_hash_builder(
                paths.iter().copied().zip(core::iter::repeat_with(value)),
                NoopAccountTrieCursor::default(),
                Default::default(),
                paths.clone(),
            );

        let provider = DefaultTrieNodeProvider;
        let mut sparse = ParallelSparseTrie::default().with_updates(true);
        for path in &paths {
            sparse.update_leaf(*path, value_encoded(), &provider).unwrap();
        }
        let sparse_root = sparse.root();
        let sparse_updates = sparse.take_updates();

        assert_eq!(sparse_root, hash_builder_root);
        assert_eq!(sparse_updates.updated_nodes, hash_builder_updates.account_nodes);
        assert_eq_parallel_sparse_trie_proof_nodes(&sparse, hash_builder_proof_nodes);
    }

    #[test]
    fn sparse_trie_empty_update_multiple() {
        let ctx = ParallelSparseTrieTestContext;

        let paths = (0..=255)
            .map(|b| {
                Nibbles::unpack(if b % 2 == 0 {
                    B256::repeat_byte(b)
                } else {
                    B256::with_last_byte(b)
                })
            })
            .collect::<Vec<_>>();
        let value = || Account::default();
        let value_encoded = || {
            let mut account_rlp = Vec::new();
            value().into_trie_account(EMPTY_ROOT_HASH).encode(&mut account_rlp);
            account_rlp
        };

        let (hash_builder_root, hash_builder_updates, hash_builder_proof_nodes, _, _) =
            run_hash_builder(
                paths.iter().sorted_unstable().copied().zip(core::iter::repeat_with(value)),
                NoopAccountTrieCursor::default(),
                Default::default(),
                paths.clone(),
            );

        let mut sparse = ParallelSparseTrie::default().with_updates(true);
        ctx.update_leaves(
            &mut sparse,
            paths.iter().copied().zip(core::iter::repeat_with(value_encoded)),
        );
        ctx.assert_with_hash_builder(
            &mut sparse,
            hash_builder_root,
            hash_builder_updates,
            hash_builder_proof_nodes,
        );
    }

    #[test]
    fn sparse_trie_empty_update_repeated() {
        let ctx = ParallelSparseTrieTestContext;

        let paths = (0..=255).map(|b| Nibbles::unpack(B256::repeat_byte(b))).collect::<Vec<_>>();
        let old_value = Account { nonce: 1, ..Default::default() };
        let old_value_encoded = {
            let mut account_rlp = Vec::new();
            old_value.into_trie_account(EMPTY_ROOT_HASH).encode(&mut account_rlp);
            account_rlp
        };
        let new_value = Account { nonce: 2, ..Default::default() };
        let new_value_encoded = {
            let mut account_rlp = Vec::new();
            new_value.into_trie_account(EMPTY_ROOT_HASH).encode(&mut account_rlp);
            account_rlp
        };

        let (hash_builder_root, hash_builder_updates, hash_builder_proof_nodes, _, _) =
            run_hash_builder(
                paths.iter().copied().zip(core::iter::repeat_with(|| old_value)),
                NoopAccountTrieCursor::default(),
                Default::default(),
                paths.clone(),
            );

        let mut sparse = ParallelSparseTrie::default().with_updates(true);
        ctx.update_leaves(
            &mut sparse,
            paths.iter().copied().zip(core::iter::repeat(old_value_encoded)),
        );
        ctx.assert_with_hash_builder(
            &mut sparse,
            hash_builder_root,
            hash_builder_updates,
            hash_builder_proof_nodes,
        );

        let (hash_builder_root, hash_builder_updates, hash_builder_proof_nodes, _, _) =
            run_hash_builder(
                paths.iter().copied().zip(core::iter::repeat(new_value)),
                NoopAccountTrieCursor::default(),
                Default::default(),
                paths.clone(),
            );

        ctx.update_leaves(
            &mut sparse,
            paths.iter().copied().zip(core::iter::repeat(new_value_encoded)),
        );
        ctx.assert_with_hash_builder(
            &mut sparse,
            hash_builder_root,
            hash_builder_updates,
            hash_builder_proof_nodes,
        );
    }

    #[test]
    fn sparse_trie_remove_leaf() {
        let ctx = ParallelSparseTrieTestContext;
        let provider = DefaultTrieNodeProvider;
        let mut sparse = ParallelSparseTrie::default();

        let value = alloy_rlp::encode_fixed_size(&U256::ZERO).to_vec();

        ctx.update_leaves(
            &mut sparse,
            [
                (
                    pad_nibbles_right(Nibbles::from_nibbles([0x5, 0x0, 0x2, 0x3, 0x1])),
                    value.clone(),
                ),
                (
                    pad_nibbles_right(Nibbles::from_nibbles([0x5, 0x0, 0x2, 0x3, 0x3])),
                    value.clone(),
                ),
                (
                    pad_nibbles_right(Nibbles::from_nibbles([0x5, 0x2, 0x0, 0x1, 0x3])),
                    value.clone(),
                ),
                (
                    pad_nibbles_right(Nibbles::from_nibbles([0x5, 0x3, 0x1, 0x0, 0x2])),
                    value.clone(),
                ),
                (
                    pad_nibbles_right(Nibbles::from_nibbles([0x5, 0x3, 0x3, 0x0, 0x2])),
                    value.clone(),
                ),
                (pad_nibbles_right(Nibbles::from_nibbles([0x5, 0x3, 0x3, 0x2, 0x0])), value),
            ],
        );

        // Extension (Key = 5)
        // └── Branch (Mask = 1011)
        //     ├── 0 -> Extension (Key = 23)
        //     │        └── Branch (Mask = 0101)
        //     │              ├── 1 -> Leaf (Key = 1, Path = 50231)
        //     │              └── 3 -> Leaf (Key = 3, Path = 50233)
        //     ├── 2 -> Leaf (Key = 013, Path = 52013)
        //     └── 3 -> Branch (Mask = 0101)
        //                ├── 1 -> Leaf (Key = 3102, Path = 53102)
        //                └── 3 -> Branch (Mask = 1010)
        //                       ├── 0 -> Leaf (Key = 3302, Path = 53302)
        //                       └── 2 -> Leaf (Key = 3320, Path = 53320)
        pretty_assertions::assert_eq!(
            parallel_sparse_trie_nodes(&sparse)
                .into_iter()
                .map(|(k, v)| (*k, v.clone()))
                .collect::<BTreeMap<_, _>>(),
            BTreeMap::from_iter([
                (Nibbles::default(), SparseNode::new_ext(Nibbles::from_nibbles([0x5]))),
                (Nibbles::from_nibbles([0x5]), SparseNode::new_branch(0b1101.into(), &[])),
                (
                    Nibbles::from_nibbles([0x5, 0x0]),
                    SparseNode::new_ext(Nibbles::from_nibbles([0x2, 0x3]))
                ),
                (
                    Nibbles::from_nibbles([0x5, 0x0, 0x2, 0x3]),
                    SparseNode::new_branch(0b1010.into(), &[])
                ),
                (
                    Nibbles::from_nibbles([0x5, 0x0, 0x2, 0x3, 0x1]),
                    SparseNode::new_leaf(leaf_key([], 59))
                ),
                (
                    Nibbles::from_nibbles([0x5, 0x0, 0x2, 0x3, 0x3]),
                    SparseNode::new_leaf(leaf_key([], 59))
                ),
                (
                    Nibbles::from_nibbles([0x5, 0x2]),
                    SparseNode::new_leaf(leaf_key([0x0, 0x1, 0x3], 62))
                ),
                (Nibbles::from_nibbles([0x5, 0x3]), SparseNode::new_branch(0b1010.into(), &[])),
                (
                    Nibbles::from_nibbles([0x5, 0x3, 0x1]),
                    SparseNode::new_leaf(leaf_key([0x0, 0x2], 61))
                ),
                (
                    Nibbles::from_nibbles([0x5, 0x3, 0x3]),
                    SparseNode::new_branch(0b0101.into(), &[])
                ),
                (
                    Nibbles::from_nibbles([0x5, 0x3, 0x3, 0x0]),
                    SparseNode::new_leaf(leaf_key([0x2], 60))
                ),
                (
                    Nibbles::from_nibbles([0x5, 0x3, 0x3, 0x2]),
                    SparseNode::new_leaf(leaf_key([0x0], 60))
                )
            ])
        );

        sparse
            .remove_leaf(
                &pad_nibbles_right(Nibbles::from_nibbles([0x5, 0x2, 0x0, 0x1, 0x3])),
                &provider,
            )
            .unwrap();

        // Extension (Key = 5)
        // └── Branch (Mask = 1001)
        //     ├── 0 -> Extension (Key = 23)
        //     │        └── Branch (Mask = 0101)
        //     │              ├── 1 -> Leaf (Path = 50231...)
        //     │              └── 3 -> Leaf (Path = 50233...)
        //     └── 3 -> Branch (Mask = 0101)
        //                ├── 1 -> Leaf (Path = 53102...)
        //                └── 3 -> Branch (Mask = 1010)
        //                       ├── 0 -> Leaf (Path = 53302...)
        //                       └── 2 -> Leaf (Path = 53320...)
        pretty_assertions::assert_eq!(
            parallel_sparse_trie_nodes(&sparse)
                .into_iter()
                .map(|(k, v)| (*k, v.clone()))
                .collect::<BTreeMap<_, _>>(),
            BTreeMap::from_iter([
                (Nibbles::default(), SparseNode::new_ext(Nibbles::from_nibbles([0x5]))),
                (Nibbles::from_nibbles([0x5]), SparseNode::new_branch(0b1001.into(), &[])),
                (
                    Nibbles::from_nibbles([0x5, 0x0]),
                    SparseNode::new_ext(Nibbles::from_nibbles([0x2, 0x3]))
                ),
                (
                    Nibbles::from_nibbles([0x5, 0x0, 0x2, 0x3]),
                    SparseNode::new_branch(0b1010.into(), &[])
                ),
                (
                    Nibbles::from_nibbles([0x5, 0x0, 0x2, 0x3, 0x1]),
                    SparseNode::new_leaf(leaf_key([], 59))
                ),
                (
                    Nibbles::from_nibbles([0x5, 0x0, 0x2, 0x3, 0x3]),
                    SparseNode::new_leaf(leaf_key([], 59))
                ),
                (Nibbles::from_nibbles([0x5, 0x3]), SparseNode::new_branch(0b1010.into(), &[])),
                (
                    Nibbles::from_nibbles([0x5, 0x3, 0x1]),
                    SparseNode::new_leaf(leaf_key([0x0, 0x2], 61))
                ),
                (
                    Nibbles::from_nibbles([0x5, 0x3, 0x3]),
                    SparseNode::new_branch(0b0101.into(), &[])
                ),
                (
                    Nibbles::from_nibbles([0x5, 0x3, 0x3, 0x0]),
                    SparseNode::new_leaf(leaf_key([0x2], 60))
                ),
                (
                    Nibbles::from_nibbles([0x5, 0x3, 0x3, 0x2]),
                    SparseNode::new_leaf(leaf_key([0x0], 60))
                )
            ])
        );

        sparse
            .remove_leaf(
                &pad_nibbles_right(Nibbles::from_nibbles([0x5, 0x0, 0x2, 0x3, 0x1])),
                &provider,
            )
            .unwrap();

        // Extension (Key = 5)
        // └── Branch (Mask = 1001)
        //     ├── 0 -> Leaf (Path = 50233...)
        //     └── 3 -> Branch (Mask = 0101)
        //                ├── 1 -> Leaf (Path = 53102...)
        //                └── 3 -> Branch (Mask = 1010)
        //                       ├── 0 -> Leaf (Path = 53302...)
        //                       └── 2 -> Leaf (Path = 53320...)
        pretty_assertions::assert_eq!(
            parallel_sparse_trie_nodes(&sparse)
                .into_iter()
                .map(|(k, v)| (*k, v.clone()))
                .collect::<BTreeMap<_, _>>(),
            BTreeMap::from_iter([
                (Nibbles::default(), SparseNode::new_ext(Nibbles::from_nibbles([0x5]))),
                (Nibbles::from_nibbles([0x5]), SparseNode::new_branch(0b1001.into(), &[])),
                (
                    Nibbles::from_nibbles([0x5, 0x0]),
                    SparseNode::new_leaf(leaf_key([0x2, 0x3, 0x3], 62))
                ),
                (Nibbles::from_nibbles([0x5, 0x3]), SparseNode::new_branch(0b1010.into(), &[])),
                (
                    Nibbles::from_nibbles([0x5, 0x3, 0x1]),
                    SparseNode::new_leaf(leaf_key([0x0, 0x2], 61))
                ),
                (
                    Nibbles::from_nibbles([0x5, 0x3, 0x3]),
                    SparseNode::new_branch(0b0101.into(), &[])
                ),
                (
                    Nibbles::from_nibbles([0x5, 0x3, 0x3, 0x0]),
                    SparseNode::new_leaf(leaf_key([0x2], 60))
                ),
                (
                    Nibbles::from_nibbles([0x5, 0x3, 0x3, 0x2]),
                    SparseNode::new_leaf(leaf_key([0x0], 60))
                )
            ])
        );

        sparse
            .remove_leaf(
                &pad_nibbles_right(Nibbles::from_nibbles([0x5, 0x3, 0x1, 0x0, 0x2])),
                &provider,
            )
            .unwrap();

        // Extension (Key = 5)
        // └── Branch (Mask = 1001)
        //     ├── 0 -> Leaf (Path = 50233...)
        //     └── 3 -> Branch (Mask = 1010)
        //                ├── 0 -> Leaf (Path = 53302...)
        //                └── 2 -> Leaf (Path = 53320...)
        pretty_assertions::assert_eq!(
            parallel_sparse_trie_nodes(&sparse)
                .into_iter()
                .map(|(k, v)| (*k, v.clone()))
                .collect::<BTreeMap<_, _>>(),
            BTreeMap::from_iter([
                (Nibbles::default(), SparseNode::new_ext(Nibbles::from_nibbles([0x5]))),
                (Nibbles::from_nibbles([0x5]), SparseNode::new_branch(0b1001.into(), &[])),
                (
                    Nibbles::from_nibbles([0x5, 0x0]),
                    SparseNode::new_leaf(leaf_key([0x2, 0x3, 0x3], 62))
                ),
                (
                    Nibbles::from_nibbles([0x5, 0x3]),
                    SparseNode::new_ext(Nibbles::from_nibbles([0x3]))
                ),
                (
                    Nibbles::from_nibbles([0x5, 0x3, 0x3]),
                    SparseNode::new_branch(0b0101.into(), &[])
                ),
                (
                    Nibbles::from_nibbles([0x5, 0x3, 0x3, 0x0]),
                    SparseNode::new_leaf(leaf_key([0x2], 60))
                ),
                (
                    Nibbles::from_nibbles([0x5, 0x3, 0x3, 0x2]),
                    SparseNode::new_leaf(leaf_key([0x0], 60))
                )
            ])
        );

        sparse
            .remove_leaf(
                &pad_nibbles_right(Nibbles::from_nibbles([0x5, 0x3, 0x3, 0x2, 0x0])),
                &provider,
            )
            .unwrap();

        // Extension (Key = 5)
        // └── Branch (Mask = 1001)
        //     ├── 0 -> Leaf (Path = 50233...)
        //     └── 3 -> Leaf (Path = 53302...)
        pretty_assertions::assert_eq!(
            parallel_sparse_trie_nodes(&sparse)
                .into_iter()
                .map(|(k, v)| (*k, v.clone()))
                .collect::<BTreeMap<_, _>>(),
            BTreeMap::from_iter([
                (Nibbles::default(), SparseNode::new_ext(Nibbles::from_nibbles([0x5]))),
                (Nibbles::from_nibbles([0x5]), SparseNode::new_branch(0b1001.into(), &[])),
                (
                    Nibbles::from_nibbles([0x5, 0x0]),
                    SparseNode::new_leaf(leaf_key([0x2, 0x3, 0x3], 62))
                ),
                (
                    Nibbles::from_nibbles([0x5, 0x3]),
                    SparseNode::new_leaf(leaf_key([0x3, 0x0, 0x2], 62))
                ),
            ])
        );

        sparse
            .remove_leaf(
                &pad_nibbles_right(Nibbles::from_nibbles([0x5, 0x0, 0x2, 0x3, 0x3])),
                &provider,
            )
            .unwrap();

        // Leaf (Path = 53302...)
        pretty_assertions::assert_eq!(
            parallel_sparse_trie_nodes(&sparse)
                .into_iter()
                .map(|(k, v)| (*k, v.clone()))
                .collect::<BTreeMap<_, _>>(),
            BTreeMap::from_iter([(
                Nibbles::default(),
                SparseNode::new_leaf(pad_nibbles_right(Nibbles::from_nibbles([
                    0x5, 0x3, 0x3, 0x0, 0x2
                ])))
            ),])
        );

        sparse
            .remove_leaf(
                &pad_nibbles_right(Nibbles::from_nibbles([0x5, 0x3, 0x3, 0x0, 0x2])),
                &provider,
            )
            .unwrap();

        // Empty
        pretty_assertions::assert_eq!(
            parallel_sparse_trie_nodes(&sparse)
                .into_iter()
                .map(|(k, v)| (*k, v.clone()))
                .collect::<BTreeMap<_, _>>(),
            BTreeMap::from_iter([(Nibbles::default(), SparseNode::Empty)])
        );
    }

    #[test]
    fn sparse_trie_remove_leaf_blinded() {
        let leaf = LeafNode::new(
            Nibbles::default(),
            alloy_rlp::encode_fixed_size(&U256::from(1)).to_vec(),
        );
        let branch = TrieNodeV2::Branch(BranchNodeV2::new(
            Nibbles::default(),
            vec![
                RlpNode::word_rlp(&B256::repeat_byte(1)),
                RlpNode::from_raw_rlp(&alloy_rlp::encode(leaf.clone())).unwrap(),
            ],
            TrieMask::new(0b11),
            None,
        ));

        let provider = DefaultTrieNodeProvider;
        let mut sparse = ParallelSparseTrie::from_root(
            branch.clone(),
            Some(BranchNodeMasks {
                hash_mask: TrieMask::new(0b01),
                tree_mask: TrieMask::default(),
            }),
            false,
        )
        .unwrap();

        // Reveal a branch node and one of its children
        //
        // Branch (Mask = 11)
        // ├── 0 -> Hash (Path = 0)
        // └── 1 -> Leaf (Path = 1)
        sparse
            .reveal_nodes(&mut [
                ProofTrieNodeV2 {
                    path: Nibbles::default(),
                    node: branch,
                    masks: Some(BranchNodeMasks {
                        hash_mask: TrieMask::default(),
                        tree_mask: TrieMask::new(0b01),
                    }),
                },
                ProofTrieNodeV2 {
                    path: Nibbles::from_nibbles([0x1]),
                    node: TrieNodeV2::Leaf(leaf),
                    masks: None,
                },
            ])
            .unwrap();

        // Removing a blinded leaf should result in an error
        assert_matches!(
            sparse.remove_leaf(&pad_nibbles_right(Nibbles::from_nibbles([0x0])), &provider).map_err(|e| e.into_kind()),
            Err(SparseTrieErrorKind::BlindedNode(path)) if path == Nibbles::from_nibbles([0x0])
        );
    }

    #[test]
    fn sparse_trie_remove_leaf_non_existent() {
        let leaf = LeafNode::new(
            Nibbles::default(),
            alloy_rlp::encode_fixed_size(&U256::from(1)).to_vec(),
        );
        let branch = TrieNodeV2::Branch(BranchNodeV2::new(
            Nibbles::default(),
            vec![
                RlpNode::word_rlp(&B256::repeat_byte(1)),
                RlpNode::from_raw_rlp(&alloy_rlp::encode(leaf.clone())).unwrap(),
            ],
            TrieMask::new(0b11),
            None,
        ));

        let provider = DefaultTrieNodeProvider;
        let mut sparse = ParallelSparseTrie::from_root(
            branch.clone(),
            Some(BranchNodeMasks {
                hash_mask: TrieMask::new(0b01),
                tree_mask: TrieMask::default(),
            }),
            false,
        )
        .unwrap();

        // Reveal a branch node and one of its children
        //
        // Branch (Mask = 11)
        // ├── 0 -> Hash (Path = 0)
        // └── 1 -> Leaf (Path = 1)
        sparse
            .reveal_nodes(&mut [
                ProofTrieNodeV2 {
                    path: Nibbles::default(),
                    node: branch,
                    masks: Some(BranchNodeMasks {
                        hash_mask: TrieMask::default(),
                        tree_mask: TrieMask::new(0b01),
                    }),
                },
                ProofTrieNodeV2 {
                    path: Nibbles::from_nibbles([0x1]),
                    node: TrieNodeV2::Leaf(leaf),
                    masks: None,
                },
            ])
            .unwrap();

        // Removing a non-existent leaf should be a noop
        let sparse_old = sparse.clone();
        assert_matches!(
            sparse.remove_leaf(&pad_nibbles_right(Nibbles::from_nibbles([0x2])), &provider),
            Ok(())
        );
        assert_eq!(sparse, sparse_old);
    }

    #[test]
    fn sparse_trie_fuzz() {
        // Having only the first 3 nibbles set, we narrow down the range of keys
        // to 4096 different hashes. It allows us to generate collisions more likely
        // to test the sparse trie updates.
        const KEY_NIBBLES_LEN: usize = 3;

        fn test(updates: Vec<(BTreeMap<Nibbles, Account>, BTreeSet<Nibbles>)>) {
            {
                let mut state = BTreeMap::default();
                let default_provider = DefaultTrieNodeProvider;
                let provider_factory = create_test_provider_factory();
                let mut sparse = ParallelSparseTrie::default().with_updates(true);

                for (update, keys_to_delete) in updates {
                    // Insert state updates into the sparse trie and calculate the root
                    for (key, account) in update.clone() {
                        let account = account.into_trie_account(EMPTY_ROOT_HASH);
                        let mut account_rlp = Vec::new();
                        account.encode(&mut account_rlp);
                        sparse.update_leaf(key, account_rlp, &default_provider).unwrap();
                    }
                    // We need to clone the sparse trie, so that all updated branch nodes are
                    // preserved, and not only those that were changed after the last call to
                    // `root()`.
                    let mut updated_sparse = sparse.clone();
                    let sparse_root = updated_sparse.root();
                    let sparse_updates = updated_sparse.take_updates();

                    // Insert state updates into the hash builder and calculate the root
                    state.extend(update);
                    let provider = provider_factory.provider().unwrap();
                    let (hash_builder_root, hash_builder_updates, hash_builder_proof_nodes, _, _) =
                        reth_trie_db::with_adapter!(provider_factory, |A| {
                            let trie_cursor =
                                DatabaseTrieCursorFactory::<_, A>::new(provider.tx_ref());
                            run_hash_builder(
                                state.clone(),
                                trie_cursor.account_trie_cursor().unwrap(),
                                Default::default(),
                                state.keys().copied(),
                            )
                        });

                    // Extract account nodes before moving hash_builder_updates
                    let hash_builder_account_nodes = hash_builder_updates.account_nodes.clone();

                    // Write trie updates to the database
                    let provider_rw = provider_factory.provider_rw().unwrap();
                    provider_rw.write_trie_updates(hash_builder_updates).unwrap();
                    provider_rw.commit().unwrap();

                    // Assert that the sparse trie root matches the hash builder root
                    assert_eq!(sparse_root, hash_builder_root);
                    // Assert that the sparse trie updates match the hash builder updates
                    pretty_assertions::assert_eq!(
                        BTreeMap::from_iter(sparse_updates.updated_nodes),
                        BTreeMap::from_iter(hash_builder_account_nodes)
                    );
                    // Assert that the sparse trie nodes match the hash builder proof nodes
                    assert_eq_parallel_sparse_trie_proof_nodes(
                        &updated_sparse,
                        hash_builder_proof_nodes,
                    );

                    // Delete some keys from both the hash builder and the sparse trie and check
                    // that the sparse trie root still matches the hash builder root
                    for key in &keys_to_delete {
                        state.remove(key).unwrap();
                        sparse.remove_leaf(key, &default_provider).unwrap();
                    }

                    // We need to clone the sparse trie, so that all updated branch nodes are
                    // preserved, and not only those that were changed after the last call to
                    // `root()`.
                    let mut updated_sparse = sparse.clone();
                    let sparse_root = updated_sparse.root();
                    let sparse_updates = updated_sparse.take_updates();

                    let provider = provider_factory.provider().unwrap();
                    let (hash_builder_root, hash_builder_updates, hash_builder_proof_nodes, _, _) =
                        reth_trie_db::with_adapter!(provider_factory, |A| {
                            let trie_cursor =
                                DatabaseTrieCursorFactory::<_, A>::new(provider.tx_ref());
                            run_hash_builder(
                                state.clone(),
                                trie_cursor.account_trie_cursor().unwrap(),
                                keys_to_delete
                                    .iter()
                                    .map(|nibbles| B256::from_slice(&nibbles.pack()))
                                    .collect(),
                                state.keys().copied(),
                            )
                        });

                    // Extract account nodes before moving hash_builder_updates
                    let hash_builder_account_nodes = hash_builder_updates.account_nodes.clone();

                    // Write trie updates to the database
                    let provider_rw = provider_factory.provider_rw().unwrap();
                    provider_rw.write_trie_updates(hash_builder_updates).unwrap();
                    provider_rw.commit().unwrap();

                    // Assert that the sparse trie root matches the hash builder root
                    assert_eq!(sparse_root, hash_builder_root);
                    // Assert that the sparse trie updates match the hash builder updates
                    pretty_assertions::assert_eq!(
                        BTreeMap::from_iter(sparse_updates.updated_nodes),
                        BTreeMap::from_iter(hash_builder_account_nodes)
                    );
                    // Assert that the sparse trie nodes match the hash builder proof nodes
                    assert_eq_parallel_sparse_trie_proof_nodes(
                        &updated_sparse,
                        hash_builder_proof_nodes,
                    );
                }
            }
        }

        fn transform_updates(
            updates: Vec<BTreeMap<Nibbles, Account>>,
            mut rng: impl rand::Rng,
        ) -> Vec<(BTreeMap<Nibbles, Account>, BTreeSet<Nibbles>)> {
            let mut keys = BTreeSet::new();
            updates
                .into_iter()
                .map(|update| {
                    keys.extend(update.keys().copied());

                    let keys_to_delete_len = update.len() / 2;
                    let keys_to_delete = (0..keys_to_delete_len)
                        .map(|_| {
                            let key =
                                *rand::seq::IteratorRandom::choose(keys.iter(), &mut rng).unwrap();
                            keys.take(&key).unwrap()
                        })
                        .collect();

                    (update, keys_to_delete)
                })
                .collect::<Vec<_>>()
        }

        proptest!(ProptestConfig::with_cases(10), |(
            updates in proptest::collection::vec(
                proptest::collection::btree_map(
                    any_with::<Nibbles>(SizeRange::new(KEY_NIBBLES_LEN..=KEY_NIBBLES_LEN)).prop_map(pad_nibbles_right),
                    arb::<Account>(),
                    1..50,
                ),
                1..50,
            ).prop_perturb(transform_updates)
        )| {
            test(updates)
        });
    }

    #[test]
    fn sparse_trie_two_leaves_at_lower_roots() {
        let provider = DefaultTrieNodeProvider;
        let mut trie = ParallelSparseTrie::default().with_updates(true);
        let key_50 = Nibbles::unpack(hex!(
            "0x5000000000000000000000000000000000000000000000000000000000000000"
        ));
        let key_51 = Nibbles::unpack(hex!(
            "0x5100000000000000000000000000000000000000000000000000000000000000"
        ));

        let account = Account::default().into_trie_account(EMPTY_ROOT_HASH);
        let mut account_rlp = Vec::new();
        account.encode(&mut account_rlp);

        // Add a leaf and calculate the root.
        trie.update_leaf(key_50, account_rlp.clone(), &provider).unwrap();
        trie.root();

        // Add a second leaf and assert that the root is the expected value.
        trie.update_leaf(key_51, account_rlp.clone(), &provider).unwrap();

        let expected_root =
            hex!("0xdaf0ef9f91a2f179bb74501209effdb5301db1697bcab041eca2234b126e25de");
        let root = trie.root();
        assert_eq!(root, expected_root);
        assert_eq!(SparseTrieUpdates::default(), trie.take_updates());
    }

    /// We have three leaves that share the same prefix: 0x00, 0x01 and 0x02. Hash builder trie has
    /// only nodes 0x00 and 0x01, and we have proofs for them. Node B is new and inserted in the
    /// sparse trie first.
    ///
    /// 1. Reveal the hash builder proof to leaf 0x00 in the sparse trie.
    /// 2. Insert leaf 0x01 into the sparse trie.
    /// 3. Reveal the hash builder proof to leaf 0x02 in the sparse trie.
    ///
    /// The hash builder proof to the leaf 0x02 didn't have the leaf 0x01 at the corresponding
    /// nibble of the branch node, so we need to adjust the branch node instead of fully
    /// replacing it.
    #[test]
    fn sparse_trie_reveal_node_1() {
        let key1 = || pad_nibbles_right(Nibbles::from_nibbles_unchecked([0x00]));
        let key2 = || pad_nibbles_right(Nibbles::from_nibbles_unchecked([0x01]));
        let key3 = || pad_nibbles_right(Nibbles::from_nibbles_unchecked([0x02]));
        let value = || Account::default();
        let value_encoded = || {
            let mut account_rlp = Vec::new();
            value().into_trie_account(EMPTY_ROOT_HASH).encode(&mut account_rlp);
            account_rlp
        };

        // Generate the proof for the root node and initialize the sparse trie with it
        let (_, _, hash_builder_proof_nodes, branch_node_hash_masks, branch_node_tree_masks) =
            run_hash_builder(
                [(key1(), value()), (key3(), value())],
                NoopAccountTrieCursor::default(),
                Default::default(),
                [Nibbles::default()],
            );

        let provider = DefaultTrieNodeProvider;
        let masks = match (
            branch_node_hash_masks.get(&Nibbles::default()).copied(),
            branch_node_tree_masks.get(&Nibbles::default()).copied(),
        ) {
            (Some(h), Some(t)) => Some(BranchNodeMasks { hash_mask: h, tree_mask: t }),
            (Some(h), None) => {
                Some(BranchNodeMasks { hash_mask: h, tree_mask: TrieMask::default() })
            }
            (None, Some(t)) => {
                Some(BranchNodeMasks { hash_mask: TrieMask::default(), tree_mask: t })
            }
            (None, None) => None,
        };
        let mut sparse = ParallelSparseTrie::from_root(
            TrieNodeV2::decode(&mut &hash_builder_proof_nodes.nodes_sorted()[0].1[..]).unwrap(),
            masks,
            false,
        )
        .unwrap();

        // Generate the proof for the first key and reveal it in the sparse trie
        let (_, _, hash_builder_proof_nodes, branch_node_hash_masks, branch_node_tree_masks) =
            run_hash_builder(
                [(key1(), value()), (key3(), value())],
                NoopAccountTrieCursor::default(),
                Default::default(),
                [key1()],
            );
        let mut revealed_nodes: Vec<ProofTrieNodeV2> = hash_builder_proof_nodes
            .nodes_sorted()
            .into_iter()
            .map(|(path, node)| {
                let hash_mask = branch_node_hash_masks.get(&path).copied();
                let tree_mask = branch_node_tree_masks.get(&path).copied();
                let masks = BranchNodeMasks::from_optional(hash_mask, tree_mask);
                ProofTrieNodeV2 { path, node: TrieNodeV2::decode(&mut &node[..]).unwrap(), masks }
            })
            .collect();
        sparse.reveal_nodes(&mut revealed_nodes).unwrap();

        // Check that the branch node exists with only two nibbles set
        assert_matches!(
            sparse.upper_subtrie.nodes.get(&Nibbles::default()),
            Some(&SparseNode::Branch { state_mask, state: SparseNodeState::Dirty, .. }) if state_mask == TrieMask::new(0b101)
        );

        // Insert the leaf for the second key
        sparse.update_leaf(key2(), value_encoded(), &provider).unwrap();

        // Check that the branch node was updated and another nibble was set
        assert_matches!(
            sparse.upper_subtrie.nodes.get(&Nibbles::default()),
            Some(&SparseNode::Branch { state_mask, state: SparseNodeState::Dirty, .. }) if state_mask == TrieMask::new(0b111)
        );

        // Generate the proof for the third key and reveal it in the sparse trie
        let (_, _, hash_builder_proof_nodes, branch_node_hash_masks, branch_node_tree_masks) =
            run_hash_builder(
                [(key1(), value()), (key3(), value())],
                NoopAccountTrieCursor::default(),
                Default::default(),
                [key3()],
            );
        let mut revealed_nodes: Vec<ProofTrieNodeV2> = hash_builder_proof_nodes
            .nodes_sorted()
            .into_iter()
            .map(|(path, node)| {
                let hash_mask = branch_node_hash_masks.get(&path).copied();
                let tree_mask = branch_node_tree_masks.get(&path).copied();
                let masks = BranchNodeMasks::from_optional(hash_mask, tree_mask);
                ProofTrieNodeV2 { path, node: TrieNodeV2::decode(&mut &node[..]).unwrap(), masks }
            })
            .collect();
        sparse.reveal_nodes(&mut revealed_nodes).unwrap();

        // Check that nothing changed in the branch node
        assert_matches!(
            sparse.upper_subtrie.nodes.get(&Nibbles::default()),
            Some(&SparseNode::Branch { state_mask, state: SparseNodeState::Dirty, .. }) if state_mask == TrieMask::new(0b111)
        );

        // Generate the nodes for the full trie with all three key using the hash builder, and
        // compare them to the sparse trie
        let (_, _, hash_builder_proof_nodes, _, _) = run_hash_builder(
            [(key1(), value()), (key2(), value()), (key3(), value())],
            NoopAccountTrieCursor::default(),
            Default::default(),
            [key1(), key2(), key3()],
        );

        assert_eq_parallel_sparse_trie_proof_nodes(&sparse, hash_builder_proof_nodes);
    }

    /// We have three leaves: 0x0000, 0x0101, and 0x0102. Hash builder trie has all nodes, and we
    /// have proofs for them.
    ///
    /// 1. Reveal the hash builder proof to leaf 0x00 in the sparse trie.
    /// 2. Remove leaf 0x00 from the sparse trie (that will remove the branch node and create an
    ///    extension node with the key 0x0000).
    /// 3. Reveal the hash builder proof to leaf 0x0101 in the sparse trie.
    ///
    /// The hash builder proof to the leaf 0x0101 had a branch node in the path, but we turned it
    /// into an extension node, so it should ignore this node.
    #[test]
    fn sparse_trie_reveal_node_2() {
        let key1 = || pad_nibbles_right(Nibbles::from_nibbles_unchecked([0x00, 0x00]));
        let key2 = || pad_nibbles_right(Nibbles::from_nibbles_unchecked([0x01, 0x01]));
        let key3 = || pad_nibbles_right(Nibbles::from_nibbles_unchecked([0x01, 0x02]));
        let value = || Account::default();

        // Generate the proof for the root node and initialize the sparse trie with it
        let (_, _, hash_builder_proof_nodes, branch_node_hash_masks, branch_node_tree_masks) =
            run_hash_builder(
                [(key1(), value()), (key2(), value()), (key3(), value())],
                NoopAccountTrieCursor::default(),
                Default::default(),
                [Nibbles::default()],
            );

        let provider = DefaultTrieNodeProvider;
        let masks = match (
            branch_node_hash_masks.get(&Nibbles::default()).copied(),
            branch_node_tree_masks.get(&Nibbles::default()).copied(),
        ) {
            (Some(h), Some(t)) => Some(BranchNodeMasks { hash_mask: h, tree_mask: t }),
            (Some(h), None) => {
                Some(BranchNodeMasks { hash_mask: h, tree_mask: TrieMask::default() })
            }
            (None, Some(t)) => {
                Some(BranchNodeMasks { hash_mask: TrieMask::default(), tree_mask: t })
            }
            (None, None) => None,
        };
        let mut sparse = ParallelSparseTrie::from_root(
            TrieNodeV2::decode(&mut &hash_builder_proof_nodes.nodes_sorted()[0].1[..]).unwrap(),
            masks,
            false,
        )
        .unwrap();

        // Generate the proof for the children of the root branch node and reveal it in the sparse
        // trie
        let (_, _, hash_builder_proof_nodes, branch_node_hash_masks, branch_node_tree_masks) =
            run_hash_builder(
                [(key1(), value()), (key2(), value()), (key3(), value())],
                NoopAccountTrieCursor::default(),
                Default::default(),
                [key1(), Nibbles::from_nibbles_unchecked([0x01])],
            );
        let mut revealed_nodes: Vec<ProofTrieNodeV2> = hash_builder_proof_nodes
            .nodes_sorted()
            .into_iter()
            .map(|(path, node)| {
                let hash_mask = branch_node_hash_masks.get(&path).copied();
                let tree_mask = branch_node_tree_masks.get(&path).copied();
                let masks = BranchNodeMasks::from_optional(hash_mask, tree_mask);
                ProofTrieNodeV2 { path, node: TrieNodeV2::decode(&mut &node[..]).unwrap(), masks }
            })
            .collect();
        sparse.reveal_nodes(&mut revealed_nodes).unwrap();

        // Check that the branch node exists
        assert_matches!(
            sparse.upper_subtrie.nodes.get(&Nibbles::default()),
            Some(&SparseNode::Branch { state_mask, state: SparseNodeState::Dirty, .. }) if state_mask == TrieMask::new(0b11)
        );

        // Remove the leaf for the first key
        sparse.remove_leaf(&key1(), &provider).unwrap();

        // Check that the branch node was turned into an extension node
        assert_eq!(
            sparse.upper_subtrie.nodes.get(&Nibbles::default()),
            Some(&SparseNode::new_ext(Nibbles::from_nibbles_unchecked([0x01])))
        );

        // Generate the proof for the third key and reveal it in the sparse trie
        let (_, _, hash_builder_proof_nodes, branch_node_hash_masks, branch_node_tree_masks) =
            run_hash_builder(
                [(key1(), value()), (key2(), value()), (key3(), value())],
                NoopAccountTrieCursor::default(),
                Default::default(),
                [key2()],
            );
        let mut revealed_nodes: Vec<ProofTrieNodeV2> = hash_builder_proof_nodes
            .nodes_sorted()
            .into_iter()
            .map(|(path, node)| {
                let hash_mask = branch_node_hash_masks.get(&path).copied();
                let tree_mask = branch_node_tree_masks.get(&path).copied();
                let masks = BranchNodeMasks::from_optional(hash_mask, tree_mask);
                ProofTrieNodeV2 { path, node: TrieNodeV2::decode(&mut &node[..]).unwrap(), masks }
            })
            .collect();
        sparse.reveal_nodes(&mut revealed_nodes).unwrap();

        // Check that nothing changed in the extension node
        assert_eq!(
            sparse.upper_subtrie.nodes.get(&Nibbles::default()),
            Some(&SparseNode::new_ext(Nibbles::from_nibbles_unchecked([0x01])))
        );
    }

    /// We have two leaves that share the same prefix: 0x0001 and 0x0002, and a leaf with a
    /// different prefix: 0x0100. Hash builder trie has only the first two leaves, and we have
    /// proofs for them.
    ///
    /// 1. Insert the leaf 0x0100 into the sparse trie, and check that the root extension node was
    ///    turned into a branch node.
    /// 2. Reveal the leaf 0x0001 in the sparse trie, and check that the root branch node wasn't
    ///    overwritten with the extension node from the proof.
    #[test]
    fn sparse_trie_reveal_node_3() {
        let key1 = || pad_nibbles_right(Nibbles::from_nibbles_unchecked([0x00, 0x01]));
        let key2 = || pad_nibbles_right(Nibbles::from_nibbles_unchecked([0x00, 0x02]));
        let key3 = || pad_nibbles_right(Nibbles::from_nibbles_unchecked([0x01, 0x00]));
        let value = || Account::default();
        let value_encoded = || {
            let mut account_rlp = Vec::new();
            value().into_trie_account(EMPTY_ROOT_HASH).encode(&mut account_rlp);
            account_rlp
        };

        // Generate the proof for the root node and initialize the sparse trie with it
        let (_, _, hash_builder_proof_nodes, branch_node_hash_masks, branch_node_tree_masks) =
            run_hash_builder(
                [(key1(), value()), (key2(), value())],
                NoopAccountTrieCursor::default(),
                Default::default(),
                [Nibbles::default()],
            );

        let mut nodes = Vec::new();

        for (path, node) in hash_builder_proof_nodes.nodes_sorted() {
            let hash_mask = branch_node_hash_masks.get(&path).copied();
            let tree_mask = branch_node_tree_masks.get(&path).copied();
            let masks = BranchNodeMasks::from_optional(hash_mask, tree_mask);
            nodes.push((path, TrieNode::decode(&mut &node[..]).unwrap(), masks));
        }

        nodes.sort_unstable_by(|a, b| reth_trie_common::depth_first_cmp(&a.0, &b.0));

        let nodes = ProofTrieNodeV2::from_sorted_trie_nodes(nodes);

        let provider = DefaultTrieNodeProvider;
        let mut sparse =
            ParallelSparseTrie::from_root(nodes[0].node.clone(), nodes[0].masks, false).unwrap();

        // Check that the root extension node exists
        assert_matches!(
            sparse.upper_subtrie.nodes.get(&Nibbles::default()),
            Some(SparseNode::Extension { key, state: SparseNodeState::Dirty }) if *key == Nibbles::from_nibbles([0x00])
        );

        // Insert the leaf with a different prefix
        sparse.update_leaf(key3(), value_encoded(), &provider).unwrap();

        // Check that the extension node was turned into a branch node
        assert_eq!(
            sparse.upper_subtrie.nodes.get(&Nibbles::default()),
            Some(&SparseNode::new_branch(TrieMask::new(0b11), &[]))
        );

        // Generate the proof for the first key and reveal it in the sparse trie
        let (_, _, hash_builder_proof_nodes, branch_node_hash_masks, branch_node_tree_masks) =
            run_hash_builder(
                [(key1(), value()), (key2(), value())],
                NoopAccountTrieCursor::default(),
                Default::default(),
                [key1()],
            );
        let mut revealed_nodes: Vec<ProofTrieNodeV2> = hash_builder_proof_nodes
            .nodes_sorted()
            .into_iter()
            .map(|(path, node)| {
                let hash_mask = branch_node_hash_masks.get(&path).copied();
                let tree_mask = branch_node_tree_masks.get(&path).copied();
                let masks = BranchNodeMasks::from_optional(hash_mask, tree_mask);
                ProofTrieNodeV2 { path, node: TrieNodeV2::decode(&mut &node[..]).unwrap(), masks }
            })
            .collect();
        sparse.reveal_nodes(&mut revealed_nodes).unwrap();

        // Check that the branch node wasn't overwritten by the extension node in the proof
        assert_eq!(
            sparse.upper_subtrie.nodes.get(&Nibbles::default()),
            Some(&SparseNode::new_branch(TrieMask::new(0b11), &[]))
        );
    }

    #[test]
    fn test_update_leaf_cross_level() {
        let ctx = ParallelSparseTrieTestContext;
        let mut trie = ParallelSparseTrie::from_root(TrieNodeV2::EmptyRoot, None, true).unwrap();

        // Test adding leaves that demonstrate the cross-level behavior
        // Based on the example: leaves 0x1234, 0x1245, 0x1334, 0x1345
        //
        // Final trie structure:
        // Upper trie:
        //   0x: Extension { key: 0x1 }
        //   └── 0x1: Branch { state_mask: 0x1100 }
        //       └── Subtrie (0x12): pointer to lower subtrie
        //       └── Subtrie (0x13): pointer to lower subtrie
        //
        // Lower subtrie (0x12):
        //   0x12: Branch { state_mask: 0x8 | 0x10 }
        //   ├── 0x123: Leaf { key: 0x4 }
        //   └── 0x124: Leaf { key: 0x5 }
        //
        // Lower subtrie (0x13):
        //   0x13: Branch { state_mask: 0x8 | 0x10 }
        //   ├── 0x133: Leaf { key: 0x4 }
        //   └── 0x134: Leaf { key: 0x5 }

        // First add leaf 0x1345 - this should create a leaf in upper trie at 0x
        let (leaf1_path, value1) = ctx.create_test_leaf([0x1, 0x3, 0x4, 0x5], 1);
        trie.update_leaf(leaf1_path, value1.clone(), DefaultTrieNodeProvider).unwrap();

        // Verify upper trie has a leaf at the root with key 1345
        ctx.assert_upper_subtrie(&trie)
            .has_leaf(
                &Nibbles::default(),
                &pad_nibbles_right(Nibbles::from_nibbles([0x1, 0x3, 0x4, 0x5])),
            )
            .has_value(&leaf1_path, &value1);

        // Add leaf 0x1234 - this should go first in the upper subtrie
        let (leaf2_path, value2) = ctx.create_test_leaf([0x1, 0x2, 0x3, 0x4], 2);
        trie.update_leaf(leaf2_path, value2.clone(), DefaultTrieNodeProvider).unwrap();

        // Upper trie should now have a branch at 0x1
        ctx.assert_upper_subtrie(&trie)
            .has_branch(&Nibbles::from_nibbles([0x1]), &[0x2, 0x3])
            .has_no_value(&leaf1_path)
            .has_no_value(&leaf2_path);

        // Add leaf 0x1245 - this should cause a branch and create the 0x12 subtrie
        let (leaf3_path, value3) = ctx.create_test_leaf([0x1, 0x2, 0x4, 0x5], 3);
        trie.update_leaf(leaf3_path, value3.clone(), DefaultTrieNodeProvider).unwrap();

        // Verify lower subtrie at 0x12 exists with correct structure
        ctx.assert_subtrie(&trie, Nibbles::from_nibbles([0x1, 0x2]))
            .has_branch(&Nibbles::from_nibbles([0x1, 0x2]), &[0x3, 0x4])
            .has_leaf(&Nibbles::from_nibbles([0x1, 0x2, 0x3]), &leaf_key([0x4], 61))
            .has_leaf(&Nibbles::from_nibbles([0x1, 0x2, 0x4]), &leaf_key([0x5], 61))
            .has_value(&leaf2_path, &value2)
            .has_value(&leaf3_path, &value3);

        // Add leaf 0x1334 - this should create another lower subtrie
        let (leaf4_path, value4) = ctx.create_test_leaf([0x1, 0x3, 0x3, 0x4], 4);
        trie.update_leaf(leaf4_path, value4.clone(), DefaultTrieNodeProvider).unwrap();

        // Verify lower subtrie at 0x13 exists with correct values
        ctx.assert_subtrie(&trie, Nibbles::from_nibbles([0x1, 0x3]))
            .has_value(&leaf1_path, &value1)
            .has_value(&leaf4_path, &value4);

        // Verify the 0x12 subtrie still has its values
        ctx.assert_subtrie(&trie, Nibbles::from_nibbles([0x1, 0x2]))
            .has_value(&leaf2_path, &value2)
            .has_value(&leaf3_path, &value3);

        // Upper trie has no values
        ctx.assert_upper_subtrie(&trie)
            .has_extension(&Nibbles::default(), &Nibbles::from_nibbles([0x1]))
            .has_branch(&Nibbles::from_nibbles([0x1]), &[0x2, 0x3])
            .has_no_value(&leaf1_path)
            .has_no_value(&leaf2_path)
            .has_no_value(&leaf3_path)
            .has_no_value(&leaf4_path);
    }

    #[test]
    fn test_update_leaf_split_at_level_boundary() {
        let ctx = ParallelSparseTrieTestContext;
        let mut trie = ParallelSparseTrie::from_root(TrieNodeV2::EmptyRoot, None, true).unwrap();

        // This test demonstrates what happens when we insert leaves that cause
        // splitting exactly at the upper/lower trie boundary (2 nibbles).
        //
        // Final trie structure:
        // Upper trie:
        //   0x: Extension { key: 0x12 }
        //       └── Subtrie (0x12): pointer to lower subtrie
        //
        // Lower subtrie (0x12):
        //   0x12: Branch { state_mask: 0x4 | 0x8 }
        //   ├── 0x122: Leaf { key: 0x4 }
        //   └── 0x123: Leaf { key: 0x4 }

        // First insert a leaf that ends exactly at the boundary (2 nibbles)
        let (first_leaf_path, first_value) = ctx.create_test_leaf([0x1, 0x2, 0x2, 0x4], 1);

        trie.update_leaf(first_leaf_path, first_value.clone(), DefaultTrieNodeProvider).unwrap();

        // In an empty trie, the first leaf becomes the root, regardless of path length
        ctx.assert_upper_subtrie(&trie)
            .has_leaf(
                &Nibbles::default(),
                &pad_nibbles_right(Nibbles::from_nibbles([0x1, 0x2, 0x2, 0x4])),
            )
            .has_value(&first_leaf_path, &first_value);

        // Now insert another leaf that shares the same 2-nibble prefix
        let (second_leaf_path, second_value) = ctx.create_test_leaf([0x1, 0x2, 0x3, 0x4], 2);

        trie.update_leaf(second_leaf_path, second_value.clone(), DefaultTrieNodeProvider).unwrap();

        // Now both leaves should be in a lower subtrie at index [0x1, 0x2]
        ctx.assert_subtrie(&trie, Nibbles::from_nibbles([0x1, 0x2]))
            .has_branch(&Nibbles::from_nibbles([0x1, 0x2]), &[0x2, 0x3])
            .has_leaf(&Nibbles::from_nibbles([0x1, 0x2, 0x2]), &leaf_key([0x4], 61))
            .has_leaf(&Nibbles::from_nibbles([0x1, 0x2, 0x3]), &leaf_key([0x4], 61))
            .has_value(&first_leaf_path, &first_value)
            .has_value(&second_leaf_path, &second_value);

        // Upper subtrie should no longer have these values
        ctx.assert_upper_subtrie(&trie)
            .has_no_value(&first_leaf_path)
            .has_no_value(&second_leaf_path);
    }

    #[test]
    fn test_update_subtrie_with_multiple_leaves() {
        let ctx = ParallelSparseTrieTestContext;
        let mut trie = ParallelSparseTrie::from_root(TrieNodeV2::EmptyRoot, None, true).unwrap();

        // First, add multiple leaves that will create a subtrie structure
        // All leaves share the prefix [0x1, 0x2] to ensure they create a subtrie
        //
        // This should result in a trie with the following structure:
        // 0x: Extension { key: 0x12 }
        //  └── Subtrie (0x12):
        //      0x12: Branch { state_mask: 0x3 | 0x4 }
        //      ├── 0x123: Branch { state_mask: 0x4 | 0x5 }
        //      │   ├── 0x1234: Leaf { key: 0x }
        //      │   └── 0x1235: Leaf { key: 0x }
        //      └── 0x124: Branch { state_mask: 0x6 | 0x7 }
        //          ├── 0x1246: Leaf { key: 0x }
        //          └── 0x1247: Leaf { key: 0x }
        let leaves = ctx.create_test_leaves(&[
            &[0x1, 0x2, 0x3, 0x4],
            &[0x1, 0x2, 0x3, 0x5],
            &[0x1, 0x2, 0x4, 0x6],
            &[0x1, 0x2, 0x4, 0x7],
        ]);

        // Insert all leaves
        ctx.update_leaves(&mut trie, leaves.clone());

        // Verify the upper subtrie has an extension node at the root with key 0x12
        ctx.assert_upper_subtrie(&trie)
            .has_extension(&Nibbles::default(), &Nibbles::from_nibbles([0x1, 0x2]));

        // Verify the subtrie structure using fluent assertions
        ctx.assert_subtrie(&trie, Nibbles::from_nibbles([0x1, 0x2]))
            .has_branch(&Nibbles::from_nibbles([0x1, 0x2]), &[0x3, 0x4])
            .has_branch(&Nibbles::from_nibbles([0x1, 0x2, 0x3]), &[0x4, 0x5])
            .has_branch(&Nibbles::from_nibbles([0x1, 0x2, 0x4]), &[0x6, 0x7])
            .has_value(&leaves[0].0, &leaves[0].1)
            .has_value(&leaves[1].0, &leaves[1].1)
            .has_value(&leaves[2].0, &leaves[2].1)
            .has_value(&leaves[3].0, &leaves[3].1);

        // Now update one of the leaves with a new value
        let updated_path = pad_nibbles_right(Nibbles::from_nibbles([0x1, 0x2, 0x3, 0x4]));
        let (_, updated_value) = ctx.create_test_leaf([0x1, 0x2, 0x3, 0x4], 100);

        trie.update_leaf(updated_path, updated_value.clone(), DefaultTrieNodeProvider).unwrap();

        // Verify the subtrie structure is maintained and value is updated
        // The branch structure should remain the same and all values should be present
        ctx.assert_subtrie(&trie, Nibbles::from_nibbles([0x1, 0x2]))
            .has_branch(&Nibbles::from_nibbles([0x1, 0x2]), &[0x3, 0x4])
            .has_value(&updated_path, &updated_value)
            .has_value(&leaves[1].0, &leaves[1].1)
            .has_value(&leaves[2].0, &leaves[2].1)
            .has_value(&leaves[3].0, &leaves[3].1);

        // Add a new leaf that extends an existing branch
        let (new_leaf_path, new_leaf_value) = ctx.create_test_leaf([0x1, 0x2, 0x3, 0x6], 200);

        trie.update_leaf(new_leaf_path, new_leaf_value.clone(), DefaultTrieNodeProvider).unwrap();

        // Verify the branch at [0x1, 0x2, 0x3] now has an additional child
        ctx.assert_subtrie(&trie, Nibbles::from_nibbles([0x1, 0x2]))
            .has_branch(&Nibbles::from_nibbles([0x1, 0x2, 0x3]), &[0x4, 0x5, 0x6])
            .has_value(&new_leaf_path, &new_leaf_value);
    }

    #[test]
    fn test_update_subtrie_extension_node_subtrie() {
        let ctx = ParallelSparseTrieTestContext;
        let mut trie = ParallelSparseTrie::from_root(TrieNodeV2::EmptyRoot, None, true).unwrap();

        // All leaves share the prefix [0x1, 0x2] to ensure they create a subtrie
        //
        // This should result in a trie with the following structure
        // 0x: Extension { key: 0x123 }
        //  └── Subtrie (0x12):
        //      0x123: Branch { state_mask: 0x3 | 0x4 }
        //      ├── 0x123: Leaf { key: 0x4 }
        //      └── 0x124: Leaf { key: 0x5 }
        let leaves = ctx.create_test_leaves(&[&[0x1, 0x2, 0x3, 0x4], &[0x1, 0x2, 0x3, 0x5]]);

        // Insert all leaves
        ctx.update_leaves(&mut trie, leaves.clone());

        // Verify the upper subtrie has an extension node at the root with key 0x123
        ctx.assert_upper_subtrie(&trie)
            .has_extension(&Nibbles::default(), &Nibbles::from_nibbles([0x1, 0x2, 0x3]));

        // Verify the lower subtrie structure
        ctx.assert_subtrie(&trie, Nibbles::from_nibbles([0x1, 0x2]))
            .has_branch(&Nibbles::from_nibbles([0x1, 0x2, 0x3]), &[0x4, 0x5])
            .has_value(&leaves[0].0, &leaves[0].1)
            .has_value(&leaves[1].0, &leaves[1].1);
    }

    #[test]
    fn update_subtrie_extension_node_cross_level() {
        let ctx = ParallelSparseTrieTestContext;
        let mut trie = ParallelSparseTrie::from_root(TrieNodeV2::EmptyRoot, None, true).unwrap();

        // First, add multiple leaves that will create a subtrie structure
        // All leaves share the prefix [0x1, 0x2] to ensure they create a branch node and subtrie
        //
        // This should result in a trie with the following structure
        // 0x: Extension { key: 0x12 }
        //  └── Subtrie (0x12):
        //      0x12: Branch { state_mask: 0x3 | 0x4 }
        //      ├── 0x123: Leaf { key: 0x4 }
        //      └── 0x124: Leaf { key: 0x5 }
        let leaves = ctx.create_test_leaves(&[&[0x1, 0x2, 0x3, 0x4], &[0x1, 0x2, 0x4, 0x5]]);

        // Insert all leaves
        ctx.update_leaves(&mut trie, leaves.clone());

        // Verify the upper subtrie has an extension node at the root with key 0x12
        ctx.assert_upper_subtrie(&trie)
            .has_extension(&Nibbles::default(), &Nibbles::from_nibbles([0x1, 0x2]));

        // Verify the lower subtrie structure
        ctx.assert_subtrie(&trie, Nibbles::from_nibbles([0x1, 0x2]))
            .has_branch(&Nibbles::from_nibbles([0x1, 0x2]), &[0x3, 0x4])
            .has_leaf(&Nibbles::from_nibbles([0x1, 0x2, 0x3]), &leaf_key([0x4], 61))
            .has_leaf(&Nibbles::from_nibbles([0x1, 0x2, 0x4]), &leaf_key([0x5], 61))
            .has_value(&leaves[0].0, &leaves[0].1)
            .has_value(&leaves[1].0, &leaves[1].1);
    }

    #[test]
    fn test_update_single_nibble_paths() {
        let ctx = ParallelSparseTrieTestContext;
        let mut trie = ParallelSparseTrie::from_root(TrieNodeV2::EmptyRoot, None, true).unwrap();

        // Test edge case: single nibble paths that create branches in upper trie
        //
        // Final trie structure:
        // Upper trie:
        //   0x: Branch { state_mask: 0x1 | 0x2 | 0x4 | 0x8 }
        //   ├── 0x0: Leaf { key: 0x }
        //   ├── 0x1: Leaf { key: 0x }
        //   ├── 0x2: Leaf { key: 0x }
        //   └── 0x3: Leaf { key: 0x }

        // Insert leaves with single nibble paths
        let (leaf1_path, value1) = ctx.create_test_leaf([0x0], 1);
        let (leaf2_path, value2) = ctx.create_test_leaf([0x1], 2);
        let (leaf3_path, value3) = ctx.create_test_leaf([0x2], 3);
        let (leaf4_path, value4) = ctx.create_test_leaf([0x3], 4);

        ctx.update_leaves(
            &mut trie,
            [
                (leaf1_path, value1.clone()),
                (leaf2_path, value2.clone()),
                (leaf3_path, value3.clone()),
                (leaf4_path, value4.clone()),
            ],
        );

        // Verify upper trie has a branch at root with 4 children
        ctx.assert_upper_subtrie(&trie)
            .has_branch(&Nibbles::default(), &[0x0, 0x1, 0x2, 0x3])
            .has_leaf(&Nibbles::from_nibbles([0x0]), &leaf_key([], 63))
            .has_leaf(&Nibbles::from_nibbles([0x1]), &leaf_key([], 63))
            .has_leaf(&Nibbles::from_nibbles([0x2]), &leaf_key([], 63))
            .has_leaf(&Nibbles::from_nibbles([0x3]), &leaf_key([], 63))
            .has_value(&leaf1_path, &value1)
            .has_value(&leaf2_path, &value2)
            .has_value(&leaf3_path, &value3)
            .has_value(&leaf4_path, &value4);
    }

    #[test]
    fn test_update_deep_extension_chain() {
        let ctx = ParallelSparseTrieTestContext;
        let mut trie = ParallelSparseTrie::from_root(TrieNodeV2::EmptyRoot, None, true).unwrap();

        // Test edge case: deep extension chains that span multiple levels
        //
        // Final trie structure:
        // Upper trie:
        //   0x: Extension { key: 0x111111 }
        //       └── Subtrie (0x11): pointer to lower subtrie
        //
        // Lower subtrie (0x11):
        //   0x111111: Branch { state_mask: 0x1 | 0x2 }
        //   ├── 0x1111110: Leaf { key: 0x }
        //   └── 0x1111111: Leaf { key: 0x }

        // Create leaves with a long common prefix
        let (leaf1_path, value1) = ctx.create_test_leaf([0x1, 0x1, 0x1, 0x1, 0x1, 0x1, 0x0], 1);
        let (leaf2_path, value2) = ctx.create_test_leaf([0x1, 0x1, 0x1, 0x1, 0x1, 0x1, 0x1], 2);

        ctx.update_leaves(&mut trie, [(leaf1_path, value1.clone()), (leaf2_path, value2.clone())]);

        // Verify upper trie has extension with the full common prefix
        ctx.assert_upper_subtrie(&trie).has_extension(
            &Nibbles::default(),
            &Nibbles::from_nibbles([0x1, 0x1, 0x1, 0x1, 0x1, 0x1]),
        );

        // Verify lower subtrie has branch structure
        ctx.assert_subtrie(&trie, Nibbles::from_nibbles([0x1, 0x1]))
            .has_branch(&Nibbles::from_nibbles([0x1, 0x1, 0x1, 0x1, 0x1, 0x1]), &[0x0, 0x1])
            .has_leaf(
                &Nibbles::from_nibbles([0x1, 0x1, 0x1, 0x1, 0x1, 0x1, 0x0]),
                &leaf_key([], 57),
            )
            .has_leaf(
                &Nibbles::from_nibbles([0x1, 0x1, 0x1, 0x1, 0x1, 0x1, 0x1]),
                &leaf_key([], 57),
            )
            .has_value(&leaf1_path, &value1)
            .has_value(&leaf2_path, &value2);
    }

    #[test]
    fn test_update_branch_with_all_nibbles() {
        let ctx = ParallelSparseTrieTestContext;
        let mut trie = ParallelSparseTrie::from_root(TrieNodeV2::EmptyRoot, None, true).unwrap();

        // Test edge case: branch node with all 16 possible nibble children
        //
        // Final trie structure:
        // Upper trie:
        //   0x: Extension { key: 0xA }
        //       └── Subtrie (0xA0): pointer to lower subtrie
        //
        // Lower subtrie (0xA0):
        //   0xA0: Branch { state_mask: 0xFFFF } (all 16 children)
        //   ├── 0xA00: Leaf { key: 0x }
        //   ├── 0xA01: Leaf { key: 0x }
        //   ├── 0xA02: Leaf { key: 0x }
        //   ... (all nibbles 0x0 through 0xF)
        //   └── 0xA0F: Leaf { key: 0x }

        // Create leaves for all 16 possible nibbles
        let mut leaves = Vec::new();
        for nibble in 0x0..=0xF {
            let (path, value) = ctx.create_test_leaf([0xA, 0x0, nibble], nibble as u64 + 1);
            leaves.push((path, value));
        }

        // Insert all leaves
        ctx.update_leaves(&mut trie, leaves.iter().cloned());

        // Verify upper trie structure
        ctx.assert_upper_subtrie(&trie)
            .has_extension(&Nibbles::default(), &Nibbles::from_nibbles([0xA, 0x0]));

        // Verify lower subtrie has branch with all 16 children
        let mut subtrie_assert =
            ctx.assert_subtrie(&trie, Nibbles::from_nibbles([0xA, 0x0])).has_branch(
                &Nibbles::from_nibbles([0xA, 0x0]),
                &[0x0, 0x1, 0x2, 0x3, 0x4, 0x5, 0x6, 0x7, 0x8, 0x9, 0xA, 0xB, 0xC, 0xD, 0xE, 0xF],
            );

        // Verify all leaves exist
        for (i, (path, value)) in leaves.iter().enumerate() {
            subtrie_assert = subtrie_assert
                .has_leaf(&Nibbles::from_nibbles([0xA, 0x0, i as u8]), &leaf_key([], 61))
                .has_value(path, value);
        }
    }

    #[test]
    fn test_update_creates_multiple_subtries() {
        let ctx = ParallelSparseTrieTestContext;
        let mut trie = ParallelSparseTrie::from_root(TrieNodeV2::EmptyRoot, None, true).unwrap();

        // Test edge case: updates that create multiple subtries at once
        //
        // Final trie structure:
        // Upper trie:
        //   0x: Extension { key: 0x0 }
        //       └── 0x0: Branch { state_mask: 0xF }
        //           ├── Subtrie (0x00): pointer
        //           ├── Subtrie (0x01): pointer
        //           ├── Subtrie (0x02): pointer
        //           └── Subtrie (0x03): pointer
        //
        // Each lower subtrie has leaves:
        //   0xXY: Leaf { key: 0xZ... }

        // Create leaves that will force multiple subtries
        let leaves = [
            ctx.create_test_leaf([0x0, 0x0, 0x1, 0x2], 1),
            ctx.create_test_leaf([0x0, 0x1, 0x3, 0x4], 2),
            ctx.create_test_leaf([0x0, 0x2, 0x5, 0x6], 3),
            ctx.create_test_leaf([0x0, 0x3, 0x7, 0x8], 4),
        ];

        // Insert all leaves
        ctx.update_leaves(&mut trie, leaves.iter().cloned());

        // Verify upper trie has extension then branch
        ctx.assert_upper_subtrie(&trie)
            .has_extension(&Nibbles::default(), &Nibbles::from_nibbles([0x0]))
            .has_branch(&Nibbles::from_nibbles([0x0]), &[0x0, 0x1, 0x2, 0x3]);

        // Verify each subtrie exists and contains its leaf
        for (i, (leaf_path, leaf_value)) in leaves.iter().enumerate() {
            let subtrie_path = Nibbles::from_nibbles([0x0, i as u8]);
            let full_path: [u8; 4] = match i {
                0 => [0x0, 0x0, 0x1, 0x2],
                1 => [0x0, 0x1, 0x3, 0x4],
                2 => [0x0, 0x2, 0x5, 0x6],
                3 => [0x0, 0x3, 0x7, 0x8],
                _ => unreachable!(),
            };
            ctx.assert_subtrie(&trie, subtrie_path)
                .has_leaf(&subtrie_path, &leaf_key(&full_path[2..], 62))
                .has_value(leaf_path, leaf_value);
        }
    }

    #[test]
    fn test_update_extension_to_branch_transformation() {
        let ctx = ParallelSparseTrieTestContext;
        let mut trie = ParallelSparseTrie::from_root(TrieNodeV2::EmptyRoot, None, true).unwrap();

        // Test edge case: extension node transforms to branch when split
        //
        // Initial state after first two leaves:
        // Upper trie:
        //   0x: Extension { key: 0xFF0 }
        //       └── Subtrie (0xFF): pointer
        //
        // After third leaf (0xF0...):
        // Upper trie:
        //   0x: Extension { key: 0xF }
        //       └── 0xF: Branch { state_mask: 0x10 | 0x8000 }
        //           ├── Subtrie (0xF0): pointer
        //           └── Subtrie (0xFF): pointer

        // First two leaves share prefix 0xFF0
        let (leaf1_path, value1) = ctx.create_test_leaf([0xF, 0xF, 0x0, 0x1], 1);
        let (leaf2_path, value2) = ctx.create_test_leaf([0xF, 0xF, 0x0, 0x2], 2);
        let (leaf3_path, value3) = ctx.create_test_leaf([0xF, 0x0, 0x0, 0x3], 3);

        ctx.update_leaves(&mut trie, [(leaf1_path, value1.clone()), (leaf2_path, value2.clone())]);

        // Verify initial extension structure
        ctx.assert_upper_subtrie(&trie)
            .has_extension(&Nibbles::default(), &Nibbles::from_nibbles([0xF, 0xF, 0x0]));

        // Add leaf that splits the extension
        ctx.update_leaves(&mut trie, [(leaf3_path, value3.clone())]);

        // Verify transformed structure
        ctx.assert_upper_subtrie(&trie)
            .has_extension(&Nibbles::default(), &Nibbles::from_nibbles([0xF]))
            .has_branch(&Nibbles::from_nibbles([0xF]), &[0x0, 0xF]);

        // Verify subtries
        ctx.assert_subtrie(&trie, Nibbles::from_nibbles([0xF, 0xF]))
            .has_branch(&Nibbles::from_nibbles([0xF, 0xF, 0x0]), &[0x1, 0x2])
            .has_leaf(&Nibbles::from_nibbles([0xF, 0xF, 0x0, 0x1]), &leaf_key([], 60))
            .has_leaf(&Nibbles::from_nibbles([0xF, 0xF, 0x0, 0x2]), &leaf_key([], 60))
            .has_value(&leaf1_path, &value1)
            .has_value(&leaf2_path, &value2);

        ctx.assert_subtrie(&trie, Nibbles::from_nibbles([0xF, 0x0]))
            .has_leaf(&Nibbles::from_nibbles([0xF, 0x0]), &leaf_key([0x0, 0x3], 62))
            .has_value(&leaf3_path, &value3);
    }

    #[test]
    fn test_update_long_shared_prefix_at_boundary() {
        let ctx = ParallelSparseTrieTestContext;
        let mut trie = ParallelSparseTrie::from_root(TrieNodeV2::EmptyRoot, None, true).unwrap();

        // Test edge case: leaves with long shared prefix that ends exactly at 2-nibble boundary
        //
        // Final trie structure:
        // Upper trie:
        //   0x: Extension { key: 0xAB }
        //       └── Subtrie (0xAB): pointer to lower subtrie
        //
        // Lower subtrie (0xAB):
        //   0xAB: Branch { state_mask: 0x1000 | 0x2000 }
        //   ├── 0xABC: Leaf { key: 0xDEF }
        //   └── 0xABD: Leaf { key: 0xEF0 }

        // Create leaves that share exactly 2 nibbles
        let (leaf1_path, value1) = ctx.create_test_leaf([0xA, 0xB, 0xC, 0xD, 0xE, 0xF], 1);
        let (leaf2_path, value2) = ctx.create_test_leaf([0xA, 0xB, 0xD, 0xE, 0xF, 0x0], 2);

        trie.update_leaf(leaf1_path, value1.clone(), DefaultTrieNodeProvider).unwrap();
        trie.update_leaf(leaf2_path, value2.clone(), DefaultTrieNodeProvider).unwrap();

        // Verify upper trie structure
        ctx.assert_upper_subtrie(&trie)
            .has_extension(&Nibbles::default(), &Nibbles::from_nibbles([0xA, 0xB]));

        // Verify lower subtrie structure
        ctx.assert_subtrie(&trie, Nibbles::from_nibbles([0xA, 0xB]))
            .has_branch(&Nibbles::from_nibbles([0xA, 0xB]), &[0xC, 0xD])
            .has_leaf(&Nibbles::from_nibbles([0xA, 0xB, 0xC]), &leaf_key([0xD, 0xE, 0xF], 61))
            .has_leaf(&Nibbles::from_nibbles([0xA, 0xB, 0xD]), &leaf_key([0xE, 0xF, 0x0], 61))
            .has_value(&leaf1_path, &value1)
            .has_value(&leaf2_path, &value2);
    }

    #[test]
    fn test_update_branch_to_extension_collapse() {
        let ctx = ParallelSparseTrieTestContext;
        let mut trie = ParallelSparseTrie::from_root(TrieNodeV2::EmptyRoot, None, true).unwrap();

        // Test creating a trie with leaves that share a long common prefix
        //
        // Initial state with 3 leaves (0x1234, 0x2345, 0x2356):
        // Upper trie:
        //   0x: Branch { state_mask: 0x6 }
        //       ├── 0x1: Leaf { key: 0x234 }
        //       └── 0x2: Extension { key: 0x3 }
        //           └── Subtrie (0x23): pointer
        // Lower subtrie (0x23):
        //   0x23: Branch { state_mask: 0x30 }
        //       ├── 0x234: Leaf { key: 0x5 }
        //       └── 0x235: Leaf { key: 0x6 }
        //
        // Then we create a new trie with leaves (0x1234, 0x1235, 0x1236):
        // Expected structure:
        // Upper trie:
        //   0x: Extension { key: 0x123 }
        //       └── Subtrie (0x12): pointer
        // Lower subtrie (0x12):
        //   0x123: Branch { state_mask: 0x70 } // bits 4, 5, 6 set
        //       ├── 0x1234: Leaf { key: 0x }
        //       ├── 0x1235: Leaf { key: 0x }
        //       └── 0x1236: Leaf { key: 0x }

        // Create initial leaves
        let (leaf1_path, value1) = ctx.create_test_leaf([0x1, 0x2, 0x3, 0x4], 1);
        let (leaf2_path, value2) = ctx.create_test_leaf([0x2, 0x3, 0x4, 0x5], 2);
        let (leaf3_path, value3) = ctx.create_test_leaf([0x2, 0x3, 0x5, 0x6], 3);

        trie.update_leaf(leaf1_path, value1, DefaultTrieNodeProvider).unwrap();
        trie.update_leaf(leaf2_path, value2, DefaultTrieNodeProvider).unwrap();
        trie.update_leaf(leaf3_path, value3, DefaultTrieNodeProvider).unwrap();

        // Verify initial structure has branch at root
        ctx.assert_upper_subtrie(&trie).has_branch(&Nibbles::default(), &[0x1, 0x2]);

        // Now update to create a pattern where extension is more efficient
        // Replace leaves to all share prefix 0x123
        let (new_leaf1_path, new_value1) = ctx.create_test_leaf([0x1, 0x2, 0x3, 0x4], 10);
        let (new_leaf2_path, new_value2) = ctx.create_test_leaf([0x1, 0x2, 0x3, 0x5], 11);
        let (new_leaf3_path, new_value3) = ctx.create_test_leaf([0x1, 0x2, 0x3, 0x6], 12);

        // Clear and add new leaves
        let mut trie = ParallelSparseTrie::from_root(TrieNodeV2::EmptyRoot, None, true).unwrap();
        trie.update_leaf(new_leaf1_path, new_value1.clone(), DefaultTrieNodeProvider).unwrap();
        trie.update_leaf(new_leaf2_path, new_value2.clone(), DefaultTrieNodeProvider).unwrap();
        trie.update_leaf(new_leaf3_path, new_value3.clone(), DefaultTrieNodeProvider).unwrap();

        // Verify new structure has extension
        ctx.assert_upper_subtrie(&trie)
            .has_extension(&Nibbles::default(), &Nibbles::from_nibbles([0x1, 0x2, 0x3]));

        // Verify lower subtrie path was correctly updated to 0x123
        ctx.assert_subtrie_path(&trie, [0x1, 0x2], [0x1, 0x2, 0x3]);

        // Verify lower subtrie - all three leaves should be properly inserted
        ctx.assert_subtrie(&trie, Nibbles::from_nibbles([0x1, 0x2]))
            .has_branch(&Nibbles::from_nibbles([0x1, 0x2, 0x3]), &[0x4, 0x5, 0x6]) // All three children
            .has_leaf(&Nibbles::from_nibbles([0x1, 0x2, 0x3, 0x4]), &leaf_key([], 60))
            .has_leaf(&Nibbles::from_nibbles([0x1, 0x2, 0x3, 0x5]), &leaf_key([], 60))
            .has_leaf(&Nibbles::from_nibbles([0x1, 0x2, 0x3, 0x6]), &leaf_key([], 60))
            .has_value(&new_leaf1_path, &new_value1)
            .has_value(&new_leaf2_path, &new_value2)
            .has_value(&new_leaf3_path, &new_value3);
    }

    #[test]
    fn test_update_shared_prefix_patterns() {
        let ctx = ParallelSparseTrieTestContext;
        let mut trie = ParallelSparseTrie::from_root(TrieNodeV2::EmptyRoot, None, true).unwrap();

        // Test edge case: different patterns of shared prefixes
        //
        // Final trie structure:
        // Upper trie:
        //   0x: Branch { state_mask: 0x6 }
        //       ├── 0x1: Leaf { key: 0x234 }
        //       └── 0x2: Extension { key: 0x3 }
        //           └── Subtrie (0x23): pointer
        //
        // Lower subtrie (0x23):
        //   0x23: Branch { state_mask: 0x10 | 0x20 }
        //   ├── 0x234: Leaf { key: 0x5 }
        //   └── 0x235: Leaf { key: 0x6 }

        // Create leaves with different shared prefix patterns
        let (leaf1_path, value1) = ctx.create_test_leaf([0x1, 0x2, 0x3, 0x4], 1);
        let (leaf2_path, value2) = ctx.create_test_leaf([0x2, 0x3, 0x4, 0x5], 2);
        let (leaf3_path, value3) = ctx.create_test_leaf([0x2, 0x3, 0x5, 0x6], 3);

        trie.update_leaf(leaf1_path, value1, DefaultTrieNodeProvider).unwrap();
        trie.update_leaf(leaf2_path, value2.clone(), DefaultTrieNodeProvider).unwrap();
        trie.update_leaf(leaf3_path, value3.clone(), DefaultTrieNodeProvider).unwrap();

        // Verify upper trie structure
        ctx.assert_upper_subtrie(&trie)
            .has_branch(&Nibbles::default(), &[0x1, 0x2])
            .has_leaf(&Nibbles::from_nibbles([0x1]), &leaf_key([0x2, 0x3, 0x4], 63))
            .has_extension(&Nibbles::from_nibbles([0x2]), &Nibbles::from_nibbles([0x3]));

        // Verify lower subtrie structure
        ctx.assert_subtrie(&trie, Nibbles::from_nibbles([0x2, 0x3]))
            .has_branch(&Nibbles::from_nibbles([0x2, 0x3]), &[0x4, 0x5])
            .has_leaf(&Nibbles::from_nibbles([0x2, 0x3, 0x4]), &leaf_key([0x5], 61))
            .has_leaf(&Nibbles::from_nibbles([0x2, 0x3, 0x5]), &leaf_key([0x6], 61))
            .has_value(&leaf2_path, &value2)
            .has_value(&leaf3_path, &value3);
    }

    #[test]
    fn test_progressive_branch_creation() {
        let ctx = ParallelSparseTrieTestContext;
        let mut trie = ParallelSparseTrie::from_root(TrieNodeV2::EmptyRoot, None, true).unwrap();

        // Test starting with a single leaf and progressively adding leaves
        // that create branch nodes at shorter and shorter paths
        //
        // Step 1: Add leaf at 0x12345
        // Upper trie:
        //   0x: Leaf { key: 0x12345 }
        //
        // Step 2: Add leaf at 0x12346
        // Upper trie:
        //   0x: Extension { key: 0x1234 }
        //       └── Subtrie (0x12): pointer
        // Lower subtrie (0x12):
        //   0x1234: Branch { state_mask: 0x60 }  // bits 5 and 6 set
        //       ├── 0x12345: Leaf { key: 0x }
        //       └── 0x12346: Leaf { key: 0x }
        //
        // Step 3: Add leaf at 0x1235
        // Lower subtrie (0x12) updates to:
        //   0x123: Branch { state_mask: 0x30 }  // bits 4 and 5 set
        //       ├── 0x1234: Branch { state_mask: 0x60 }
        //       │   ├── 0x12345: Leaf { key: 0x }
        //       │   └── 0x12346: Leaf { key: 0x }
        //       └── 0x1235: Leaf { key: 0x }
        //
        // Step 4: Add leaf at 0x124
        // Lower subtrie (0x12) updates to:
        //   0x12: Branch { state_mask: 0x18 }  // bits 3 and 4 set
        //       ├── 0x123: Branch { state_mask: 0x30 }
        //       │   ├── 0x1234: Branch { state_mask: 0x60 }
        //       │   │   ├── 0x12345: Leaf { key: 0x }
        //       │   │   └── 0x12346: Leaf { key: 0x }
        //       │   └── 0x1235: Leaf { key: 0x }
        //       └── 0x124: Leaf { key: 0x }

        // Step 1: Add first leaf - initially stored as leaf in upper trie
        let (leaf1_path, value1) = ctx.create_test_leaf([0x1, 0x2, 0x3, 0x4, 0x5], 1);
        trie.update_leaf(leaf1_path, value1.clone(), DefaultTrieNodeProvider).unwrap();

        // Verify leaf node in upper trie (optimized single-leaf case)
        ctx.assert_upper_subtrie(&trie)
            .has_leaf(
                &Nibbles::default(),
                &pad_nibbles_right(Nibbles::from_nibbles([0x1, 0x2, 0x3, 0x4, 0x5])),
            )
            .has_value(&leaf1_path, &value1);

        // Step 2: Add leaf at 0x12346 - creates branch at 0x1234
        let (leaf2_path, value2) = ctx.create_test_leaf([0x1, 0x2, 0x3, 0x4, 0x6], 2);
        trie.update_leaf(leaf2_path, value2.clone(), DefaultTrieNodeProvider).unwrap();

        // Verify extension now goes to 0x1234
        ctx.assert_upper_subtrie(&trie)
            .has_extension(&Nibbles::default(), &Nibbles::from_nibbles([0x1, 0x2, 0x3, 0x4]));

        // Verify subtrie path updated to 0x1234
        ctx.assert_subtrie_path(&trie, [0x1, 0x2], [0x1, 0x2, 0x3, 0x4]);

        ctx.assert_subtrie(&trie, Nibbles::from_nibbles([0x1, 0x2]))
            .has_branch(&Nibbles::from_nibbles([0x1, 0x2, 0x3, 0x4]), &[0x5, 0x6])
            .has_leaf(&Nibbles::from_nibbles([0x1, 0x2, 0x3, 0x4, 0x5]), &leaf_key([], 59))
            .has_leaf(&Nibbles::from_nibbles([0x1, 0x2, 0x3, 0x4, 0x6]), &leaf_key([], 59))
            .has_value(&leaf1_path, &value1)
            .has_value(&leaf2_path, &value2);

        // Step 3: Add leaf at 0x1235 - creates branch at 0x123
        let (leaf3_path, value3) = ctx.create_test_leaf([0x1, 0x2, 0x3, 0x5], 3);
        trie.update_leaf(leaf3_path, value3.clone(), DefaultTrieNodeProvider).unwrap();

        // Verify extension now goes to 0x123
        ctx.assert_upper_subtrie(&trie)
            .has_extension(&Nibbles::default(), &Nibbles::from_nibbles([0x1, 0x2, 0x3]));

        // Verify subtrie path updated to 0x123
        ctx.assert_subtrie_path(&trie, [0x1, 0x2], [0x1, 0x2, 0x3]);

        ctx.assert_subtrie(&trie, Nibbles::from_nibbles([0x1, 0x2]))
            .has_branch(&Nibbles::from_nibbles([0x1, 0x2, 0x3]), &[0x4, 0x5])
            .has_branch(&Nibbles::from_nibbles([0x1, 0x2, 0x3, 0x4]), &[0x5, 0x6])
            .has_leaf(&Nibbles::from_nibbles([0x1, 0x2, 0x3, 0x5]), &leaf_key([], 60))
            .has_value(&leaf1_path, &value1)
            .has_value(&leaf2_path, &value2)
            .has_value(&leaf3_path, &value3);

        // Step 4: Add leaf at 0x124 - creates branch at 0x12 (subtrie root)
        let (leaf4_path, value4) = ctx.create_test_leaf([0x1, 0x2, 0x4], 4);
        trie.update_leaf(leaf4_path, value4.clone(), DefaultTrieNodeProvider).unwrap();

        // Verify extension now goes to 0x12
        ctx.assert_upper_subtrie(&trie)
            .has_extension(&Nibbles::default(), &Nibbles::from_nibbles([0x1, 0x2]));

        // Verify subtrie path updated to 0x12
        ctx.assert_subtrie_path(&trie, [0x1, 0x2], [0x1, 0x2]);

        // Verify final structure
        ctx.assert_subtrie(&trie, Nibbles::from_nibbles([0x1, 0x2]))
            .has_branch(&Nibbles::from_nibbles([0x1, 0x2]), &[0x3, 0x4])
            .has_branch(&Nibbles::from_nibbles([0x1, 0x2, 0x3]), &[0x4, 0x5])
            .has_branch(&Nibbles::from_nibbles([0x1, 0x2, 0x3, 0x4]), &[0x5, 0x6])
            .has_leaf(&Nibbles::from_nibbles([0x1, 0x2, 0x4]), &leaf_key([], 61))
            .has_value(&leaf1_path, &value1)
            .has_value(&leaf2_path, &value2)
            .has_value(&leaf3_path, &value3)
            .has_value(&leaf4_path, &value4);
    }

    #[test]
    fn test_update_max_depth_paths() {
        let ctx = ParallelSparseTrieTestContext;
        let mut trie = ParallelSparseTrie::from_root(TrieNodeV2::EmptyRoot, None, true).unwrap();

        // Test edge case: very long paths (64 nibbles - max for addresses/storage)
        //
        // Final trie structure:
        // Upper trie:
        //   0x: Extension { key: 0xFF }
        //       └── Subtrie (0xFF): pointer
        //
        // Lower subtrie (0xFF):
        //   Has very long paths with slight differences at the end

        // Create two 64-nibble paths that differ only in the last nibble
        let mut path1_nibbles = vec![0xF; 63];
        path1_nibbles.push(0x0);
        let mut path2_nibbles = vec![0xF; 63];
        path2_nibbles.push(0x1);

        let (leaf1_path, value1) = ctx.create_test_leaf(&path1_nibbles, 1);
        let (leaf2_path, value2) = ctx.create_test_leaf(&path2_nibbles, 2);

        trie.update_leaf(leaf1_path, value1.clone(), DefaultTrieNodeProvider).unwrap();
        trie.update_leaf(leaf2_path, value2.clone(), DefaultTrieNodeProvider).unwrap();

        // The common prefix of 63 F's will create a very long extension
        let extension_key = vec![0xF; 63];
        ctx.assert_upper_subtrie(&trie)
            .has_extension(&Nibbles::default(), &Nibbles::from_nibbles(&extension_key));

        // Verify the subtrie has the branch at the end
        ctx.assert_subtrie(&trie, Nibbles::from_nibbles([0xF, 0xF]))
            .has_branch(&Nibbles::from_nibbles(&path1_nibbles[..63]), &[0x0, 0x1])
            .has_value(&leaf1_path, &value1)
            .has_value(&leaf2_path, &value2);
    }

    #[test]
    fn test_hoodie_block_1_data() {
        // Reveal node at path Nibbles(0x) - root branch node
        let root_branch_stack = vec![
            hex!("a0550b6aba4dd4582a2434d2cbdad8d3007d09f622d7a6e6eaa7a49385823c2fa2"),
            hex!("a04788a4975a9e1efd29b834fd80fdfe8a57cc1b1c5ace6d30ce5a36a15e0092b3"),
            hex!("a093aeccf87da304e6f7d09edc5d7bd3a552808866d2149dd0940507a8f9bfa910"),
            hex!("a08b5b423ba68d0dec2eca1f408076f9170678505eb4a5db2abbbd83bb37666949"),
            hex!("a08592f62216af4218098a78acad7cf472a727fb55e6c27d3cfdf2774d4518eb83"),
            hex!("a0ef02aeee845cb64c11f85edc1a3094227c26445952554b8a9248915d80c746c3"),
            hex!("a0df2529ee3a1ce4df5a758cf17e6a86d0fb5ea22ab7071cf60af6412e9b0a428a"),
            hex!("a0acaa1092db69cd5a63676685827b3484c4b80dc1d3361f6073bbb9240101e144"),
            hex!("a09c3f2bb2a729d71f246a833353ade65667716bb330e0127a3299a42d11200f93"),
            hex!("a0ce978470f4c0b1f8069570563a14d2b79d709add2db4bf22dd9b6aed3271c566"),
            hex!("a095f783cd1d464a60e3c8adcadc28c6eb9fec7306664df39553be41dccc909606"),
            hex!("a0a9083f5fb914b255e1feb5d951a4dfddacf3c8003ef1d1ec6a13bb6ba5b2ac62"),
            hex!("a0fec113d537d8577cd361e0cabf5e95ef58f1cc34318292fdecce9fae57c3e094"),
            hex!("a08b7465f5fe8b3e3c0d087cb7521310d4065ef2a0ee43bf73f68dee8a5742b3dd"),
            hex!("a0c589aa1ae3d5fd87d8640957f7d5184a4ac06f393b453a8e8ed7e8fba0d385c8"),
            hex!("a0b516d6f3352f87beab4ed6e7322f191fc7a147686500ef4de7dd290ad784ef51"),
        ];

        let root_branch_rlp_stack: Vec<RlpNode> = root_branch_stack
            .iter()
            .map(|hex_str| RlpNode::from_raw_rlp(&hex_str[..]).unwrap())
            .collect();

        let root_branch_node = BranchNodeV2::new(
            Default::default(),
            root_branch_rlp_stack,
            TrieMask::new(0b1111111111111111), // state_mask: all 16 children present
            None,
        );

        let root_branch_masks = Some(BranchNodeMasks {
            hash_mask: TrieMask::new(0b1111111111111111),
            tree_mask: TrieMask::new(0b1111111111111111),
        });

        let mut trie = ParallelSparseTrie::from_root(
            TrieNodeV2::Branch(root_branch_node),
            root_branch_masks,
            true,
        )
        .unwrap();

        // Reveal node at path Nibbles(0x3) - branch node
        let branch_0x3_stack = vec![
            hex!("a09da7d9755fe0c558b3c3de9fdcdf9f28ae641f38c9787b05b73ab22ae53af3e2"),
            hex!("a0d9990bf0b810d1145ecb2b011fd68c63cc85564e6724166fd4a9520180706e5f"),
            hex!("a0f60eb4b12132a40df05d9bbdb88bbde0185a3f097f3c76bf4200c23eda26cf86"),
            hex!("a0ca976997ddaf06f18992f6207e4f6a05979d07acead96568058789017cc6d06b"),
            hex!("a04d78166b48044fdc28ed22d2fd39c8df6f8aaa04cb71d3a17286856f6893ff83"),
            hex!("a021d4f90c34d3f1706e78463b6482bca77a3aa1cd059a3f326c42a1cfd30b9b60"),
            hex!("a0fc3b71c33e2e6b77c5e494c1db7fdbb447473f003daf378c7a63ba9bf3f0049d"),
            hex!("a0e33ed2be194a3d93d343e85642447c93a9d0cfc47a016c2c23d14c083be32a7c"),
            hex!("a07b8e7a21c1178d28074f157b50fca85ee25c12568ff8e9706dcbcdacb77bf854"),
            hex!("a0973274526811393ea0bf4811ca9077531db00d06b86237a2ecd683f55ba4bcb0"),
            hex!("a03a93d726d7487874e51b52d8d534c63aa2a689df18e3b307c0d6cb0a388b00f3"),
            hex!("a06aa67101d011d1c22fe739ef83b04b5214a3e2f8e1a2625d8bfdb116b447e86f"),
            hex!("a02dd545b33c62d33a183e127a08a4767fba891d9f3b94fc20a2ca02600d6d1fff"),
            hex!("a0fe6db87d00f06d53bff8169fa497571ff5af1addfb715b649b4d79dd3e394b04"),
            hex!("a0d9240a9d2d5851d05a97ff3305334dfdb0101e1e321fc279d2bb3cad6afa8fc8"),
            hex!("a01b69c6ab5173de8a8ec53a6ebba965713a4cc7feb86cb3e230def37c230ca2b2"),
        ];

        let branch_0x3_rlp_stack: Vec<RlpNode> = branch_0x3_stack
            .iter()
            .map(|hex_str| RlpNode::from_raw_rlp(&hex_str[..]).unwrap())
            .collect();

        let branch_0x3_node = BranchNodeV2::new(
            Default::default(),
            branch_0x3_rlp_stack,
            TrieMask::new(0b1111111111111111), // state_mask: all 16 children present
            None,
        );

        let branch_0x3_masks = Some(BranchNodeMasks {
            hash_mask: TrieMask::new(0b0100010000010101),
            tree_mask: TrieMask::new(0b0100000000000000),
        });

        // Reveal node at path Nibbles(0x37) - leaf node
        let leaf_path = Nibbles::from_nibbles([0x3, 0x7]);
        let leaf_key = Nibbles::unpack(
            &hex!("d65eaa92c6bc4c13a5ec45527f0c18ea8932588728769ec7aecfe6d9f32e42")[..],
        );
        let leaf_value = hex!("f8440180a056e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421a0f57acd40259872606d76197ef052f3d35588dadf919ee1f0e3cb9b62d3f4b02c").to_vec();

        let leaf_node = LeafNode::new(leaf_key, leaf_value);
        let leaf_masks = None;

        trie.reveal_nodes(&mut [
            ProofTrieNodeV2 {
                path: Nibbles::from_nibbles([0x3]),
                node: TrieNodeV2::Branch(branch_0x3_node),
                masks: branch_0x3_masks,
            },
            ProofTrieNodeV2 {
                path: leaf_path,
                node: TrieNodeV2::Leaf(leaf_node),
                masks: leaf_masks,
            },
        ])
        .unwrap();

        // Update leaf with its new value
        let mut leaf_full_path = leaf_path;
        leaf_full_path.extend(&leaf_key);

        let leaf_new_value = vec![
            248, 68, 1, 128, 160, 224, 163, 152, 169, 122, 160, 155, 102, 53, 41, 0, 47, 28, 205,
            190, 199, 5, 215, 108, 202, 22, 138, 70, 196, 178, 193, 208, 18, 96, 95, 63, 238, 160,
            245, 122, 205, 64, 37, 152, 114, 96, 109, 118, 25, 126, 240, 82, 243, 211, 85, 136,
            218, 223, 145, 158, 225, 240, 227, 203, 155, 98, 211, 244, 176, 44,
        ];

        trie.update_leaf(leaf_full_path, leaf_new_value.clone(), DefaultTrieNodeProvider).unwrap();

        // Sanity checks before calculating the root
        assert_eq!(
            Some(&leaf_new_value),
            trie.lower_subtrie_for_path(&leaf_path).unwrap().inner.values.get(&leaf_full_path)
        );
        assert!(trie.upper_subtrie.inner.values.is_empty());

        // Assert the root hash matches the expected value
        let expected_root =
            b256!("0x29b07de8376e9ce7b3a69e9b102199869514d3f42590b5abc6f7d48ec9b8665c");
        assert_eq!(trie.root(), expected_root);
    }

    #[test]
    fn find_leaf_existing_leaf() {
        // Create a simple trie with one leaf
        let provider = DefaultTrieNodeProvider;
        let mut sparse = ParallelSparseTrie::default();
        let path = pad_nibbles_right(Nibbles::from_nibbles([0x1, 0x2, 0x3]));
        let value = b"test_value".to_vec();

        sparse.update_leaf(path, value.clone(), &provider).unwrap();

        // Check that the leaf exists
        let result = sparse.find_leaf(&path, None);
        assert_matches!(result, Ok(LeafLookup::Exists));

        // Check with expected value matching
        let result = sparse.find_leaf(&path, Some(&value));
        assert_matches!(result, Ok(LeafLookup::Exists));
    }

    #[test]
    fn find_leaf_value_mismatch() {
        // Create a simple trie with one leaf
        let provider = DefaultTrieNodeProvider;
        let mut sparse = ParallelSparseTrie::default();
        let path = pad_nibbles_right(Nibbles::from_nibbles([0x1, 0x2, 0x3]));
        let value = b"test_value".to_vec();
        let wrong_value = b"wrong_value".to_vec();

        sparse.update_leaf(path, value, &provider).unwrap();

        // Check with wrong expected value
        let result = sparse.find_leaf(&path, Some(&wrong_value));
        assert_matches!(
            result,
            Err(LeafLookupError::ValueMismatch { path: p, expected: Some(e), actual: _a }) if p == path && e == wrong_value
        );
    }

    #[test]
    fn find_leaf_not_found_empty_trie() {
        // Empty trie
        let sparse = ParallelSparseTrie::default();
        let path = Nibbles::from_nibbles([0x1, 0x2, 0x3]);

        // Leaf should not exist
        let result = sparse.find_leaf(&path, None);
        assert_matches!(result, Ok(LeafLookup::NonExistent));
    }

    #[test]
    fn find_leaf_empty_trie() {
        let sparse = ParallelSparseTrie::default();
        let path = Nibbles::from_nibbles_unchecked([0x1, 0x2, 0x3, 0x4]);

        let result = sparse.find_leaf(&path, None);
        assert_matches!(result, Ok(LeafLookup::NonExistent));
    }

    #[test]
    fn find_leaf_exists_no_value_check() {
        let provider = DefaultTrieNodeProvider;
        let mut sparse = ParallelSparseTrie::default();
        let path = pad_nibbles_right(Nibbles::from_nibbles([0x1, 0x2, 0x3, 0x4]));
        sparse.update_leaf(path, encode_account_value(0), &provider).unwrap();

        let result = sparse.find_leaf(&path, None);
        assert_matches!(result, Ok(LeafLookup::Exists));
    }

    #[test]
    fn find_leaf_exists_with_value_check_ok() {
        let provider = DefaultTrieNodeProvider;
        let mut sparse = ParallelSparseTrie::default();
        let path = pad_nibbles_right(Nibbles::from_nibbles([0x1, 0x2, 0x3, 0x4]));
        let value = encode_account_value(0);
        sparse.update_leaf(path, value.clone(), &provider).unwrap();

        let result = sparse.find_leaf(&path, Some(&value));
        assert_matches!(result, Ok(LeafLookup::Exists));
    }

    #[test]
    fn find_leaf_exclusion_branch_divergence() {
        let provider = DefaultTrieNodeProvider;
        let mut sparse = ParallelSparseTrie::default();
        let path1 = pad_nibbles_right(Nibbles::from_nibbles([0x1, 0x2, 0x3, 0x4])); // Creates branch at 0x12
        let path2 = pad_nibbles_right(Nibbles::from_nibbles([0x1, 0x2, 0x5, 0x6])); // Belongs to same branch
        let search_path = pad_nibbles_right(Nibbles::from_nibbles([0x1, 0x2, 0x7, 0x8])); // Diverges at nibble 7

        sparse.update_leaf(path1, encode_account_value(0), &provider).unwrap();
        sparse.update_leaf(path2, encode_account_value(1), &provider).unwrap();

        let result = sparse.find_leaf(&search_path, None);
        assert_matches!(result, Ok(LeafLookup::NonExistent))
    }

    #[test]
    fn find_leaf_exclusion_extension_divergence() {
        let provider = DefaultTrieNodeProvider;
        let mut sparse = ParallelSparseTrie::default();
        // This will create an extension node at root with key 0x12
        let path1 = pad_nibbles_right(Nibbles::from_nibbles([0x1, 0x2, 0x3, 0x4, 0x5, 0x6]));
        // This path diverges from the extension key
        let search_path = pad_nibbles_right(Nibbles::from_nibbles([0x1, 0x2, 0x7, 0x8]));

        sparse.update_leaf(path1, encode_account_value(0), &provider).unwrap();

        let result = sparse.find_leaf(&search_path, None);
        assert_matches!(result, Ok(LeafLookup::NonExistent))
    }

    #[test]
    fn find_leaf_exclusion_leaf_divergence() {
        let provider = DefaultTrieNodeProvider;
        let mut sparse = ParallelSparseTrie::default();
        let existing_leaf_path = pad_nibbles_right(Nibbles::from_nibbles([0x1, 0x2, 0x3, 0x4]));
        let search_path = pad_nibbles_right(Nibbles::from_nibbles([0x1, 0x2, 0x3, 0x4, 0x5, 0x6]));

        sparse.update_leaf(existing_leaf_path, encode_account_value(0), &provider).unwrap();

        let result = sparse.find_leaf(&search_path, None);
        assert_matches!(result, Ok(LeafLookup::NonExistent))
    }

    #[test]
    fn find_leaf_exclusion_path_ends_at_branch() {
        let provider = DefaultTrieNodeProvider;
        let mut sparse = ParallelSparseTrie::default();
        let path1 = pad_nibbles_right(Nibbles::from_nibbles([0x1, 0x2, 0x3, 0x4])); // Creates branch at 0x12
        let path2 = pad_nibbles_right(Nibbles::from_nibbles([0x1, 0x2, 0x5, 0x6]));
        let search_path = pad_nibbles_right(Nibbles::from_nibbles([0x1, 0x2])); // Path of the branch itself

        sparse.update_leaf(path1, encode_account_value(0), &provider).unwrap();
        sparse.update_leaf(path2, encode_account_value(1), &provider).unwrap();

        let result = sparse.find_leaf(&search_path, None);
        assert_matches!(result, Ok(LeafLookup::NonExistent));
    }

    #[test]
    fn find_leaf_error_blinded_node_at_leaf_path() {
        // Scenario: The node *at* the leaf path is blinded.
        let blinded_hash = B256::repeat_byte(0xBB);
        let leaf_path = Nibbles::from_nibbles_unchecked([0x1, 0x2, 0x3, 0x4]);

        let sparse = new_test_trie(
            [
                (
                    // Ext 0x12
                    Nibbles::default(),
                    SparseNode::new_ext(Nibbles::from_nibbles_unchecked([0x1, 0x2])),
                ),
                (
                    // Ext 0x123
                    Nibbles::from_nibbles_unchecked([0x1, 0x2]),
                    SparseNode::new_ext(Nibbles::from_nibbles_unchecked([0x3])),
                ),
                (
                    // Branch at 0x123, child 4
                    Nibbles::from_nibbles_unchecked([0x1, 0x2, 0x3]),
                    SparseNode::new_branch(TrieMask::new(0b10000), &[(0x4, blinded_hash)]),
                ),
            ]
            .into_iter(),
        );

        let result = sparse.find_leaf(&leaf_path, None);

        // Should error because it hit the blinded node exactly at the leaf path
        assert_matches!(result, Err(LeafLookupError::BlindedNode { path, hash })
            if path == leaf_path && hash == blinded_hash
        );
    }

    #[test]
    fn find_leaf_error_blinded_node() {
        let blinded_hash = B256::repeat_byte(0xAA);
        let path_to_blind = Nibbles::from_nibbles_unchecked([0x1]);
        let search_path = Nibbles::from_nibbles_unchecked([0x1, 0x2, 0x3, 0x4]);

        let sparse = new_test_trie(
            [
                // Root is a branch with child 0x1 (blinded) and 0x5 (revealed leaf)
                // So we set Bit 1 and Bit 5 in the state_mask
                (
                    Nibbles::default(),
                    SparseNode::new_branch(TrieMask::new(0b100010), &[(0x1, blinded_hash)]),
                ),
                (
                    Nibbles::from_nibbles_unchecked([0x5]),
                    SparseNode::new_leaf(Nibbles::from_nibbles_unchecked([0x6, 0x7, 0x8])),
                ),
            ]
            .into_iter(),
        );

        let result = sparse.find_leaf(&search_path, None);

        // Should error because it hit the blinded node at path 0x1
        assert_matches!(result, Err(LeafLookupError::BlindedNode { path, hash })
            if path == path_to_blind && hash == blinded_hash
        );
    }

    #[test]
    fn test_mainnet_block_24185431_storage_0x6ba784ee() {
        reth_tracing::init_test_tracing();

        // Reveal branch at 0x3 with full state
        let mut branch_0x3_hashes = vec![
            B256::from(hex!("fc11ba8de4b220b8f19a09f0676c69b8e18bae1350788392640069e59b41733d")),
            B256::from(hex!("8afe085cc6685680bd8ba4bac6e65937a4babf737dc5e7413d21cdda958e8f74")),
            B256::from(hex!("c7b6f7c0fc601a27aece6ec178fd9be17cdee77c4884ecfbe1ee459731eb57da")),
            B256::from(hex!("71c1aec60db78a2deb4e10399b979a2ed5be42b4ee0c0a17c614f9ddc9f9072e")),
            B256::from(hex!("e9261302e7c0b77930eaf1851b585210906cd01e015ab6be0f7f3c0cc947c32a")),
            B256::from(hex!("38ce8f369c56bd77fabdf679b27265b1f8d0a54b09ef612c8ee8ddfc6b3fab95")),
            B256::from(hex!("7b507a8936a28c5776b647d1c4bda0bbbb3d0d227f16c5f5ebba58d02e31918d")),
            B256::from(hex!("0f456b9457a824a81e0eb555aa861461acb38674dcf36959b3b26deb24ed0af9")),
            B256::from(hex!("2145420289652722ad199ba932622e3003c779d694fa5a2acfb2f77b0782b38a")),
            B256::from(hex!("2c1a04dce1a9e2f1cfbf8806edce50a356dfa58e7e7c542c848541502613b796")),
            B256::from(hex!("dad7ca55186ac8f40d4450dc874166df8267b44abc07e684d9507260f5712df3")),
            B256::from(hex!("3a8c2a1d7d2423e92965ec29014634e7f0307ded60b1a63d28c86c3222b24236")),
            B256::from(hex!("4e9929e6728b3a7bf0db6a0750ab376045566b556c9c605e606ecb8ec25200d7")),
            B256::from(hex!("1797c36f98922f52292c161590057a1b5582d5503e3370bcfbf6fd939f3ec98b")),
            B256::from(hex!("9e514589a9c9210b783c19fa3f0b384bbfaefe98f10ea189a2bfc58c6bf000a1")),
            B256::from(hex!("85bdaabbcfa583cbd049650e41d3d19356bd833b3ed585cf225a3548557c7fa3")),
        ];
        let branch_0x3_node = create_branch_node(
            Nibbles::from_nibbles([0x3]),
            &[0x0, 0x1, 0x2, 0x3, 0x4, 0x5, 0x6, 0x7, 0x8, 0x9, 0xa, 0xb, 0xc, 0xd, 0xe, 0xf],
            branch_0x3_hashes.iter().map(RlpNode::word_rlp),
        );

        // Reveal branch at 0x31
        let branch_0x31_hashes = vec![B256::from(hex!(
            "3ca994ba59ce70b83fee1f01731c8dac4fdd0f70ade79bf9b0695c4c53531aab"
        ))];
        let branch_0x31_node = create_branch_node_with_children(
            &[0xc],
            branch_0x31_hashes.into_iter().map(|h| RlpNode::word_rlp(&h)),
        );

        // Reveal leaf at 0x31b0b645a6c4a0a1bb3d2f0c1d31c39f4aba2e3b015928a8eef7161e28388b81
        let leaf_path = hex!("31b0b645a6c4a0a1bb3d2f0c1d31c39f4aba2e3b015928a8eef7161e28388b81");
        let leaf_nibbles = Nibbles::unpack(leaf_path.as_slice());
        let leaf_value = hex!("0009ae8ce8245bff").to_vec();

        // Reveal branch at 0x31c
        let branch_0x31c_hashes = vec![
            B256::from(hex!("1a68fdb36b77e9332b49a977faf800c22d0199e6cecf44032bb083c78943e540")),
            B256::from(hex!("cd4622c6df6fd7172c7fed1b284ef241e0f501b4c77b675ef10c612bd0948a7a")),
            B256::from(hex!("abf3603d2f991787e21f1709ee4c7375d85dfc506995c0435839fccf3fe2add4")),
        ];
        let branch_0x31c_node = create_branch_node_with_children(
            &[0x3, 0x7, 0xc],
            branch_0x31c_hashes.into_iter().map(|h| RlpNode::word_rlp(&h)),
        );

        // Reveal the trie structure using ProofTrieNode
        let mut proof_nodes = vec![ProofTrieNodeV2 {
            path: Nibbles::from_nibbles([0x3, 0x1]),
            node: branch_0x31_node,
            masks: Some(BranchNodeMasks {
                tree_mask: TrieMask::new(4096),
                hash_mask: TrieMask::new(4096),
            }),
        }];

        // Create a sparse trie and reveal nodes
        let mut trie = ParallelSparseTrie::default()
            .with_root(
                branch_0x3_node,
                Some(BranchNodeMasks {
                    tree_mask: TrieMask::new(26099),
                    hash_mask: TrieMask::new(65535),
                }),
                true,
            )
            .expect("root revealed");

        trie.reveal_nodes(&mut proof_nodes).unwrap();

        // Update the leaf in order to reveal it in the trie
        trie.update_leaf(leaf_nibbles, leaf_value, NoRevealProvider).unwrap();

        // Now try deleting the leaf
        let Err(err) = trie.remove_leaf(&leaf_nibbles, NoRevealProvider) else {
            panic!("expected blinded node error");
        };
        assert_matches!(err.kind(), SparseTrieErrorKind::BlindedNode(path) if path == &Nibbles::from_nibbles([0x3, 0x1, 0xc]));

        trie.reveal_nodes(&mut [ProofTrieNodeV2 {
            path: Nibbles::from_nibbles([0x3, 0x1, 0xc]),
            node: branch_0x31c_node,
            masks: Some(BranchNodeMasks { tree_mask: 0.into(), hash_mask: 4096.into() }),
        }])
        .unwrap();

        // Now remove the leaf again, this should succeed
        trie.remove_leaf(&leaf_nibbles, NoRevealProvider).unwrap();

        // Compute the root to trigger updates
        let _ = trie.root();

        // Assert the resulting branch node updates
        let updates = trie.updates_ref();

        // Check that the branch at 0x3 was updated with the expected structure
        let branch_0x3_update = updates
            .updated_nodes
            .get(&Nibbles::from_nibbles([0x3]))
            .expect("Branch at 0x3 should be in updates");

        // We no longer expect to track the hash for child 1
        branch_0x3_hashes.remove(1);

        // Expected structure from prompt.md
        let expected_branch = BranchNodeCompact::new(
            0b1111111111111111,
            0b0110010111110011,
            0b1111111111111101,
            branch_0x3_hashes,
            None,
        );

        assert_eq!(branch_0x3_update, &expected_branch);
    }

    #[test]
    fn test_get_leaf_value_lower_subtrie() {
        // This test demonstrates that get_leaf_value must look in the correct subtrie,
        // not always in upper_subtrie.

        // Set up a root branch pointing to nibble 0x1, and a branch at [0x1] pointing to
        // nibble 0x2, so that the lower subtrie at [0x1, 0x2] is reachable.
        let root_branch =
            create_branch_node_with_children(&[0x1], [RlpNode::word_rlp(&B256::repeat_byte(0xAA))]);
        let branch_at_1 =
            create_branch_node_with_children(&[0x2], [RlpNode::word_rlp(&B256::repeat_byte(0xBB))]);
        let mut trie = ParallelSparseTrie::from_root(root_branch, None, false).unwrap();
        trie.reveal_nodes(&mut [ProofTrieNodeV2 {
            path: Nibbles::from_nibbles([0x1]),
            node: branch_at_1,
            masks: None,
        }])
        .unwrap();

        // Create a leaf node with path >= 2 nibbles (will go to lower subtrie)
        let leaf_path = Nibbles::from_nibbles([0x1, 0x2]);
        let leaf_key = Nibbles::from_nibbles([0x3, 0x4]);
        let leaf_node = create_leaf_node(leaf_key.to_vec(), 42);

        // Reveal the leaf node
        trie.reveal_nodes(&mut [ProofTrieNodeV2 { path: leaf_path, node: leaf_node, masks: None }])
            .unwrap();

        // The full path is leaf_path + leaf_key
        let full_path = Nibbles::from_nibbles([0x1, 0x2, 0x3, 0x4]);

        // Verify the value is stored in the lower subtrie, not upper
        let idx = path_subtrie_index_unchecked(&leaf_path);
        let lower_subtrie = trie.lower_subtries[idx].as_revealed_ref().unwrap();
        assert!(
            lower_subtrie.inner.values.contains_key(&full_path),
            "value should be in lower subtrie"
        );
        assert!(
            !trie.upper_subtrie.inner.values.contains_key(&full_path),
            "value should NOT be in upper subtrie"
        );

        // get_leaf_value should find the value
        assert!(
            trie.get_leaf_value(&full_path).is_some(),
            "get_leaf_value should find the value in lower subtrie"
        );
    }

    /// Test that `get_leaf_value` correctly returns values stored via `update_leaf`
    /// when the leaf node ends up in the upper subtrie (depth < 2).
    ///
    /// This can happen when the trie is sparse and the leaf is inserted at the root level.
    /// Previously, `get_leaf_value` only checked the lower subtrie based on the full path,
    /// missing values stored in `upper_subtrie.inner.values`.
    #[test]
    fn test_get_leaf_value_upper_subtrie_via_update_leaf() {
        let provider = NoRevealProvider;

        // Create an empty trie with an empty root
        let mut trie = ParallelSparseTrie::default()
            .with_root(TrieNodeV2::EmptyRoot, None, false)
            .expect("root revealed");

        // Create a full 64-nibble path (like a real account hash)
        let full_path = pad_nibbles_right(Nibbles::from_nibbles([0x0, 0xA, 0xB, 0xC]));
        let value = encode_account_value(42);

        // Insert the leaf - since the trie is empty, the leaf node will be created
        // at the root level (depth 0), which is in the upper subtrie
        trie.update_leaf(full_path, value.clone(), provider).unwrap();

        // Verify the value is stored in upper_subtrie (where update_leaf puts it)
        assert!(
            trie.upper_subtrie.inner.values.contains_key(&full_path),
            "value should be in upper subtrie after update_leaf"
        );

        // Verify the value can be retrieved via get_leaf_value
        // Before the fix, this would return None because get_leaf_value only
        // checked the lower subtrie based on the path length
        let retrieved = trie.get_leaf_value(&full_path);
        assert_eq!(retrieved, Some(&value));
    }

    /// Test that `get_leaf_value` works for values in both upper and lower subtries.
    #[test]
    fn test_get_leaf_value_upper_and_lower_subtries() {
        let provider = NoRevealProvider;

        // Create an empty trie
        let mut trie = ParallelSparseTrie::default()
            .with_root(TrieNodeV2::EmptyRoot, None, false)
            .expect("root revealed");

        // Insert first leaf - will be at root level (upper subtrie)
        let path1 = pad_nibbles_right(Nibbles::from_nibbles([0x0, 0xA]));
        let value1 = encode_account_value(1);
        trie.update_leaf(path1, value1.clone(), provider).unwrap();

        // Insert second leaf with different prefix - creates a branch
        let path2 = pad_nibbles_right(Nibbles::from_nibbles([0x1, 0xB]));
        let value2 = encode_account_value(2);
        trie.update_leaf(path2, value2.clone(), provider).unwrap();

        // Both values should be retrievable
        assert_eq!(trie.get_leaf_value(&path1), Some(&value1));
        assert_eq!(trie.get_leaf_value(&path2), Some(&value2));
    }

    /// Test that `get_leaf_value` works for storage tries which are often very sparse.
    #[test]
    fn test_get_leaf_value_sparse_storage_trie() {
        let provider = NoRevealProvider;

        // Simulate a storage trie with a single slot
        let mut trie = ParallelSparseTrie::default()
            .with_root(TrieNodeV2::EmptyRoot, None, false)
            .expect("root revealed");

        // Single storage slot - leaf will be at root (depth 0)
        let slot_path = pad_nibbles_right(Nibbles::from_nibbles([0x2, 0x9]));
        let slot_value = alloy_rlp::encode(U256::from(12345));
        trie.update_leaf(slot_path, slot_value.clone(), provider).unwrap();

        // Value should be retrievable
        assert_eq!(trie.get_leaf_value(&slot_path), Some(&slot_value));
    }

    #[test]
    fn test_prune_empty_suffix_key_regression() {
        // Regression test: when a leaf has an empty suffix key (full path == node path),
        // the value must be removed when that path becomes a pruned root.
        // This catches the bug where is_strict_descendant fails to remove p == pruned_root.

        use crate::provider::DefaultTrieNodeProvider;

        let provider = DefaultTrieNodeProvider;
        let mut parallel = ParallelSparseTrie::default();

        // Large value to ensure nodes have hashes (RLP >= 32 bytes)
        let value = {
            let account = Account {
                nonce: 0x123456789abcdef,
                balance: U256::from(0x123456789abcdef0123456789abcdef_u128),
                ..Default::default()
            };
            let mut buf = Vec::new();
            account.into_trie_account(EMPTY_ROOT_HASH).encode(&mut buf);
            buf
        };

        // Create a trie with multiple leaves to force a branch at root
        for i in 0..16u8 {
            parallel
                .update_leaf(
                    pad_nibbles_right(Nibbles::from_nibbles([i, 0x1, 0x2, 0x3, 0x4, 0x5])),
                    value.clone(),
                    &provider,
                )
                .unwrap();
        }

        // Compute root to get hashes
        let root_before = parallel.root();

        // Prune at depth 0: the children of root become pruned roots
        parallel.prune(0);

        let root_after = parallel.root();
        assert_eq!(root_before, root_after, "root hash must be preserved");

        // Key assertion: values under pruned paths must be removed
        // With the bug, values at pruned_root paths (not strict descendants) would remain
        for i in 0..16u8 {
            let path = pad_nibbles_right(Nibbles::from_nibbles([i, 0x1, 0x2, 0x3, 0x4, 0x5]));
            assert!(
                parallel.get_leaf_value(&path).is_none(),
                "value at {:?} should be removed after prune",
                path
            );
        }
    }

    #[test]
    fn test_prune_at_various_depths() {
        // Test depths 0 and 1, which are in the Upper subtrie (no heat tracking).
        // Depth 2 is the boundary where Lower subtries start (UPPER_TRIE_MAX_DEPTH=2),
        // and with `depth >= max_depth` heat check, hot Lower subtries at depth 2
        // are protected from pruning traversal.
        for max_depth in [0, 1] {
            let provider = DefaultTrieNodeProvider;
            let mut trie = ParallelSparseTrie::default();

            let value = large_account_value();

            for i in 0..4u8 {
                for j in 0..4u8 {
                    for k in 0..4u8 {
                        trie.update_leaf(
                            pad_nibbles_right(Nibbles::from_nibbles([i, j, k, 0x1, 0x2, 0x3])),
                            value.clone(),
                            &provider,
                        )
                        .unwrap();
                    }
                }
            }

            let root_before = trie.root();
            let nodes_before = trie.size_hint();

            // Prune multiple times to allow heat to fully decay.
            // Heat starts at 1 and decays by 1 each cycle for unmodified subtries,
            // so we need 2 prune cycles: 1→0, then actual prune.
            for _ in 0..2 {
                trie.prune(max_depth);
            }

            let root_after = trie.root();
            assert_eq!(root_before, root_after, "root hash should be preserved after prune");

            let nodes_after = trie.size_hint();
            assert!(
                nodes_after < nodes_before,
                "node count should decrease after prune at depth {max_depth}"
            );

            if max_depth == 0 {
                // Root with 4 blinded hashes for children at [0], [1], [2], [3]
                assert_eq!(nodes_after, 1, "root");
            }
        }
    }

    #[test]
    fn test_prune_empty_trie() {
        let mut trie = ParallelSparseTrie::default();
        trie.prune(2);
        let root = trie.root();
        assert_eq!(root, EMPTY_ROOT_HASH, "empty trie should have empty root hash");
    }

    #[test]
    fn test_prune_preserves_root_hash() {
        let provider = DefaultTrieNodeProvider;
        let mut trie = ParallelSparseTrie::default();

        let value = large_account_value();

        for i in 0..8u8 {
            for j in 0..4u8 {
                trie.update_leaf(
                    pad_nibbles_right(Nibbles::from_nibbles([i, j, 0x3, 0x4, 0x5, 0x6])),
                    value.clone(),
                    &provider,
                )
                .unwrap();
            }
        }

        let root_before = trie.root();
        trie.prune(1);
        let root_after = trie.root();
        assert_eq!(root_before, root_after, "root hash must be preserved after prune");
    }

    #[test]
    fn test_prune_single_leaf_trie() {
        let provider = DefaultTrieNodeProvider;
        let mut trie = ParallelSparseTrie::default();

        let value = large_account_value();
        trie.update_leaf(
            pad_nibbles_right(Nibbles::from_nibbles([0x1, 0x2, 0x3, 0x4])),
            value,
            &provider,
        )
        .unwrap();

        let root_before = trie.root();
        let nodes_before = trie.size_hint();

        trie.prune(0);

        let root_after = trie.root();
        assert_eq!(root_before, root_after, "root hash should be preserved");
        assert_eq!(trie.size_hint(), nodes_before, "single leaf trie should not change");
    }

    #[test]
    fn test_prune_deep_depth_no_effect() {
        let provider = DefaultTrieNodeProvider;
        let mut trie = ParallelSparseTrie::default();

        let value = large_account_value();

        for i in 0..4u8 {
            trie.update_leaf(
                pad_nibbles_right(Nibbles::from_nibbles([i, 0x2, 0x3, 0x4])),
                value.clone(),
                &provider,
            )
            .unwrap();
        }

        trie.root();
        let nodes_before = trie.size_hint();

        trie.prune(100);

        assert_eq!(nodes_before, trie.size_hint(), "deep prune should have no effect");
    }

    #[test]
    fn test_prune_extension_node_depth_semantics() {
        let provider = DefaultTrieNodeProvider;
        let mut trie = ParallelSparseTrie::default();

        let value = large_account_value();

        trie.update_leaf(
            pad_nibbles_right(Nibbles::from_nibbles([0, 1, 2, 3, 0, 5, 6, 7])),
            value.clone(),
            &provider,
        )
        .unwrap();
        trie.update_leaf(
            pad_nibbles_right(Nibbles::from_nibbles([0, 1, 2, 3, 1, 5, 6, 7])),
            value,
            &provider,
        )
        .unwrap();

        let root_before = trie.root();
        // Prune multiple times to allow heat to fully decay.
        // Heat starts at 1 and decays by 1 each cycle for unmodified subtries,
        // so we need 2 prune cycles: 1→0, then actual prune.
        for _ in 0..2 {
            trie.prune(1);
        }

        assert_eq!(root_before, trie.root(), "root hash should be preserved");
        // Root + branch
        assert_eq!(trie.size_hint(), 2, "root + extension + hash stubs after prune(1)");
    }

    #[test]
    fn test_prune_root_hash_preserved() {
        let provider = DefaultTrieNodeProvider;
        let mut trie = ParallelSparseTrie::default();

        // Create two 64-nibble paths that differ only in the first nibble
        let key1 = Nibbles::unpack(B256::repeat_byte(0x00));
        let key2 = Nibbles::unpack(B256::repeat_byte(0x11));

        let large_value = large_account_value();
        trie.update_leaf(key1, large_value.clone(), &provider).unwrap();
        trie.update_leaf(key2, large_value, &provider).unwrap();

        let root_before = trie.root();

        trie.prune(0);

        assert_eq!(root_before, trie.root(), "root hash must be preserved after pruning");
    }

    #[test]
    fn test_prune_mixed_embedded_and_hashed() {
        let provider = DefaultTrieNodeProvider;
        let mut trie = ParallelSparseTrie::default();

        let large_value = large_account_value();
        let small_value = vec![0x80];

        for i in 0..8u8 {
            let value = if i < 4 { large_value.clone() } else { small_value.clone() };
            trie.update_leaf(
                pad_nibbles_right(Nibbles::from_nibbles([i, 0x1, 0x2, 0x3])),
                value,
                &provider,
            )
            .unwrap();
        }

        let root_before = trie.root();
        trie.prune(0);
        assert_eq!(root_before, trie.root(), "root hash must be preserved");
    }

    #[test]
    fn test_prune_many_lower_subtries() {
        let provider = DefaultTrieNodeProvider;

        let large_value = large_account_value();

        let mut keys = Vec::new();
        for first in 0..16u8 {
            for second in 0..16u8 {
                keys.push(pad_nibbles_right(Nibbles::from_nibbles([
                    first, second, 0x1, 0x2, 0x3, 0x4,
                ])));
            }
        }

        let mut trie = ParallelSparseTrie::default();

        for key in &keys {
            trie.update_leaf(*key, large_value.clone(), &provider).unwrap();
        }

        let root_before = trie.root();

        // Prune multiple times to allow heat to fully decay.
        // Heat starts at 1 and decays by 1 each cycle for unmodified subtries.
        let mut total_pruned = 0;
        for _ in 0..2 {
            total_pruned += trie.prune(1);
        }

        assert!(total_pruned > 0, "should have pruned some nodes");
        assert_eq!(root_before, trie.root(), "root hash should be preserved");

        for key in &keys {
            assert!(trie.get_leaf_value(key).is_none(), "value should be pruned");
        }
    }

    #[test]
    fn test_prune_max_depth_overflow() {
        // Verify that max_depth > 255 is not truncated (was u8, now usize)
        let provider = DefaultTrieNodeProvider;
        let mut trie = ParallelSparseTrie::default();

        let value = large_account_value();

        for i in 0..4u8 {
            trie.update_leaf(
                pad_nibbles_right(Nibbles::from_nibbles([i, 0x1, 0x2, 0x3])),
                value.clone(),
                &provider,
            )
            .unwrap();
        }

        trie.root();
        let nodes_before = trie.size_hint();

        // If depth were truncated to u8, 300 would become 44 and might prune something
        trie.prune(300);

        assert_eq!(
            nodes_before,
            trie.size_hint(),
            "prune(300) should have no effect on a shallow trie"
        );
    }

    #[test]
    fn test_prune_fast_path_case2_update_after() {
        // Test fast-path Case 2: upper prune root is prefix of lower subtrie.
        // After pruning, we should be able to update leaves without panic.
        let provider = DefaultTrieNodeProvider;
        let mut trie = ParallelSparseTrie::default();

        let value = large_account_value();

        // Create keys that span into lower subtries (path.len() >= UPPER_TRIE_MAX_DEPTH)
        // UPPER_TRIE_MAX_DEPTH is typically 2, so paths of length 3+ go to lower subtries
        for first in 0..4u8 {
            for second in 0..4u8 {
                trie.update_leaf(
                    pad_nibbles_right(Nibbles::from_nibbles([
                        first, second, 0x1, 0x2, 0x3, 0x4, 0x5, 0x6,
                    ])),
                    value.clone(),
                    &provider,
                )
                .unwrap();
            }
        }

        let root_before = trie.root();

        // Prune at depth 0 - upper roots become prefixes of lower subtrie paths
        trie.prune(0);

        let root_after = trie.root();
        assert_eq!(root_before, root_after, "root hash should be preserved");

        // Now try to update a leaf - this should not panic even though lower subtries
        // were replaced with Blind(None)
        let new_path =
            pad_nibbles_right(Nibbles::from_nibbles([0x5, 0x5, 0x1, 0x2, 0x3, 0x4, 0x5, 0x6]));
        trie.update_leaf(new_path, value, &provider).unwrap();

        // The trie should still be functional
        let _ = trie.root();
    }

    // update_leaves tests

    #[test]
    fn test_update_leaves_successful_update() {
        use crate::LeafUpdate;
        use alloy_primitives::map::B256Map;
        use std::cell::RefCell;

        let provider = DefaultTrieNodeProvider;
        let mut trie = ParallelSparseTrie::default();

        // Create a leaf in the trie using a full-length key
        let b256_key = B256::with_last_byte(42);
        let key = Nibbles::unpack(b256_key);
        let value = encode_account_value(1);
        trie.update_leaf(key, value, &provider).unwrap();

        // Create update map with a new value for the same key
        let new_value = encode_account_value(2);

        let mut updates: B256Map<LeafUpdate> = B256Map::default();
        updates.insert(b256_key, LeafUpdate::Changed(new_value));

        let proof_targets = RefCell::new(Vec::new());
        trie.update_leaves(&mut updates, |path, min_len| {
            proof_targets.borrow_mut().push((path, min_len));
        })
        .unwrap();

        // Update should succeed: map empty, callback not invoked
        assert!(updates.is_empty(), "Update map should be empty after successful update");
        assert!(
            proof_targets.borrow().is_empty(),
            "Callback should not be invoked for revealed paths"
        );
    }

    #[test]
    fn test_update_leaves_insert_new_leaf() {
        use crate::LeafUpdate;
        use alloy_primitives::map::B256Map;
        use std::cell::RefCell;

        let mut trie = ParallelSparseTrie::default();

        // Insert a NEW leaf (key doesn't exist yet) via update_leaves
        let b256_key = B256::with_last_byte(99);
        let new_value = encode_account_value(42);

        let mut updates: B256Map<LeafUpdate> = B256Map::default();
        updates.insert(b256_key, LeafUpdate::Changed(new_value.clone()));

        let proof_targets = RefCell::new(Vec::new());
        trie.update_leaves(&mut updates, |path, min_len| {
            proof_targets.borrow_mut().push((path, min_len));
        })
        .unwrap();

        // Insert should succeed: map empty, callback not invoked
        assert!(updates.is_empty(), "Update map should be empty after successful insert");
        assert!(
            proof_targets.borrow().is_empty(),
            "Callback should not be invoked for new leaf insert"
        );

        // Verify the leaf was actually inserted
        let full_path = Nibbles::unpack(b256_key);
        assert_eq!(
            trie.get_leaf_value(&full_path),
            Some(&new_value),
            "New leaf value should be retrievable"
        );
    }

    #[test]
    fn test_update_leaves_blinded_node() {
        use crate::LeafUpdate;
        use alloy_primitives::map::B256Map;
        use std::cell::RefCell;

        // Create a trie with a blinded node
        // Use a small value that fits in RLP encoding
        let small_value = alloy_rlp::encode_fixed_size(&U256::from(1)).to_vec();
        let leaf = LeafNode::new(
            Nibbles::default(), // short key for RLP encoding
            small_value,
        );
        let branch = TrieNodeV2::Branch(BranchNodeV2::new(
            Nibbles::default(),
            vec![
                RlpNode::word_rlp(&B256::repeat_byte(1)), // blinded child at 0
                RlpNode::from_raw_rlp(&alloy_rlp::encode(leaf.clone())).unwrap(), // revealed at 1
            ],
            TrieMask::new(0b11),
            None,
        ));

        let mut trie = ParallelSparseTrie::from_root(
            branch.clone(),
            Some(BranchNodeMasks {
                hash_mask: TrieMask::new(0b01),
                tree_mask: TrieMask::default(),
            }),
            false,
        )
        .unwrap();

        // Reveal only the branch and one child, leaving child 0 as a Hash node
        trie.reveal_node(
            Nibbles::default(),
            branch,
            Some(BranchNodeMasks {
                hash_mask: TrieMask::default(),
                tree_mask: TrieMask::new(0b01),
            }),
        )
        .unwrap();
        trie.reveal_node(Nibbles::from_nibbles([0x1]), TrieNodeV2::Leaf(leaf), None).unwrap();

        // The path 0x0... is blinded (Hash node)
        // Create an update targeting the blinded path using a full B256 key
        let b256_key = B256::ZERO; // starts with 0x0...

        let new_value = encode_account_value(42);
        let mut updates: B256Map<LeafUpdate> = B256Map::default();
        updates.insert(b256_key, LeafUpdate::Changed(new_value));

        let proof_targets = RefCell::new(Vec::new());
        let prefix_set_len_before = trie.prefix_set.len();
        trie.update_leaves(&mut updates, |path, min_len| {
            proof_targets.borrow_mut().push((path, min_len));
        })
        .unwrap();

        // Update should remain in map (blinded node)
        assert!(!updates.is_empty(), "Update should remain in map when hitting blinded node");

        // prefix_set should be unchanged after failed update
        assert_eq!(
            trie.prefix_set.len(),
            prefix_set_len_before,
            "prefix_set should be unchanged after failed update on blinded node"
        );

        // Callback should be invoked
        let targets = proof_targets.borrow();
        assert!(!targets.is_empty(), "Callback should be invoked for blinded path");

        // min_len should equal the blinded node's path length (1 nibble)
        assert_eq!(targets[0].1, 1, "min_len should equal blinded node path length");
    }

    #[test]
    fn test_update_leaves_removal() {
        use crate::LeafUpdate;
        use alloy_primitives::map::B256Map;
        use std::cell::RefCell;

        let provider = DefaultTrieNodeProvider;
        let mut trie = ParallelSparseTrie::default();

        // Create two leaves so removal doesn't result in empty trie issues
        // Use full-length keys
        let b256_key1 = B256::with_last_byte(1);
        let b256_key2 = B256::with_last_byte(2);
        let key1 = Nibbles::unpack(b256_key1);
        let key2 = Nibbles::unpack(b256_key2);
        let value = encode_account_value(1);
        trie.update_leaf(key1, value.clone(), &provider).unwrap();
        trie.update_leaf(key2, value, &provider).unwrap();

        // Create an update to remove key1 (empty value = removal)
        let mut updates: B256Map<LeafUpdate> = B256Map::default();
        updates.insert(b256_key1, LeafUpdate::Changed(vec![])); // empty = removal

        let proof_targets = RefCell::new(Vec::new());
        trie.update_leaves(&mut updates, |path, min_len| {
            proof_targets.borrow_mut().push((path, min_len));
        })
        .unwrap();

        // Removal should succeed: map empty
        assert!(updates.is_empty(), "Update map should be empty after successful removal");
    }

    #[test]
    fn test_update_leaves_removal_blinded() {
        use crate::LeafUpdate;
        use alloy_primitives::map::B256Map;
        use std::cell::RefCell;

        // Create a trie with a blinded node
        // Use a small value that fits in RLP encoding
        let small_value = alloy_rlp::encode_fixed_size(&U256::from(1)).to_vec();
        let leaf = LeafNode::new(
            Nibbles::default(), // short key for RLP encoding
            small_value,
        );
        let branch = TrieNodeV2::Branch(BranchNodeV2::new(
            Nibbles::default(),
            vec![
                RlpNode::word_rlp(&B256::repeat_byte(1)), // blinded child at 0
                RlpNode::from_raw_rlp(&alloy_rlp::encode(leaf.clone())).unwrap(), // revealed at 1
            ],
            TrieMask::new(0b11),
            None,
        ));

        let mut trie = ParallelSparseTrie::from_root(
            branch.clone(),
            Some(BranchNodeMasks {
                hash_mask: TrieMask::new(0b01),
                tree_mask: TrieMask::default(),
            }),
            false,
        )
        .unwrap();

        trie.reveal_node(
            Nibbles::default(),
            branch,
            Some(BranchNodeMasks {
                hash_mask: TrieMask::default(),
                tree_mask: TrieMask::new(0b01),
            }),
        )
        .unwrap();
        trie.reveal_node(Nibbles::from_nibbles([0x1]), TrieNodeV2::Leaf(leaf), None).unwrap();

        // Simulate having a known value behind the blinded node
        let b256_key = B256::ZERO; // starts with 0x0...
        let full_path = Nibbles::unpack(b256_key);

        // Insert the value into the trie's values map (simulating we know about it)
        let old_value = encode_account_value(99);
        trie.upper_subtrie.inner.values.insert(full_path, old_value.clone());

        let mut updates: B256Map<LeafUpdate> = B256Map::default();
        updates.insert(b256_key, LeafUpdate::Changed(vec![])); // empty = removal

        let proof_targets = RefCell::new(Vec::new());
        let prefix_set_len_before = trie.prefix_set.len();
        trie.update_leaves(&mut updates, |path, min_len| {
            proof_targets.borrow_mut().push((path, min_len));
        })
        .unwrap();

        // Callback should be invoked
        assert!(
            !proof_targets.borrow().is_empty(),
            "Callback should be invoked when removal hits blinded node"
        );

        // Update should remain in map
        assert!(!updates.is_empty(), "Update should remain in map when removal hits blinded node");

        // Original value should be preserved (reverted)
        assert_eq!(
            trie.upper_subtrie.inner.values.get(&full_path),
            Some(&old_value),
            "Original value should be preserved after failed removal"
        );

        // prefix_set should be unchanged after failed removal
        assert_eq!(
            trie.prefix_set.len(),
            prefix_set_len_before,
            "prefix_set should be unchanged after failed removal on blinded node"
        );
    }

    #[test]
    fn test_update_leaves_removal_branch_collapse_blinded() {
        use crate::LeafUpdate;
        use alloy_primitives::map::B256Map;
        use std::cell::RefCell;

        // Create a branch node at root with two children:
        // - Child at nibble 0: a blinded Hash node
        // - Child at nibble 1: a revealed Leaf node
        let small_value = alloy_rlp::encode_fixed_size(&U256::from(1)).to_vec();
        let leaf = LeafNode::new(Nibbles::default(), small_value);
        let branch = TrieNodeV2::Branch(BranchNodeV2::new(
            Nibbles::default(),
            vec![
                RlpNode::word_rlp(&B256::repeat_byte(1)), // blinded child at nibble 0
                RlpNode::from_raw_rlp(&alloy_rlp::encode(leaf.clone())).unwrap(), /* leaf at nibble 1 */
            ],
            TrieMask::new(0b11),
            None,
        ));

        let mut trie = ParallelSparseTrie::from_root(
            branch.clone(),
            Some(BranchNodeMasks {
                hash_mask: TrieMask::new(0b01), // nibble 0 is hashed
                tree_mask: TrieMask::default(),
            }),
            false,
        )
        .unwrap();

        // Reveal the branch and the leaf at nibble 1, leaving nibble 0 as Hash node
        trie.reveal_node(
            Nibbles::default(),
            branch,
            Some(BranchNodeMasks {
                hash_mask: TrieMask::default(),
                tree_mask: TrieMask::new(0b01),
            }),
        )
        .unwrap();
        trie.reveal_node(Nibbles::from_nibbles([0x1]), TrieNodeV2::Leaf(leaf), None).unwrap();

        // Insert the leaf's value into the values map for the revealed leaf
        // Use B256 key that starts with nibble 1 (0x10 has first nibble = 1)
        let b256_key = B256::with_last_byte(0x10);
        let full_path = Nibbles::unpack(b256_key);
        let leaf_value = encode_account_value(42);
        trie.upper_subtrie.inner.values.insert(full_path, leaf_value.clone());

        // Record state before update_leaves
        let prefix_set_len_before = trie.prefix_set.len();
        let node_count_before = trie.upper_subtrie.nodes.len() +
            trie.lower_subtries
                .iter()
                .filter_map(|s| s.as_revealed_ref())
                .map(|s| s.nodes.len())
                .sum::<usize>();

        let mut updates: B256Map<LeafUpdate> = B256Map::default();
        updates.insert(b256_key, LeafUpdate::Changed(vec![])); // removal

        let proof_targets = RefCell::new(Vec::new());
        trie.update_leaves(&mut updates, |path, min_len| {
            proof_targets.borrow_mut().push((path, min_len));
        })
        .unwrap();

        // Assert: update remains in map (removal blocked by blinded sibling)
        assert!(
            !updates.is_empty(),
            "Update should remain in map when removal would collapse branch with blinded sibling"
        );

        // Assert: callback was invoked for the blinded path
        assert!(
            !proof_targets.borrow().is_empty(),
            "Callback should be invoked for blinded sibling path"
        );

        // Assert: prefix_set unchanged (atomic failure)
        assert_eq!(
            trie.prefix_set.len(),
            prefix_set_len_before,
            "prefix_set should be unchanged after atomic failure"
        );

        // Assert: node count unchanged
        let node_count_after = trie.upper_subtrie.nodes.len() +
            trie.lower_subtries
                .iter()
                .filter_map(|s| s.as_revealed_ref())
                .map(|s| s.nodes.len())
                .sum::<usize>();
        assert_eq!(
            node_count_before, node_count_after,
            "Node count should be unchanged after atomic failure"
        );

        // Assert: the leaf value still exists (not removed)
        assert_eq!(
            trie.upper_subtrie.inner.values.get(&full_path),
            Some(&leaf_value),
            "Leaf value should still exist after failed removal"
        );
    }

    #[test]
    fn test_update_leaves_touched() {
        use crate::LeafUpdate;
        use alloy_primitives::map::B256Map;
        use std::cell::RefCell;

        let provider = DefaultTrieNodeProvider;
        let mut trie = ParallelSparseTrie::default();

        // Create a leaf in the trie using a full-length key
        let b256_key = B256::with_last_byte(42);
        let key = Nibbles::unpack(b256_key);
        let value = encode_account_value(1);
        trie.update_leaf(key, value, &provider).unwrap();

        // Create a Touched update for the existing key
        let mut updates: B256Map<LeafUpdate> = B256Map::default();
        updates.insert(b256_key, LeafUpdate::Touched);

        let proof_targets = RefCell::new(Vec::new());
        let prefix_set_len_before = trie.prefix_set.len();

        trie.update_leaves(&mut updates, |path, min_len| {
            proof_targets.borrow_mut().push((path, min_len));
        })
        .unwrap();

        // Update should be removed (path is accessible)
        assert!(updates.is_empty(), "Touched update should be removed for accessible path");

        // No callback
        assert!(
            proof_targets.borrow().is_empty(),
            "Callback should not be invoked for accessible path"
        );

        // prefix_set should be unchanged since Touched is read-only
        assert_eq!(
            trie.prefix_set.len(),
            prefix_set_len_before,
            "prefix_set should be unchanged for Touched update (read-only)"
        );
    }

    #[test]
    fn test_update_leaves_touched_nonexistent() {
        use crate::LeafUpdate;
        use alloy_primitives::map::B256Map;
        use std::cell::RefCell;

        let mut trie = ParallelSparseTrie::default();

        // Create a Touched update for a key that doesn't exist
        let b256_key = B256::with_last_byte(99);
        let full_path = Nibbles::unpack(b256_key);

        let prefix_set_len_before = trie.prefix_set.len();

        let mut updates: B256Map<LeafUpdate> = B256Map::default();
        updates.insert(b256_key, LeafUpdate::Touched);

        let proof_targets = RefCell::new(Vec::new());
        trie.update_leaves(&mut updates, |path, min_len| {
            proof_targets.borrow_mut().push((path, min_len));
        })
        .unwrap();

        // Update should be removed (path IS accessible - it's just empty)
        assert!(updates.is_empty(), "Touched update should be removed for accessible (empty) path");

        // No callback should be invoked (path is revealed, just empty)
        assert!(
            proof_targets.borrow().is_empty(),
            "Callback should not be invoked for accessible path"
        );

        // prefix_set should NOT be modified (Touched is read-only)
        assert_eq!(
            trie.prefix_set.len(),
            prefix_set_len_before,
            "prefix_set should not be modified by Touched update"
        );

        // No value should be inserted
        assert!(
            trie.get_leaf_value(&full_path).is_none(),
            "No value should exist for non-existent key after Touched update"
        );
    }

    #[test]
    fn test_update_leaves_touched_blinded() {
        use crate::LeafUpdate;
        use alloy_primitives::map::B256Map;
        use std::cell::RefCell;

        // Create a trie with a blinded node
        // Use a small value that fits in RLP encoding
        let small_value = alloy_rlp::encode_fixed_size(&U256::from(1)).to_vec();
        let leaf = LeafNode::new(
            Nibbles::default(), // short key for RLP encoding
            small_value,
        );
        let branch = TrieNodeV2::Branch(BranchNodeV2::new(
            Nibbles::default(),
            vec![
                RlpNode::word_rlp(&B256::repeat_byte(1)), // blinded child at 0
                RlpNode::from_raw_rlp(&alloy_rlp::encode(leaf.clone())).unwrap(), // revealed at 1
            ],
            TrieMask::new(0b11),
            None,
        ));

        let mut trie = ParallelSparseTrie::from_root(
            branch.clone(),
            Some(BranchNodeMasks {
                hash_mask: TrieMask::new(0b01),
                tree_mask: TrieMask::default(),
            }),
            false,
        )
        .unwrap();

        trie.reveal_node(
            Nibbles::default(),
            branch,
            Some(BranchNodeMasks {
                hash_mask: TrieMask::default(),
                tree_mask: TrieMask::new(0b01),
            }),
        )
        .unwrap();
        trie.reveal_node(Nibbles::from_nibbles([0x1]), TrieNodeV2::Leaf(leaf), None).unwrap();

        // Create a Touched update targeting the blinded path using full B256 key
        let b256_key = B256::ZERO; // starts with 0x0...

        let mut updates: B256Map<LeafUpdate> = B256Map::default();
        updates.insert(b256_key, LeafUpdate::Touched);

        let proof_targets = RefCell::new(Vec::new());
        let prefix_set_len_before = trie.prefix_set.len();
        trie.update_leaves(&mut updates, |path, min_len| {
            proof_targets.borrow_mut().push((path, min_len));
        })
        .unwrap();

        // Callback should be invoked
        assert!(!proof_targets.borrow().is_empty(), "Callback should be invoked for blinded path");

        // Update should remain in map
        assert!(!updates.is_empty(), "Touched update should remain in map for blinded path");

        // prefix_set should be unchanged since Touched is read-only
        assert_eq!(
            trie.prefix_set.len(),
            prefix_set_len_before,
            "prefix_set should be unchanged for Touched update on blinded path"
        );
    }

    #[test]
    fn test_update_leaves_deduplication() {
        use crate::LeafUpdate;
        use alloy_primitives::map::B256Map;
        use std::cell::RefCell;

        // Create a trie with a blinded node
        // Use a small value that fits in RLP encoding
        let small_value = alloy_rlp::encode_fixed_size(&U256::from(1)).to_vec();
        let leaf = LeafNode::new(
            Nibbles::default(), // short key for RLP encoding
            small_value,
        );
        let branch = TrieNodeV2::Branch(BranchNodeV2::new(
            Nibbles::default(),
            vec![
                RlpNode::word_rlp(&B256::repeat_byte(1)), // blinded child at 0
                RlpNode::from_raw_rlp(&alloy_rlp::encode(leaf.clone())).unwrap(), // revealed at 1
            ],
            TrieMask::new(0b11),
            None,
        ));

        let mut trie = ParallelSparseTrie::from_root(
            branch.clone(),
            Some(BranchNodeMasks {
                hash_mask: TrieMask::new(0b01),
                tree_mask: TrieMask::default(),
            }),
            false,
        )
        .unwrap();

        trie.reveal_node(
            Nibbles::default(),
            branch,
            Some(BranchNodeMasks {
                hash_mask: TrieMask::default(),
                tree_mask: TrieMask::new(0b01),
            }),
        )
        .unwrap();
        trie.reveal_node(Nibbles::from_nibbles([0x1]), TrieNodeV2::Leaf(leaf), None).unwrap();

        // Create multiple updates that would all hit the same blinded node at path 0x0
        // Use full B256 keys that all start with 0x0
        let b256_key1 = B256::ZERO;
        let b256_key2 = B256::with_last_byte(1); // still starts with 0x0
        let b256_key3 = B256::with_last_byte(2); // still starts with 0x0

        let mut updates: B256Map<LeafUpdate> = B256Map::default();
        let value = encode_account_value(42);

        updates.insert(b256_key1, LeafUpdate::Changed(value.clone()));
        updates.insert(b256_key2, LeafUpdate::Changed(value.clone()));
        updates.insert(b256_key3, LeafUpdate::Changed(value));

        let proof_targets = RefCell::new(Vec::new());
        trie.update_leaves(&mut updates, |path, min_len| {
            proof_targets.borrow_mut().push((path, min_len));
        })
        .unwrap();

        // The callback should be invoked 3 times - once for each unique full_path
        // The deduplication is by (full_path, min_len), not by blinded node
        let targets = proof_targets.borrow();
        assert_eq!(targets.len(), 3, "Callback should be invoked for each unique key");

        // All should have the same min_len (1) since they all hit blinded node at path 0x0
        for (_, min_len) in targets.iter() {
            assert_eq!(*min_len, 1, "All should have min_len 1 from blinded node at 0x0");
        }
    }

    #[test]
    fn test_nibbles_to_padded_b256() {
        // Empty nibbles should produce all zeros
        let empty = Nibbles::default();
        assert_eq!(ParallelSparseTrie::nibbles_to_padded_b256(&empty), B256::ZERO);

        // Full 64-nibble path should round-trip through B256
        let full_key = b256!("0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef");
        let full_nibbles = Nibbles::unpack(full_key);
        assert_eq!(ParallelSparseTrie::nibbles_to_padded_b256(&full_nibbles), full_key);

        // Partial nibbles should be left-aligned with zero padding on the right
        // 4 nibbles [0x1, 0x2, 0x3, 0x4] should pack to 0x1234...00
        let partial = Nibbles::from_nibbles_unchecked([0x1, 0x2, 0x3, 0x4]);
        let expected = b256!("1234000000000000000000000000000000000000000000000000000000000000");
        assert_eq!(ParallelSparseTrie::nibbles_to_padded_b256(&partial), expected);

        // Single nibble
        let single = Nibbles::from_nibbles_unchecked([0xf]);
        let expected_single =
            b256!("f000000000000000000000000000000000000000000000000000000000000000");
        assert_eq!(ParallelSparseTrie::nibbles_to_padded_b256(&single), expected_single);
    }

    #[test]
    fn test_memory_size() {
        // Test that memory_size returns a reasonable value for an empty trie
        let trie = ParallelSparseTrie::default();
        let empty_size = trie.memory_size();

        // Should at least be the size of the struct itself
        assert!(empty_size >= core::mem::size_of::<ParallelSparseTrie>());

        // Create a trie with some data. Set up a root branch with children at 0x1 and
        // 0x5, and branches at [0x1] and [0x5] pointing to 0x2 and 0x6 respectively,
        // so the lower subtries at [0x1, 0x2] and [0x5, 0x6] are reachable.
        let root_branch = create_branch_node_with_children(
            &[0x1, 0x5],
            [
                RlpNode::word_rlp(&B256::repeat_byte(0xAA)),
                RlpNode::word_rlp(&B256::repeat_byte(0xBB)),
            ],
        );
        let mut trie = ParallelSparseTrie::from_root(root_branch, None, false).unwrap();

        let branch_at_1 =
            create_branch_node_with_children(&[0x2], [RlpNode::word_rlp(&B256::repeat_byte(0xCC))]);
        let branch_at_5 =
            create_branch_node_with_children(&[0x6], [RlpNode::word_rlp(&B256::repeat_byte(0xDD))]);
        trie.reveal_nodes(&mut [
            ProofTrieNodeV2 {
                path: Nibbles::from_nibbles_unchecked([0x1]),
                node: branch_at_1,
                masks: None,
            },
            ProofTrieNodeV2 {
                path: Nibbles::from_nibbles_unchecked([0x5]),
                node: branch_at_5,
                masks: None,
            },
        ])
        .unwrap();

        let mut nodes = vec![
            ProofTrieNodeV2 {
                path: Nibbles::from_nibbles_unchecked([0x1, 0x2]),
                node: TrieNodeV2::Leaf(LeafNode {
                    key: Nibbles::from_nibbles_unchecked([0x3, 0x4]),
                    value: vec![1, 2, 3],
                }),
                masks: None,
            },
            ProofTrieNodeV2 {
                path: Nibbles::from_nibbles_unchecked([0x5, 0x6]),
                node: TrieNodeV2::Leaf(LeafNode {
                    key: Nibbles::from_nibbles_unchecked([0x7, 0x8]),
                    value: vec![4, 5, 6],
                }),
                masks: None,
            },
        ];
        trie.reveal_nodes(&mut nodes).unwrap();

        let populated_size = trie.memory_size();

        // Populated trie should use more memory than an empty one
        assert!(populated_size > empty_size);
    }

    #[test]
    fn test_reveal_extension_branch_leaves_then_root() {
        // Test structure:
        // - 0x (root): extension node with key of 63 zeroes
        // - 0x000...000 (63 zeroes): branch node with children at 1 and 2
        // - 0x000...0001 (62 zeroes + 01): leaf with value 1
        // - 0x000...0002 (62 zeroes + 02): leaf with value 2
        //
        // The leaves and branch are small enough to be embedded (< 32 bytes),
        // so we manually RLP encode them and use those encodings in parent nodes.

        // Create the extension key (63 zero nibbles)
        let ext_key: [u8; 63] = [0; 63];

        // The branch is at the end of the extension (63 zeroes)
        let branch_path = Nibbles::from_nibbles(ext_key);

        // Leaf paths: 63 zeroes + 1, 63 zeroes + 2
        let mut leaf1_path_bytes = [0u8; 64];
        leaf1_path_bytes[63] = 1;
        let leaf1_path = Nibbles::from_nibbles(leaf1_path_bytes);

        let mut leaf2_path_bytes = [0u8; 64];
        leaf2_path_bytes[63] = 2;
        let leaf2_path = Nibbles::from_nibbles(leaf2_path_bytes);

        // Create leaves with empty keys (full path consumed by extension + branch)
        // and simple values
        let leaf1_node = LeafNode::new(Nibbles::default(), vec![0x1]);
        let leaf2_node = LeafNode::new(Nibbles::default(), vec![0x2]);

        // RLP encode the leaves to get their RlpNode representations
        let leaf1_rlp = RlpNode::from_rlp(&alloy_rlp::encode(TrieNodeV2::Leaf(leaf1_node.clone())));
        let leaf2_rlp = RlpNode::from_rlp(&alloy_rlp::encode(TrieNodeV2::Leaf(leaf2_node.clone())));

        // Create the branch node with children at indices 1 and 2, using the RLP-encoded leaves.
        // In V2, branch and extension are combined: the key holds the extension prefix.
        let state_mask = TrieMask::new(0b0000_0110); // bits 1 and 2 set
        let stack = vec![leaf1_rlp, leaf2_rlp];

        // First encode the bare branch (empty key) to get its RlpNode
        let bare_branch = BranchNodeV2::new(Nibbles::new(), stack.clone(), state_mask, None);
        let branch_rlp = RlpNode::from_rlp(&alloy_rlp::encode(&bare_branch));

        // Create the combined extension+branch node as the root.
        let root_node = TrieNodeV2::Branch(BranchNodeV2::new(
            Nibbles::from_nibbles(ext_key),
            stack.clone(),
            state_mask,
            Some(branch_rlp),
        ));

        // Initialize trie with the extension+branch as root
        let mut trie = ParallelSparseTrie::from_root(root_node, None, false).unwrap();

        // Reveal the branch and leaves
        let mut nodes = vec![
            ProofTrieNodeV2 {
                path: branch_path,
                node: TrieNodeV2::Branch(BranchNodeV2::new(
                    Nibbles::new(),
                    stack,
                    state_mask,
                    None,
                )),
                masks: None,
            },
            ProofTrieNodeV2 { path: leaf1_path, node: TrieNodeV2::Leaf(leaf1_node), masks: None },
            ProofTrieNodeV2 { path: leaf2_path, node: TrieNodeV2::Leaf(leaf2_node), masks: None },
        ];
        trie.reveal_nodes(&mut nodes).unwrap();

        // Add the leaf paths to prefix_set so that root() will update their hashes
        trie.prefix_set.insert(leaf1_path);
        trie.prefix_set.insert(leaf2_path);

        // Call root() to compute the trie root hash
        let _root = trie.root();
    }

    #[test]
    fn test_update_leaf_creates_embedded_nodes_then_root() {
        // Similar structure to test_reveal_extension_branch_leaves_then_root, but created
        // via update_leaf calls on an empty trie instead of revealing pre-built nodes.
        //
        // Two leaves with paths that share a long common prefix will create:
        // - Extension node at root with the shared prefix
        // - Branch node where the paths diverge
        // - Two leaf nodes (embedded in the branch since they're small)

        // Create two paths that share 63 nibbles and differ only at the 64th
        let mut leaf1_path_bytes = [0u8; 64];
        leaf1_path_bytes[63] = 1;
        let leaf1_path = Nibbles::from_nibbles(leaf1_path_bytes);

        let mut leaf2_path_bytes = [0u8; 64];
        leaf2_path_bytes[63] = 2;
        let leaf2_path = Nibbles::from_nibbles(leaf2_path_bytes);

        // Create an empty trie and update with two leaves
        let mut trie = ParallelSparseTrie::default();
        trie.update_leaf(leaf1_path, vec![0x1], DefaultTrieNodeProvider).unwrap();
        trie.update_leaf(leaf2_path, vec![0x2], DefaultTrieNodeProvider).unwrap();

        // Call root() to compute the trie root hash
        let _root = trie.root();
    }
}
