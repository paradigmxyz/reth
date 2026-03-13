use super::{ArenaSparseNode, ArenaSparseNodeState, Index, NodeArena};
use alloc::vec::Vec;
use reth_trie_common::Nibbles;
use tracing::{instrument, trace};

const TRACE_TARGET: &str = "trie::arena::cursor";

/// An entry on the cursor's traversal stack, tracking an ancestor node during trie walks.
#[derive(Debug, Clone)]
pub(super) struct ArenaCursorStackEntry {
    /// The arena index of this node.
    pub(super) index: Index,
    /// The absolute path of this node in the trie (not including its `short_key`).
    pub(super) path: Nibbles,
    /// The nibble value at which to resume child iteration in [`ArenaCursor::next`].
    /// Children with nibble values strictly less than this are skipped.
    /// Only meaningful when this entry's node is a branch.
    pub(super) resume_nibble: u8,
}

/// Result of [`ArenaCursor::seek`] describing the state at the deepest ancestor node.
#[derive(Debug)]
pub(super) enum SeekResult {
    /// The stack head is an empty root node.
    EmptyRoot,
    /// The stack head is a leaf whose full path matches the target exactly.
    RevealedLeaf,
    /// The next child along the path is blinded (unrevealed).
    Blinded,
    /// The target path diverges from the stack head's `short_key` (branch or leaf).
    Diverged,
    /// The target nibble has no child in the branch's `state_mask`.
    NoChild { child_nibble: u8 },
    /// The target nibble has a revealed subtrie child (now pushed onto the stack).
    RevealedSubtrie,
}

/// Result of [`ArenaCursor::next`] describing what the cursor did.
#[derive(Debug)]
pub(super) enum NextResult {
    /// The head is a non-branch node (subtrie, taken-subtrie, leaf, etc.).
    /// The caller should process it; the next call to [`ArenaCursor::next`] will pop it.
    NonBranch,
    /// The head branch has no more qualifying children. It is still on the stack;
    /// the caller should process it. The next call to [`ArenaCursor::next`] will pop it.
    Branch,
    /// The stack is empty — the traversal is complete.
    Done,
}

/// A cursor for depth-first traversal of an arena-based sparse trie.
///
/// Wraps a stack of [`ArenaCursorStackEntry`]s and provides methods for navigating
/// the trie: pushing children, popping with dirty-state propagation, seeking
/// to ancestors, and computing child paths.
///
/// The cursor borrows the arena on each method call rather than holding a
/// reference, so the caller retains full ownership of the arena between calls.
#[derive(Debug, Default, Clone)]
pub(super) struct ArenaCursor {
    stack: Vec<ArenaCursorStackEntry>,
    /// Whether the head entry should be popped at the start of the next [`Self::next`] call.
    /// Set when `next` returns [`NextResult::NonBranch`] or [`NextResult::Branch`].
    needs_pop: bool,
}

impl ArenaCursor {
    /// Returns the entry at the top of the stack, or `None` if empty.
    pub(super) fn head(&self) -> Option<&ArenaCursorStackEntry> {
        self.stack.last()
    }

    /// Returns the entry below the top of the stack (the parent of the head), or `None`.
    pub(super) fn parent(&self) -> Option<&ArenaCursorStackEntry> {
        let len = self.stack.len();
        (len >= 2).then(|| &self.stack[len - 2])
    }

    /// Returns the depth of the head node (0 for the root).
    ///
    /// # Panics
    ///
    /// Panics if the stack is empty.
    pub(super) const fn depth(&self) -> usize {
        self.stack.len() - 1
    }

    /// Replaces the root entry on the stack with a new one.
    ///
    /// The stack must contain exactly the root (depth 0) or be empty (freshly constructed).
    #[instrument(level = "trace", target = TRACE_TARGET, skip(self, arena))]
    pub(super) fn reset(&mut self, arena: &NodeArena, idx: Index, path: Nibbles) {
        debug_assert!(
            self.stack.len() <= 1 && !self.needs_pop,
            "cursor must be drained before reset; stack has {} entries, needs_pop={}",
            self.stack.len(),
            self.needs_pop,
        );
        self.stack.clear();
        self.needs_pop = false;
        self.push(arena, idx, path);
    }

    /// Pushes an entry onto the stack for the node at the given index and path.
    fn push(&mut self, arena: &NodeArena, idx: Index, path: Nibbles) {
        debug_assert!(arena.contains_key(idx), "push called with invalid arena index");
        self.stack.push(ArenaCursorStackEntry { index: idx, path, resume_nibble: 0 });
        trace!(target: TRACE_TARGET, entry = ?self.stack.last().expect("just pushed"), "Pushed stack entry");
    }

