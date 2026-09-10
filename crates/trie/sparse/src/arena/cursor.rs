use super::{
    branch_child_idx::{BranchChildIdx, BranchChildIter},
    ArenaSparseNode, ArenaSparseNodeBranchChild, ArenaSparseNodeState, Index, NodeArena,
};
use alloc::vec::Vec;
use reth_trie_common::Nibbles;
use tracing::{instrument, trace};

const TRACE_TARGET: &str = "trie::arena::cursor";

/// An entry on the cursor's traversal stack, tracking an ancestor node during trie walks.
///
/// The node's absolute path is not stored: it is the first `path_len` nibbles of the cursor's
/// [`ArenaCursor::path`], which every entry on the stack is a prefix of.
#[derive(Debug, Clone)]
pub(super) struct ArenaCursorStackEntry {
    /// The arena index of this node.
    pub(super) index: Index,
    /// The nibble length of the absolute path of this node (not including its `short_key`).
    pub(super) path_len: u8,
    /// The dense index at which to resume child iteration in [`ArenaCursor::next`].
    /// Only meaningful when this entry's node is a branch.
    pub(super) next_dense_idx: u8,
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
    /// A path that every stack entry's path is a prefix of: the entry with `path_len` nibbles
    /// has the absolute path `path[..path_len]`.
    ///
    /// It holds the last seeked target, or the path of the deepest node a [`Self::next`] walk
    /// descended to, and may therefore be longer than the head's own path. Popping never
    /// shortens it, which is what keeps the ancestor test in [`Self::seek`] to a single
    /// [`Nibbles::common_prefix_length`].
    path: Nibbles,
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

    /// Returns the absolute path of the node at the top of the stack.
    pub(super) fn head_path(&self) -> Nibbles {
        self.entry_path(self.head_entry())
    }

    /// Returns the nibble length of the absolute path of the node at the top of the stack.
    /// Equivalent to `head_path().len()` but avoids constructing the path.
    pub(super) fn head_path_len(&self) -> usize {
        self.head_entry().path_len as usize
    }

    /// Returns the last nibble of the head node's absolute path, or `None` if that path is
    /// empty.
    pub(super) fn head_last_nibble(&self) -> Option<u8> {
        self.head_path_len().checked_sub(1).map(|i| self.path.get_unchecked(i))
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
        self.path = path;
        self.push(arena, idx, path.len());
    }

    /// Pushes an entry onto the stack for the node at the given index, whose absolute path is
    /// the first `path_len` nibbles of [`Self::path`].
    fn push(&mut self, arena: &NodeArena, idx: Index, path_len: usize) {
        debug_assert!(arena.contains_key(idx), "push called with invalid arena index");
        debug_assert!(
            path_len <= self.path.len(),
            "pushed path length {path_len} exceeds cursor path {:?}",
            self.path,
        );
        self.stack.push(ArenaCursorStackEntry {
            index: idx,
            path_len: path_len as u8,
            next_dense_idx: 0,
        });
        trace!(
            target: TRACE_TARGET,
            ?idx,
            path = ?self.path.slice_unchecked(0, path_len),
            "Pushed stack entry",
        );
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
            let entry_path = self.path.slice_unchecked(0, entry.path_len as usize);
            debug_assert_eq!(
                s.path, entry_path,
                "subtrie cached path {:?} does not match stack entry path {:?}",
                s.path, entry_path,
            );
        }

