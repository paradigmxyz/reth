use super::{
    child_dense_index, ArenaSparseNode, ArenaSparseNodeBranchChild, ArenaSparseNodeState,
    TRACE_TARGET,
};
use alloc::vec::Vec;
use reth_trie_common::Nibbles;
use thunderdome::{Arena, Index};
use tracing::{instrument, trace};

/// An entry on the cursor's traversal stack, tracking an ancestor node during trie walks.
#[derive(Debug, Clone)]
pub(super) struct ArenaCursorStackEntry {
    /// The arena index of this node.
    pub(super) index: Index,
    /// The absolute path of this node in the trie (not including its `short_key`).
    pub(super) path: Nibbles,
}

/// Result of [`ArenaCursor::seek`] describing the state at the deepest ancestor node.
#[derive(Debug)]
pub(super) enum SeekResult {
    /// The stack head is an empty root node.
    EmptyRoot,
    /// The stack head is a leaf node. The leaf has been pushed onto the stack.
    RevealedLeaf,
    /// The next child along the path is blinded (unrevealed).
    Blinded,
    /// The target path diverges within the stack head branch's `short_key`.
    Diverged,
    /// The target nibble has no child in the branch's `state_mask`.
    NoChild { child_nibble: u8 },
    /// The target nibble has a revealed subtrie child (now pushed onto the stack).
    RevealedSubtrie,
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

    /// Returns the number of entries on the stack.
    pub(super) fn len(&self) -> usize {
        self.stack.len()
    }

    /// Returns `true` if the stack is empty.
    pub(super) fn is_empty(&self) -> bool {
        self.stack.is_empty()
    }

    /// Clears the traversal stack.
    pub(super) fn clear(&mut self) {
        self.stack.clear();
    }

    /// Pushes an entry onto the stack for the node at the given index and path.
    pub(super) fn push(&mut self, arena: &Arena<ArenaSparseNode>, idx: Index, path: Nibbles) {
        let _ = &arena[idx];
        self.stack.push(ArenaCursorStackEntry { index: idx, path });
        trace!(target: TRACE_TARGET, entry = ?self.stack.last().expect("just pushed"), "Pushed stack entry");
    }

    /// Pops the top entry from the stack and propagates dirty state to the parent.
    /// Returns the popped entry's path.
    #[instrument(level = "trace", target = "trie::arena", skip(self, arena))]
    pub(super) fn pop(&mut self, arena: &mut Arena<ArenaSparseNode>) -> Nibbles {
        let entry = self.stack.pop().expect("pop can't be called on empty stack");
        trace!(target: TRACE_TARGET, entry = ?entry, "Popped stack entry");

        #[cfg(debug_assertions)]
        if let ArenaSparseNode::Subtrie(s) = &arena[entry.index] {
            debug_assert_eq!(
                s.path, entry.path,
                "subtrie cached path {:?} does not match stack entry path {:?}",
                s.path, entry.path,
            );
        }

        if let Some(parent) = self.stack.last() {
            let child_is_dirty = match &arena[entry.index] {
                ArenaSparseNode::Branch(b) => matches!(b.state, ArenaSparseNodeState::Dirty),
                ArenaSparseNode::Leaf { state, .. } => matches!(state, ArenaSparseNodeState::Dirty),
                ArenaSparseNode::Subtrie(s) => {
                    let root = &s.arena[s.root];
                    matches!(root, ArenaSparseNode::EmptyRoot) ||
                        matches!(root.state_ref(), Some(ArenaSparseNodeState::Dirty))
                }
                _ => false,
            };
            if child_is_dirty {
                let parent_state = arena[parent.index].state_mut();
                if !matches!(parent_state, ArenaSparseNodeState::Dirty) {
                    *parent_state = ArenaSparseNodeState::Dirty;
                }
            }
        }

        entry.path
    }

    /// Drains the stack, propagating dirty state from each entry to its parent.
    pub(super) fn drain(&mut self, arena: &mut Arena<ArenaSparseNode>) {
        trace!(target: TRACE_TARGET, "Draining stack");
        while self.stack.len() > 1 {
            self.pop(arena);
        }
    }

    /// Returns the logical path of the branch at the top of the stack.
    /// The logical path is `entry.path + branch.short_key`.
    pub(super) fn head_logical_branch_path(&self, arena: &Arena<ArenaSparseNode>) -> Nibbles {
        logical_branch_path(arena, self.stack.last().expect("cursor is non-empty"))
    }

    /// Returns the length of the logical path of the branch at the top of the stack.
    /// Equivalent to `head_logical_branch_path(arena).len()` but avoids constructing the path.
    pub(super) fn head_logical_branch_path_len(&self, arena: &Arena<ArenaSparseNode>) -> usize {
        logical_branch_path_len(arena, self.stack.last().expect("cursor is non-empty"))
    }