    /// Pops the top entry from the stack and propagates dirty state to the parent.
    /// Returns the popped entry.
    ///
    /// Uses `arena.get()` for the popped node because callers (e.g. pruning) may remove
    /// the node from the arena between the time it was pushed and the time it is popped.
    #[instrument(level = "trace", target = TRACE_TARGET, skip(self, arena))]
    pub(super) fn pop(&mut self, arena: &mut NodeArena) -> ArenaCursorStackEntry {
        let entry = self.stack.pop().expect("pop can't be called on empty stack");
        trace!(target: TRACE_TARGET, entry = ?entry, "Popped stack entry");

        #[cfg(debug_assertions)]
        if let Some(ArenaSparseNode::Subtrie(s)) = arena.get(entry.index) {
            debug_assert_eq!(
                s.path, entry.path,
                "subtrie cached path {:?} does not match stack entry path {:?}",
                s.path, entry.path,
            );
        }

        if let Some(parent) = self.stack.last() {
            let child_is_dirty = arena.get(entry.index).is_some_and(|node| match node {
                ArenaSparseNode::Branch(_) | ArenaSparseNode::Leaf { .. } => {
                    arena.get_cold(entry.index).is_some_and(|c| c.state().is_dirty())
                }
                ArenaSparseNode::Subtrie(s) => {
                    let root = &s.arena[s.root];
                    matches!(root, ArenaSparseNode::EmptyRoot) ||
                        s.arena.state_ref(s.root).is_some_and(|st| st.is_dirty())
                }
                _ => false,
            });
            if child_is_dirty {
                *arena.cold_mut(parent.index).state_mut() = ArenaSparseNodeState::Dirty;
            }
        }

        entry
    }

    /// Drains the stack down to the root, propagating dirty state from each popped entry
    /// to its parent. The root entry remains on the stack (there is no parent to propagate to).
    #[instrument(level = "trace", target = TRACE_TARGET, skip_all)]
    pub(super) fn drain(&mut self, arena: &mut NodeArena) {
        trace!(target: TRACE_TARGET, "Draining stack");
        self.needs_pop = false;
        while self.stack.len() > 1 {
            self.pop(arena);
        }
    }

    /// Returns the logical path of the branch at the top of the stack.
    /// The logical path is `entry.path + branch.short_key`.
    pub(super) fn head_logical_branch_path(&self, arena: &NodeArena) -> Nibbles {
        logical_branch_path(arena, self.stack.last().expect("cursor is non-empty"))
    }

    /// Returns the length of the logical path of the branch at the top of the stack.
    /// Equivalent to `head_logical_branch_path(arena).len()` but avoids constructing the path.
    pub(super) fn head_logical_branch_path_len(&self, arena: &NodeArena) -> usize {
        logical_branch_path_len(arena, self.stack.last().expect("cursor is non-empty"))
    }

    /// Returns the absolute path of a child at `child_nibble` under the branch at the top of
    /// the stack. The result is `stack_head.path + branch.short_key + child_nibble`.
    pub(super) fn child_path(&self, arena: &NodeArena, child_nibble: u8) -> Nibbles {
        let mut path = logical_branch_path(arena, self.stack.last().expect("cursor is non-empty"));
        path.push_unchecked(child_nibble);
        path
    }

    /// Returns the logical path of the parent branch entry (second from top of the stack).
    /// Panics if the stack has fewer than 2 entries.
    pub(super) fn parent_logical_branch_path(&self, arena: &NodeArena) -> Nibbles {
        logical_branch_path(arena, self.parent().expect("cursor must have a parent"))
    }

    /// Replaces the arena index stored in the head entry with `new_idx`, and updates the
    /// parent branch's children array to point to the new index. If the head is the root
    /// (stack has one entry), `root` is updated instead.
    pub(super) fn replace_head_index(
        &mut self,
        arena: &mut NodeArena,
        root: &mut Index,
        new_idx: Index,
    ) {
        let head = self.stack.last_mut().expect("cursor must have head");
        let old_idx = head.index;
        let child_nibble = head.path.last();
        head.index = new_idx;

        let Some(parent) = self.parent() else {
            *root = new_idx;
            return;
        };

        let child_nibble =
            child_nibble.expect("if cursor has a parent then the head path can't be empty");

        let parent_branch = arena[parent.index].branch_mut();

        debug_assert!(
            parent_branch.revealed_mask.is_bit_set(child_nibble) &&
                parent_branch.children[child_nibble as usize] == old_idx,
            "parent child at nibble {child_nibble} does not match old_idx",
        );

        parent_branch.children[child_nibble as usize] = new_idx;
    }