        if let Some(parent) = self.stack.last() {
            let child_is_dirty = arena.get(entry.index).is_some_and(|node| match node {
                ArenaSparseNode::Branch(b) => matches!(b.state, ArenaSparseNodeState::Dirty),
                ArenaSparseNode::Leaf { state, .. } => matches!(state, ArenaSparseNodeState::Dirty),
                ArenaSparseNode::Subtrie(s) => {
                    let root = &s.arena[s.root];
                    matches!(root.state_ref(), Some(ArenaSparseNodeState::Dirty))
                }
                _ => false,
            });
            if child_is_dirty {
                *arena[parent.index].state_mut() = ArenaSparseNodeState::Dirty;
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
    /// The logical path is `head_path() + branch.short_key`.
    pub(super) fn head_logical_branch_path(&self, arena: &NodeArena) -> Nibbles {
        self.logical_branch_path(arena, self.head_entry())
    }

    /// Returns the length of the logical path of the branch at the top of the stack.
    /// Equivalent to `head_logical_branch_path(arena).len()` but avoids constructing the path.
    pub(super) fn head_logical_branch_path_len(&self, arena: &NodeArena) -> usize {
        let head = self.head_entry();
        head.path_len as usize + arena[head.index].branch_ref().short_key.len()
    }

    /// Returns the logical path of the parent branch entry (second from top of the stack).
    /// Panics if the stack has fewer than 2 entries.
    pub(super) fn parent_logical_branch_path(&self, arena: &NodeArena) -> Nibbles {
        self.logical_branch_path(arena, self.parent().expect("cursor must have a parent"))
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
        let child_nibble = self.head_last_nibble();
        let head = self.stack.last_mut().expect("cursor must have head");
        let old_idx = head.index;
        head.index = new_idx;

        let Some(parent) = self.parent() else {
            *root = new_idx;
            return;
        };

        let child_nibble =
            child_nibble.expect("if cursor has a parent then the head path can't be empty");

        let parent_branch = arena[parent.index].branch_mut();
        let child_idx = BranchChildIdx::new(parent_branch.state_mask, child_nibble)
            .expect("child nibble not found in parent state_mask");

        debug_assert!(
            matches!(
                parent_branch.children[child_idx],
                ArenaSparseNodeBranchChild::Revealed(idx)
                if idx == old_idx
            ),
            "parent child at nibble {child_nibble} does not match old_idx",
        );

        parent_branch.children[child_idx] = ArenaSparseNodeBranchChild::Revealed(new_idx);
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
        should_descend: impl Fn(usize, &ArenaSparseNode) -> bool,
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
            let head_path_len = head.path_len as usize;

            let ArenaSparseNode::Branch(branch) = &arena[head_idx] else {
                self.needs_pop = true;
                return NextResult::NonBranch;
            };

            let state_mask = branch.state_mask;
            let start = head.next_dense_idx as usize;
            let child_depth = self.stack.len();

            let mut descended = false;
            for (branch_child_idx, nibble) in BranchChildIter::new(state_mask) {
                if branch_child_idx.get() < start {
                    continue;
                }

                let child_idx = match &arena[head_idx].branch_ref().children[branch_child_idx] {
                    ArenaSparseNodeBranchChild::Revealed(child_idx) => *child_idx,
                    ArenaSparseNodeBranchChild::Blinded(_) => continue,
                };

                if should_descend(child_depth, &arena[child_idx]) {
                    // Record where to resume iteration when we return to this entry.
                    self.stack.last_mut().expect("head exists").next_dense_idx =
                        branch_child_idx.get() as u8 + 1;
                    self.path.truncate(head_path_len);
                    self.path.extend(&arena[head_idx].branch_ref().short_key);
                    self.path.push_unchecked(nibble);
                    self.push(arena, child_idx, self.path.len());
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
        // Every entry's path is a prefix of `self.path`, so an entry is an ancestor of
        // `full_path` exactly when its length fits within the common prefix of the previous
        // target and this one.
        let common = self.path.common_prefix_length(full_path);
        while self.stack.len() > 1 && self.head_entry().path_len as usize > common {
            self.pop(arena);
        }

        if self.head_entry().path_len as usize > common {
            // The target is not below the walk's root. Callers only seek within the root's
            // prefix, so this cannot happen for a revealed root; leaving `self.path` alone
            // keeps the entry paths derivable.
            return SeekResult::Diverged;
        }
        self.path = *full_path;

        loop {
            let head = self.head_entry();
            let head_idx = head.index;
            let head_path_len = head.path_len as usize;

            let head_branch = match &arena[head_idx] {
                ArenaSparseNode::EmptyRoot { .. } => {
                    return SeekResult::EmptyRoot;
                }
                ArenaSparseNode::Leaf { key, .. } => {
                    return if full_path.slice_unchecked(head_path_len, full_path.len()) == *key {
                        SeekResult::RevealedLeaf
                    } else {
                        SeekResult::Diverged
                    };
                }
                ArenaSparseNode::Branch(b) => b,
                ArenaSparseNode::Subtrie(_) => {
                    return SeekResult::RevealedSubtrie;
                }
                _ => unreachable!("unexpected node type on stack: {:?}", arena[head_idx]),
            };

            let short_key = &head_branch.short_key;
            let logical_len = head_path_len + short_key.len();

            // If full_path doesn't extend past the branch's logical path, the target is at or
            // within the branch's short_key — treat as diverged. Most branches carry no
            // short_key, in which case there is nothing left to compare.
            if full_path.len() <= logical_len ||
                (!short_key.is_empty() &&
                    full_path.slice_unchecked(head_path_len, logical_len) != *short_key)
            {
                return SeekResult::Diverged;
            }

            let child_nibble = full_path.get_unchecked(logical_len);
            let Some(branch_child_idx) = BranchChildIdx::new(head_branch.state_mask, child_nibble)
            else {
                return SeekResult::NoChild { child_nibble };
            };

            match &head_branch.children[branch_child_idx] {
                ArenaSparseNodeBranchChild::Blinded(_) => {
                    return SeekResult::Blinded;
                }
                ArenaSparseNodeBranchChild::Revealed(child_idx) => {
                    let child_idx = *child_idx;
                    self.push(arena, child_idx, logical_len + 1);
                }
            }
        }
    }

    /// Returns the entry at the top of the stack, which must be non-empty.
    fn head_entry(&self) -> &ArenaCursorStackEntry {
        self.stack.last().expect("cursor is non-empty")
    }

    /// Returns the absolute path of a stack entry.
    fn entry_path(&self, entry: &ArenaCursorStackEntry) -> Nibbles {
        self.path.slice_unchecked(0, entry.path_len as usize)
    }

    /// Returns the logical path of a branch stack entry: `entry path + branch.short_key`.
    fn logical_branch_path(&self, arena: &NodeArena, entry: &ArenaCursorStackEntry) -> Nibbles {
        let mut path = self.entry_path(entry);
        path.extend(&arena[entry.index].branch_ref().short_key);
        path
    }
}
