//! Pruning, mutation, revelation, and hashing within an independently allocated subtrie.

use super::*;

impl ArenaSparseSubtrie {
    /// Creates a new subtrie with a pre-allocated root slot containing
    /// [`ArenaSparseNode::EmptyRoot`]. The caller must overwrite `subtrie.arena[subtrie.root]`
    /// before use.
    pub(super) fn new(record_updates: bool) -> Box<Self> {
        let mut arena = NodeArena::new();
        let root =
            arena.insert(ArenaSparseNode::EmptyRoot { state: ArenaSparseNodeState::Revealed });
        let buffers = ArenaTrieBuffers {
            updates: record_updates.then(SparseTrieUpdates::default),
            ..Default::default()
        };
        Box::new(Self {
            arena,
            root,
            path: Nibbles::default(),
            buffers,
            required_proofs: Vec::new(),
            num_leaves: 0,
            num_dirty_leaves: 0,
        })
    }

    /// Asserts that `num_leaves` and `num_dirty_leaves` match the actual counts in the arena.
    #[cfg(debug_assertions)]
    pub(super) fn debug_assert_counters(&self) {
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

    /// Collapses nodes last modified before `prune_before` into hash stubs while copying retained
    /// nodes into a compacted arena.
    ///
    /// Expects that all nodes have computed hashes (i.e. `prune` is called after hashing).
    pub(super) fn prune(&mut self, prune_before: TrieNodeEpoch) -> usize {
        // Only branches can have pruneable children.
        if !matches!(&self.arena[self.root], ArenaSparseNode::Branch(_)) {
            return 0;
        }

        debug_assert_eq!(self.num_dirty_leaves, 0, "prune must run after hashing");

        if prune_before == TrieNodeEpoch::UNMODIFIED {
            return 0;
        }

        let old_count = self.arena.len();
        // Reserve an upper bound on the retained nodes and hand the excess back after the copy,
        // so that copying never reallocates but discarded nodes still release their capacity.
        let mut new_arena = NodeArena::with_capacity(old_count);
        let mut marks = new_arena.adopt_blinded(&mut self.arena);
        let mut new_num_leaves = 0u64;

        // The subtrie root is retained by the owning upper trie.
        let root_node = self.arena.drain_node(self.root);
        let new_root = new_arena.insert(root_node);
        let mut stack = Vec::new();
        if let Some(frame) =
            prepare_retained_node(&new_arena, new_root, self.path, &mut new_num_leaves)
        {
            stack.push(frame);
        }

        while let Some(frame) = stack.last_mut() {
            let Some((child_pos, nibble, child)) = frame.next_child(&new_arena) else {
                stack.pop();
                continue;
            };

            let Some(old_child_idx) = child.revealed_index() else {
                // A blinded child needs no rewrite, its slot survived the adoption.
                marks.mark(child);
                continue;
            };

            let parent_new_idx = frame.new_idx;
            let mut child_path = frame.branch_logical_path;
            child_path.push(nibble);

            let child_epoch = self.arena[old_child_idx]
                .state_ref()
                .and_then(ArenaSparseNodeState::cached_epoch)
                .expect("prune must run after hashing");

            if child_epoch.should_prune(prune_before) {
                let node = &self.arena[old_child_idx];
                let rlp_node = node
                    .state_ref()
                    .and_then(ArenaSparseNodeState::cached_rlp_node)
                    .cloned()
                    .expect("prune must run after hashing");
                trace!(
                    target: TRACE_TARGET,
                    path = ?child_path,
                    variant = %AsRef::<str>::as_ref(node),
                    cached_rlp_node = ?rlp_node,
                    "pruning node",
                );
                let new_child = new_arena.insert_blinded(rlp_node);
                marks.mark(new_child);
                let ArenaSparseNode::Branch(b) = &mut new_arena[parent_new_idx] else {
                    unreachable!()
                };
                b.children[child_pos] = new_child;
            } else {
                let child_node = self.arena.drain_node(old_child_idx);
                let new_child_idx = new_arena.insert(child_node);
                if let Some(frame) = prepare_retained_node(
                    &new_arena,
                    new_child_idx,
                    child_path,
                    &mut new_num_leaves,
                ) {
                    stack.push(frame);
                }
                let ArenaSparseNode::Branch(b) = &mut new_arena[parent_new_idx] else {
                    unreachable!()
                };
                b.children[child_pos] = BranchChild::revealed(new_child_idx);
            }
        }

        new_arena.sweep_blinded(marks);
        new_arena.shrink_nodes_to_fit();
        let pruned = old_count - new_arena.len();
        self.num_leaves = new_num_leaves;
        self.num_dirty_leaves = 0;
        self.arena = new_arena;
        self.root = new_root;

        #[cfg(debug_assertions)]
        self.debug_assert_counters();
        return pruned;

        struct CopyFrame {
            new_idx: Index,
            branch_logical_path: Nibbles,
            state_mask: TrieMask,
            remaining_child_mask: TrieMask,
        }

        impl CopyFrame {
            fn next_child(&mut self, new_arena: &NodeArena) -> Option<(usize, u8, BranchChild)> {
                let ArenaSparseNode::Branch(b) = &new_arena[self.new_idx] else { unreachable!() };

                let nibble = self.remaining_child_mask.first_set_bit_index()?;
                self.remaining_child_mask.unset_bit(nibble);
                let child_idx = BranchChildIdx::new(self.state_mask, nibble)
                    .expect("remaining_child_mask must be a subset of state_mask");

                Some((child_idx.get(), nibble, b.children[child_idx]))
            }
        }

        /// Prepares a retained node for copying, returning a stack frame when the node has children
        /// to walk.
        fn prepare_retained_node(
            new_arena: &NodeArena,
            new_idx: Index,
            node_path: Nibbles,
            new_num_leaves: &mut u64,
        ) -> Option<CopyFrame> {
            let ArenaSparseNode::Branch(b) = &new_arena[new_idx] else {
                if matches!(&new_arena[new_idx], ArenaSparseNode::Leaf { .. }) {
                    *new_num_leaves += 1;
                }
                return None;
            };

            let mut branch_logical_path = node_path;
            branch_logical_path.extend(&b.short_key);

            Some(CopyFrame {
                new_idx,
                branch_logical_path,
                state_mask: b.state_mask,
                remaining_child_mask: b.state_mask,
            })
        }
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
    pub(super) fn update_leaves(&mut self, sorted_updates: &[(B256, Nibbles, LeafUpdate)]) {
        if sorted_updates.is_empty() {
            return;
        }
        trace!(target: TRACE_TARGET, "Subtrie update_leaves");

        debug_assert!(
            !matches!(self.arena[self.root], ArenaSparseNode::EmptyRoot { .. }),
            "subtrie root must not be EmptyRoot at start of update_leaves"
        );

        self.buffers.cursor.reset(&self.arena, self.root, self.path);

        for (idx, &(key, ref full_path, ref update)) in sorted_updates.iter().enumerate() {
            let find_result = self.buffers.cursor.seek(&mut self.arena, full_path);

            // If the path hits a blinded node, request a proof regardless of update type.
            if matches!(find_result, SeekResult::Blinded) {
                let logical_len = self.buffers.cursor.head_logical_branch_path_len(&self.arena);
                self.required_proofs.push((
                    idx,
                    ArenaRequiredProof { key, parent: ProofV2TargetParent::new(logical_len) },
                ));
                continue;
            }

            match update {
                LeafUpdate::Changed(value) if !value.is_empty() => {
                    // Upsert: insert or update a leaf with the given value.
                    let (_result, deltas) = ArenaParallelSparseTrie::upsert_leaf(
                        &mut self.arena,
                        &mut self.buffers.cursor,
                        &mut self.root,
                        full_path,
                        value,
                        find_result,
                    );
                    self.num_leaves = (self.num_leaves as i64 + deltas.num_leaves_delta) as u64;
                    self.num_dirty_leaves =
                        (self.num_dirty_leaves as i64 + deltas.num_dirty_leaves_delta) as u64;
                }
                LeafUpdate::Changed(_) => {
                    let (result, deltas) = ArenaParallelSparseTrie::remove_leaf(
                        &mut self.arena,
                        &mut self.buffers.cursor,
                        &mut self.root,
                        key,
                        full_path,
                        find_result,
                        &mut self.buffers.updates,
                    );
                    self.num_leaves = (self.num_leaves as i64 + deltas.num_leaves_delta) as u64;
                    self.num_dirty_leaves =
                        (self.num_dirty_leaves as i64 + deltas.num_dirty_leaves_delta) as u64;

                    if let RemoveLeafResult::NeedsProof { key, proof_key, parent } = result {
                        self.required_proofs
                            .push((idx, ArenaRequiredProof { key: proof_key, parent }));
                        self.required_proofs.push((idx, ArenaRequiredProof { key, parent }));
                    }
                }
                LeafUpdate::Touched => {}
            }
        }

        // Drain remaining cursor entries, propagating dirty state.
        self.buffers.cursor.drain(&mut self.arena);

        #[cfg(debug_assertions)]
        self.debug_assert_counters();
    }

    /// Reveals nodes inside this subtrie. Uses [`ArenaCursor::seek`] to locate the ancestor
    /// node, then replaces blinded children with the proof nodes.
    pub(super) fn reveal_nodes(&mut self, nodes: &mut [ProofTrieNodeV2]) -> SparseTrieResult<()> {
        if nodes.is_empty() {
            return Ok(());
        }
        trace!(target: TRACE_TARGET, path = ?self.path, num_nodes = nodes.len(), "Subtrie reveal_nodes");

        debug_assert!(
            !matches!(self.arena[self.root], ArenaSparseNode::EmptyRoot { .. }),
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
    pub(super) fn update_cached_rlp(&mut self, new_epoch: TrieNodeEpoch) {
        ArenaParallelSparseTrie::update_cached_rlp(
            &mut self.arena,
            self.root,
            self.path,
            &mut self.buffers,
            new_epoch,
        );
        self.num_dirty_leaves = 0;
        #[cfg(debug_assertions)]
        self.debug_assert_counters();
    }
}