    /// Advances the DFS traversal to the next actionable node.
    ///
    /// If a previous call returned [`NextResult::NonBranch`] or [`NextResult::Branch`],
    /// the head entry is automatically popped (with dirty-state propagation) before
    /// descending further. This means callers never need to call [`Self::pop`] after
    /// `next` — it is handled internally on the subsequent call.
    ///
    /// Returns [`NextResult::NonBranch`] when the head is a non-branch node the caller
    /// should process, or [`NextResult::Branch`] when a branch has exhausted its
    /// qualifying children. In both cases the node is still on the stack so the caller
    /// can read it via [`Self::head`].
    ///
    /// Returns [`NextResult::Done`] when the stack is empty (traversal complete).
    #[instrument(level = "trace", target = TRACE_TARGET, skip_all, ret)]
    pub(super) fn next(
        &mut self,
        arena: &mut NodeArena,
        should_descend: impl Fn(&NodeArena, Index) -> bool,
    ) -> NextResult {
        if self.needs_pop {
            self.pop(arena);
            self.needs_pop = false;
        }

        loop {
            let Some(head) = self.stack.last_mut() else {
                return NextResult::Done;
            };
            let head_idx = head.index;

            let ArenaSparseNode::Branch(branch) = &arena[head_idx] else {
                self.needs_pop = true;
                return NextResult::NonBranch;
            };

            let resume_nibble = head.resume_nibble;

            let mut descended = false;
            // Iterate over revealed children starting from resume_nibble.
            for nibble in branch.revealed_mask.iter() {
                if nibble < resume_nibble {
                    continue;
                }

                let child_idx = branch.children[nibble as usize];

                if should_descend(arena, child_idx) {
                    // Record where to resume iteration when we return to this entry.
                    self.stack.last_mut().expect("head exists").resume_nibble = nibble + 1;
                    let path = self.child_path(arena, nibble);
                    self.push(arena, child_idx, path);
                    descended = true;
                    break;
                }
            }

            if !descended {
                self.needs_pop = true;
                return NextResult::Branch;
            }
        }
    }

    /// Pops the stack until the head is an ancestor of `full_path`, then descends from that head
    /// toward `full_path`, pushing revealed branch (and leaf) children onto the stack until the
    /// deepest ancestor is reached.
    ///
    /// Returns a [`SeekResult`] describing the state at the stack head.
    #[instrument(level = "trace", target = TRACE_TARGET, skip(self, arena), ret)]
    pub(super) fn seek(&mut self, arena: &mut NodeArena, full_path: &Nibbles) -> SeekResult {
        // Pop stack until head is ancestor of full_path.
        while self.stack.len() > 1 &&
            !full_path.starts_with(&self.stack.last().expect("cursor has root").path)
        {
            self.pop(arena);
        }

        loop {
            let head = self.stack.last().expect("cursor has root");
            let head_idx = head.index;

            match &arena[head_idx] {
                ArenaSparseNode::EmptyRoot => {
                    return SeekResult::EmptyRoot;
                }
                ArenaSparseNode::Leaf { key, .. } => {
                    let mut leaf_full_path = head.path;
                    leaf_full_path.extend(key);
                    return if &leaf_full_path == full_path {
                        SeekResult::RevealedLeaf
                    } else {
                        SeekResult::Diverged
                    };
                }
                ArenaSparseNode::Subtrie(_) => {
                    return SeekResult::RevealedSubtrie;
                }
                ArenaSparseNode::TakenSubtrie => {
                    unreachable!("unexpected TakenSubtrie on stack");
                }
                ArenaSparseNode::Branch(branch) => {
                    let head_path = head.path;

                    // Compute logical path once for this branch.
                    let short_key = &branch.short_key;
                    let logical_path_len = head_path.len() + short_key.len();

                    // If full_path doesn't extend past the branch's logical path, the target
                    // is at or within the branch's short_key — treat as diverged.
                    if full_path.len() <= logical_path_len {
                        return SeekResult::Diverged;
                    }

                    // Check that full_path matches the short_key prefix.
                    if !short_key.is_empty() {
                        let path_slice = full_path.slice(head_path.len()..logical_path_len);
                        if path_slice != *short_key {
                            return SeekResult::Diverged;
                        }
                    }

                    let child_nibble = full_path.get_unchecked(logical_path_len);

                    if !branch.state_mask.is_bit_set(child_nibble) {
                        return SeekResult::NoChild { child_nibble };
                    }

                    if branch.is_child_blinded(child_nibble) {
                        return SeekResult::Blinded;
                    }

                    // Child is revealed — push and continue descent.
                    let child_idx = branch.children[child_nibble as usize];
                    let mut child_path = head_path;
                    child_path.extend(short_key);
                    child_path.push_unchecked(child_nibble);
                    self.push(arena, child_idx, child_path);
                }
            }
        }
    }
}

/// Returns the logical path of a branch stack entry. The logical path is
/// `entry.path + branch.short_key`.
fn logical_branch_path(arena: &NodeArena, entry: &ArenaCursorStackEntry) -> Nibbles {
    let mut path = entry.path;
    path.extend(&arena[entry.index].branch_ref().short_key);
    path
}

/// Returns the length of the logical path of a branch stack entry.
/// Equivalent to `logical_branch_path(arena, entry).len()` but avoids constructing the path.
fn logical_branch_path_len(arena: &NodeArena, entry: &ArenaCursorStackEntry) -> usize {
    entry.path.len() + arena[entry.index].branch_ref().short_key.len()
}
