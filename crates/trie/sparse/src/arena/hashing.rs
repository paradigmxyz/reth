//! Cached RLP calculation for revealed arena nodes.

use super::*;

impl ArenaParallelSparseTrie {
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
    pub(super) fn update_cached_rlp(
        arena: &mut NodeArena,
        root: Index,
        base_path: Nibbles,
        buffers: &mut ArenaTrieBuffers,
        new_epoch: TrieNodeEpoch,
    ) -> RlpNode {
        let cursor = &mut buffers.cursor;
        let rlp_buf = &mut buffers.rlp_buf;
        let rlp_node_buf = &mut buffers.rlp_node_buf;
        let updates = &mut buffers.updates;

        rlp_node_buf.clear();

        // Step 1: Handle trivial roots that don't need the stack-based walk.
        // Empty roots and leaves are encoded in place. Already-cached branches need no work.
        // Only dirty branches enter the main loop below.
        match &arena[root] {
            ArenaSparseNode::EmptyRoot { state } => {
                let node_epoch = match state {
                    ArenaSparseNodeState::Cached { epoch, .. } => *epoch,
                    ArenaSparseNodeState::Revealed => TrieNodeEpoch::UNMODIFIED,
                    ArenaSparseNodeState::Dirty => new_epoch,
                };
                let rlp_node = RlpNode::word_rlp(&EMPTY_ROOT_HASH);
                *arena[root].state_mut() =
                    ArenaSparseNodeState::Cached { rlp_node: rlp_node.clone(), epoch: node_epoch };
                return rlp_node
            }
            ArenaSparseNode::Leaf { .. } => {
                Self::encode_leaf(arena, root, rlp_buf, rlp_node_buf, new_epoch);
                return rlp_node_buf.pop().expect("encode_leaf must push an RlpNode");
            }
            ArenaSparseNode::Branch(b) => {
                if let ArenaSparseNodeState::Cached { rlp_node, .. } = &b.state {
                    let rlp_node = rlp_node.clone();
                    return rlp_node;
                }
            }
            ArenaSparseNode::Subtrie(_) | ArenaSparseNode::TakenSubtrie | ArenaSparseNode::Free => {
                unreachable!("Subtrie/TakenSubtrie/Free cannot be a subtrie's root");
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

            let head_idx = cursor.head().expect("cursor is non-empty").index;
            let head_path = cursor.head_path();

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
            let mut node_epoch = TrieNodeEpoch::UNMODIFIED;
            let state_mask = arena[head_idx].branch_ref().state_mask;
            for (dense_idx, _nibble) in BranchChildIter::new(state_mask) {
                let child = arena[head_idx].branch_ref().children[dense_idx];
                match child.revealed_index() {
                    None => {
                        rlp_node_buf.push(arena.blinded(child).clone());
                    }
                    Some(child_idx) => {
                        match &arena[child_idx] {
                            ArenaSparseNode::Leaf { .. } => {
                                Self::encode_leaf(
                                    arena,
                                    child_idx,
                                    rlp_buf,
                                    rlp_node_buf,
                                    new_epoch,
                                );
                            }
                            ArenaSparseNode::Branch(child_b) => {
                                let ArenaSparseNodeState::Cached { rlp_node, .. } = &child_b.state
                                else {
                                    panic!("child branch must be cached after DFS");
                                };
                                let rlp_node = rlp_node.clone();
                                rlp_node_buf.push(rlp_node);
                            }
                            ArenaSparseNode::Subtrie(subtrie) => {
                                let subtrie_root = &subtrie.arena[subtrie.root];
                                match subtrie_root {
                                    ArenaSparseNode::Branch(ArenaSparseNodeBranch {
                                        state: ArenaSparseNodeState::Cached { rlp_node, .. },
                                        ..
                                    }) |
                                    ArenaSparseNode::Leaf {
                                        state: ArenaSparseNodeState::Cached { rlp_node, .. },
                                        ..
                                    } => {
                                        rlp_node_buf.push(rlp_node.clone());
                                    }
                                    _ => panic!("subtrie root must be a cached Branch or Leaf"),
                                }
                            }
                            ArenaSparseNode::TakenSubtrie |
                            ArenaSparseNode::EmptyRoot { .. } |
                            ArenaSparseNode::Free => {
                                unreachable!("Unexpected child {:?}", arena[child_idx]);
                            }
                        }
                        let Some(ArenaSparseNodeState::Cached { epoch: child_epoch, .. }) =
                            arena[child_idx].state_ref()
                        else {
                            panic!("revealed child must be cached after encoding");
                        };
                        node_epoch = node_epoch.max(*child_epoch);
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
            if was_dirty {
                node_epoch = node_epoch.max(new_epoch);
            }

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
            branch.state = ArenaSparseNodeState::Cached { rlp_node, epoch: node_epoch };
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

    /// Encodes a leaf node's RLP and pushes it onto `rlp_node_buf`.
    ///
    /// If the leaf is already cached, its existing `RlpNode` is reused.
    pub(super) fn encode_leaf(
        arena: &mut NodeArena,
        idx: Index,
        rlp_buf: &mut Vec<u8>,
        rlp_node_buf: &mut Vec<RlpNode>,
        new_epoch: TrieNodeEpoch,
    ) {
        let (key, value, state) = match &arena[idx] {
            ArenaSparseNode::Leaf { key, value, state } => (key, value, state),
            _ => unreachable!("encode_leaf called on non-Leaf node"),
        };

        let epoch = match state {
            ArenaSparseNodeState::Cached { rlp_node, .. } => {
                rlp_node_buf.push(rlp_node.clone());
                return;
            }
            ArenaSparseNodeState::Revealed => TrieNodeEpoch::UNMODIFIED,
            ArenaSparseNodeState::Dirty => new_epoch,
        };

        rlp_buf.clear();
        let rlp_node = LeafNodeRef { key, value }.rlp(rlp_buf);

        *arena[idx].state_mut() =
            ArenaSparseNodeState::Cached { rlp_node: rlp_node.clone(), epoch };
        rlp_node_buf.push(rlp_node);
    }
}