    /// Returns the absolute path of a child at `child_nibble` under the branch at the top of
    /// the stack. The result is `stack_head.path + branch.short_key + child_nibble`.
    pub(super) fn child_path(&self, arena: &Arena<ArenaSparseNode>, child_nibble: u8) -> Nibbles {
        let mut path = logical_branch_path(arena, self.stack.last().expect("cursor is non-empty"));
        path.push_unchecked(child_nibble);
        path
    }

    /// Returns the logical path of the parent branch entry (second from top of the stack).
    /// Panics if the stack has fewer than 2 entries.
    pub(super) fn parent_logical_branch_path(&self, arena: &Arena<ArenaSparseNode>) -> Nibbles {
        logical_branch_path(arena, self.parent().expect("cursor must have a parent"))
    }

    /// Replaces the arena index stored in the head entry with `new_idx`, and updates the
    /// parent branch's children array to point to the new index. If the head is the root
    /// (stack has one entry), `root` is updated instead.
    pub(super) fn replace_head_index(
        &mut self,
        arena: &mut Arena<ArenaSparseNode>,
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
        let dense_idx = child_dense_index(parent_branch.state_mask, child_nibble)
            .expect("child nibble not found in parent state_mask");

        debug_assert!(
            matches!(
                parent_branch.children[dense_idx],
                ArenaSparseNodeBranchChild::Revealed(idx)
                if idx == old_idx
            ),
            "parent child at nibble {child_nibble} does not match old_idx",
        );

        parent_branch.children[dense_idx] = ArenaSparseNodeBranchChild::Revealed(new_idx);
    }

    /// Pops the stack until the head is an ancestor of `full_path`, then descends from that head
    /// toward `full_path`, pushing revealed branch (and leaf) children onto the stack until the
    /// deepest ancestor is reached.
    ///
    /// Returns a [`SeekResult`] describing the state at the stack head.
    #[instrument(level = "trace", target = "trie::arena", skip(self, arena), ret)]
    pub(super) fn seek(
        &mut self,
        arena: &mut Arena<ArenaSparseNode>,
        full_path: &Nibbles,
    ) -> SeekResult {
        // Pop stack until head is ancestor of full_path.
        while self.stack.len() > 1 &&
            !full_path.starts_with(&self.stack.last().expect("cursor has root").path)
        {
            self.pop(arena);
        }

        loop {
            let head = self.stack.last().expect("cursor has root");
            let head_idx = head.index;

            let head_branch = match &arena[head_idx] {
                ArenaSparseNode::EmptyRoot => {
                    return SeekResult::EmptyRoot;
                }
                ArenaSparseNode::Leaf { .. } => {
                    return SeekResult::RevealedLeaf;
                }
                ArenaSparseNode::Branch(b) => b,
                ArenaSparseNode::Subtrie(_) => {
                    return SeekResult::RevealedSubtrie;
                }
                _ => unreachable!("unexpected node type on stack: {:?}", arena[head_idx]),
            };

            let head_branch_logical_path = logical_branch_path(arena, head);

            // If full_path doesn't extend past the branch's logical path, the target is at or
            // within the branch's short_key — treat as diverged.
            if full_path.len() <= head_branch_logical_path.len() ||
                !full_path.starts_with(&head_branch_logical_path)
            {
                return SeekResult::Diverged;
            }

            let child_nibble = full_path.get_unchecked(head_branch_logical_path.len());
            let Some(dense_idx) = child_dense_index(head_branch.state_mask, child_nibble) else {
                return SeekResult::NoChild { child_nibble };
            };

            match &head_branch.children[dense_idx] {
                ArenaSparseNodeBranchChild::Blinded(_) => {
                    return SeekResult::Blinded;
                }
                ArenaSparseNodeBranchChild::Revealed(child_idx) => {
                    let child_idx = *child_idx;
                    let path = self.child_path(arena, child_nibble);
                    self.push(arena, child_idx, path);
                }
            }
        }
    }
}

/// Returns the logical path of a branch stack entry. The logical path is
/// `entry.path + branch.short_key`.
fn logical_branch_path(arena: &Arena<ArenaSparseNode>, entry: &ArenaCursorStackEntry) -> Nibbles {
    let mut path = entry.path;
    path.extend(&arena[entry.index].branch_ref().short_key);
    path
}

/// Returns the length of the logical path of a branch stack entry.
/// Equivalent to `logical_branch_path(arena, entry).len()` but avoids constructing the path.
fn logical_branch_path_len(arena: &Arena<ArenaSparseNode>, entry: &ArenaCursorStackEntry) -> usize {
    entry.path.len() + arena[entry.index].branch_ref().short_key.len()
}
