use super::{
    branch_child_idx::{BranchChildIdx, BranchChildIter},
    ArenaParallelSparseTrie, ArenaSparseNode, ArenaSparseNodeBranchChild, ArenaSparseNodeState,
    Index, NodeArena,
};
use crate::{HashedCursor, TrieCursor};
use alloc::{format, vec::Vec};
use alloy_primitives::{B256, U256};
use alloy_rlp::Decodable;
use core::{cmp::Ordering, fmt::Debug};
use reth_primitives_traits::Account;
use reth_storage_errors::db::DatabaseError;
use reth_trie_common::{BranchNodeCompact, Nibbles, RlpNode, TrieAccount, TrieMask};
use tracing::{instrument, trace};

const TRACE_TARGET: &str = "trie::arena::cursor";

/// An entry on the cursor's traversal stack, tracking an ancestor node during trie walks.
#[derive(Debug, Clone)]
pub(super) struct ArenaCursorStackEntry {
    /// The arena index of this node.
    pub(super) index: Index,
    /// The absolute path of this node in the trie (not including its `short_key`).
    pub(super) path: Nibbles,
    /// Whether this branch has already been yielded to cached trie cursors.
    pub(super) branch_emitted: bool,
    /// The child nibble at which to resume iteration in [`ArenaCursor::next`].
    /// Only meaningful when this entry's node is a branch.
    pub(super) next_child_nibble: u8,
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

/// Result of read-only sparse traversal for cursor-backed cached topology.
#[derive(Debug)]
enum CachedCursorEvent {
    /// No user-visible item was produced; continue traversal.
    Continue,
    /// The current stack head is a non-branch node.
    NonBranch,
    /// The current stack head is a branch that should be emitted before its children.
    Branch,
    /// The current stack head has a blinded child at this nibble.
    BlindChild { nibble: u8 },
    /// The stack is empty.
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
        self.stack.push(ArenaCursorStackEntry {
            index: idx,
            path,
            branch_emitted: false,
            next_child_nibble: 0,
        });
        trace!(target: TRACE_TARGET, entry = ?self.stack.last().expect("just pushed"), "Pushed stack entry");
    }

    /// Pops the top entry from the stack and propagates dirty state to the parent.
    /// Returns the popped entry.
    ///
    /// Uses `arena.get()` for the popped node because callers (e.g. pruning) may remove
    /// the node from the arena between the time it was pushed and the time it is popped.
    #[instrument(level = "trace", target = TRACE_TARGET, skip(self, arena))]
    pub(super) fn pop(&mut self, arena: &mut NodeArena) -> ArenaCursorStackEntry {
        let (entry, child_is_dirty) = self.pop_inner(arena);

        if child_is_dirty && let Some(parent) = self.stack.last() {
            *arena[parent.index].state_mut() = ArenaSparseNodeState::Dirty;
        }

        entry
    }

    /// Pops the top entry from a cursor over cached trie data.
    ///
    /// Cached cursors are read-only, so dirty nodes indicate the cached topology is not safe to
    /// serve through a cursor.
    #[allow(dead_code)]
    #[instrument(level = "trace", target = TRACE_TARGET, skip(self, arena))]
    pub(super) fn pop_cached(&mut self, arena: &NodeArena) -> ArenaCursorStackEntry {
        let (entry, child_is_dirty) = self.pop_inner(arena);
        assert!(!child_is_dirty, "cached arena cursor encountered dirty node at {:?}", entry.path);
        entry
    }

    fn pop_inner(&mut self, arena: &NodeArena) -> (ArenaCursorStackEntry, bool) {
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

        let child_is_dirty = arena_node_is_dirty(arena, entry.index);

        (entry, child_is_dirty)
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

        self.next_inner(arena, should_descend)
    }

    /// Advances a read-only cursor over cached trie data.
    ///
    /// This is equivalent to [`Self::next`], but pops with [`Self::pop_cached`] so dirty nodes
    /// trigger a panic instead of propagating dirty state.
    #[allow(dead_code)]
    #[instrument(level = "trace", target = TRACE_TARGET, skip_all, ret)]
    pub(super) fn next_cached(
        &mut self,
        arena: &NodeArena,
        should_descend: impl Fn(usize, &ArenaSparseNode) -> bool,
    ) -> NextResult {
        if self.needs_pop {
            self.pop_cached(arena);
            self.needs_pop = false;
        }

        self.next_inner(arena, should_descend)
    }

    /// Advances a read-only cursor over cached topology, returning blinded children instead of
    /// skipping them so overlay cursors can delegate only those ranges to the inner cursor.
    #[instrument(level = "trace", target = TRACE_TARGET, skip_all, ret)]
    fn next_cached_event(
        &mut self,
        arena: &NodeArena,
        emit_branches: bool,
        should_descend: impl Fn(usize, &ArenaSparseNode) -> bool,
    ) -> Result<CachedCursorEvent, DatabaseError> {
        debug_assert!(
            !self.needs_pop,
            "pending cached pop must be handled by the owner so it can update active arena state"
        );

        loop {
            let Some(head) = self.stack.last_mut() else { return Ok(CachedCursorEvent::Done) };
            let head_idx = head.index;

            let ArenaSparseNode::Branch(branch) = &arena[head_idx] else {
                self.needs_pop = true;
                return Ok(CachedCursorEvent::NonBranch)
            };
            ensure_node_not_dirty(&branch.state, head.path)?;

            if emit_branches && !head.branch_emitted {
                head.branch_emitted = true;
                return Ok(CachedCursorEvent::Branch)
            }

            let state_mask = branch.state_mask;
            let start_nibble = head.next_child_nibble;
            let child_depth = self.stack.len();

            let mut descended = false;
            for (branch_child_idx, nibble) in BranchChildIter::from_nibble(state_mask, start_nibble)
            {
                self.stack.last_mut().expect("head exists").next_child_nibble = nibble + 1;

                let child_idx = match &arena[head_idx].branch_ref().children[branch_child_idx] {
                    ArenaSparseNodeBranchChild::Blinded(_) => {
                        return Ok(CachedCursorEvent::BlindChild { nibble })
                    }
                    ArenaSparseNodeBranchChild::Revealed(child_idx) => *child_idx,
                };

                if should_descend(child_depth, &arena[child_idx]) {
                    let path = self.child_path(arena, nibble);
                    self.push(arena, child_idx, path);
                    descended = true;
                    break;
                }
            }

            if !descended {
                self.needs_pop = true;
                return Ok(CachedCursorEvent::Continue)
            }
        }
    }

    fn next_inner(
        &mut self,
        arena: &NodeArena,
        should_descend: impl Fn(usize, &ArenaSparseNode) -> bool,
    ) -> NextResult {
        loop {
            let Some(head) = self.stack.last_mut() else {
                return NextResult::Done;
            };
            let head_idx = head.index;

            let ArenaSparseNode::Branch(branch) = &arena[head_idx] else {
                self.needs_pop = true;
                return NextResult::NonBranch;
            };

            let state_mask = branch.state_mask;
            let start_nibble = head.next_child_nibble;
            let child_depth = self.stack.len();

            let mut descended = false;
            for (branch_child_idx, nibble) in BranchChildIter::from_nibble(state_mask, start_nibble)
            {
                let child_idx = match &arena[head_idx].branch_ref().children[branch_child_idx] {
                    ArenaSparseNodeBranchChild::Revealed(child_idx) => *child_idx,
                    ArenaSparseNodeBranchChild::Blinded(_) => continue,
                };

                if should_descend(child_depth, &arena[child_idx]) {
                    // Record where to resume iteration when we return to this entry.
                    self.stack.last_mut().expect("head exists").next_child_nibble = nibble + 1;
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

        self.seek_inner(arena, full_path)
    }

    /// Seeks a read-only cursor over cached trie data.
    ///
    /// This is equivalent to [`Self::seek`], but pops with [`Self::pop_cached`] so dirty nodes
    /// trigger a panic instead of propagating dirty state.
    #[allow(dead_code)]
    #[instrument(level = "trace", target = TRACE_TARGET, skip(self, arena), ret)]
    pub(super) fn seek_cached(&mut self, arena: &NodeArena, full_path: &Nibbles) -> SeekResult {
        // Pop stack until head is ancestor of full_path.
        while self.stack.len() > 1 &&
            !full_path.starts_with(&self.stack.last().expect("cursor has root").path)
        {
            self.pop_cached(arena);
        }

        self.seek_inner(arena, full_path)
    }

    fn seek_inner(&mut self, arena: &NodeArena, full_path: &Nibbles) -> SeekResult {
        loop {
            let head = self.stack.last().expect("cursor has root");
            let head_idx = head.index;

            let head_branch = match &arena[head_idx] {
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
                    let path = self.child_path(arena, child_nibble);
                    self.push(arena, child_idx, path);
                }
            }
        }
    }
}

/// Cursor backed by cached sparse-trie topology and an inner database cursor.
#[derive(Debug, Clone)]
pub struct ArenaCachedCursor<'a, C, K, V = ()> {
    trie: Option<&'a ArenaParallelSparseTrie>,
    cursor: ArenaCursor,
    active_subtrie: Option<&'a super::ArenaSparseSubtrie>,
    mode: CursorMode<K>,
    current: Option<(K, V)>,
    inner: C,
}

impl<'a, C, K, V> ArenaCachedCursor<'a, C, K, V> {
    /// Creates a new cursor for `trie`, delegating blinded ranges to `inner`.
    pub fn new(trie: Option<&'a ArenaParallelSparseTrie>, inner: C) -> Self {
        let mut this = Self {
            trie,
            cursor: ArenaCursor::default(),
            active_subtrie: None,
            mode: CursorMode::Sparse,
            current: None,
            inner,
        };
        this.reset_sparse_cursor();
        this
    }

    /// Updates the sparse trie backing this cursor.
    pub fn set_trie(&mut self, trie: Option<&'a ArenaParallelSparseTrie>) {
        self.trie = trie;
        self.reset_sparse_cursor();
    }

    /// Returns the inner cursor.
    pub fn inner_mut(&mut self) -> &mut C {
        &mut self.inner
    }

    fn reset_sparse_cursor(&mut self) {
        self.cursor = ArenaCursor::default();
        self.active_subtrie = None;
        if let Some(trie) = self.trie {
            self.cursor = trie.cached_cursor();
        }
        self.mode = CursorMode::Sparse;
        self.current = None;
    }

    fn active_arena(&self) -> Option<&'a NodeArena> {
        self.active_subtrie
            .map(|subtrie| &subtrie.arena)
            .or_else(|| self.trie.map(|trie| &trie.upper_arena))
    }

    fn pop_cached(&mut self) {
        if let Some(subtrie) = self.active_subtrie {
            self.cursor.pop_cached(&subtrie.arena);
            if !self.cursor.head().is_some_and(|entry| entry.path.starts_with(&subtrie.path)) {
                self.active_subtrie = None;
            }
        } else if let Some(trie) = self.trie {
            self.cursor.pop_cached(&trie.upper_arena);
        }
    }

    fn enter_subtrie(&mut self, subtrie: &'a super::ArenaSparseSubtrie) {
        let head = self.cursor.stack.last_mut().expect("head exists");
        head.index = subtrie.root;
        head.path = subtrie.path;
        head.branch_emitted = false;
        head.next_child_nibble = 0;
        self.cursor.needs_pop = false;
        self.active_subtrie = Some(subtrie);
    }

    fn next_sparse_event(
        &mut self,
        emit_branches: bool,
    ) -> Result<CachedCursorEvent, DatabaseError> {
        loop {
            if self.cursor.needs_pop {
                self.pop_cached();
                self.cursor.needs_pop = false;
            }

            let Some(arena) = self.active_arena() else { return Ok(CachedCursorEvent::Done) };

            match self.cursor.next_cached_event(arena, emit_branches, |_, _| true)? {
                CachedCursorEvent::Continue => continue,
                event => return Ok(event),
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

fn arena_node_is_dirty(arena: &NodeArena, idx: Index) -> bool {
    arena.get(idx).is_some_and(|node| match node {
        ArenaSparseNode::Branch(b) => matches!(b.state, ArenaSparseNodeState::Dirty),
        ArenaSparseNode::Leaf { state, .. } => matches!(state, ArenaSparseNodeState::Dirty),
        ArenaSparseNode::Subtrie(s) => {
            let root = &s.arena[s.root];
            matches!(root, ArenaSparseNode::EmptyRoot) ||
                matches!(root.state_ref(), Some(ArenaSparseNodeState::Dirty))
        }
        _ => false,
    })
}

#[derive(Debug, Clone, Copy)]
enum CursorMode<K> {
    Sparse,
    InnerRange { end: Option<K> },
    Exhausted,
}

impl<C, V> ArenaCachedCursor<'_, C, B256, V>
where
    C: HashedCursor<Value = V>,
    V: ArenaCachedHashedValue + Clone,
{
    fn current_hashed_key(&self) -> Option<B256> {
        self.current.as_ref().map(|(key, _)| *key)
    }

    fn set_current_hashed(&mut self, key: B256, value: V) -> (B256, V) {
        self.current = Some((key, value.clone()));
        (key, value)
    }

    fn reset_for_hashed_seek(&mut self) {
        self.reset_sparse_cursor();
        self.inner.reset();
    }

    fn next_inner_in_active_hashed_range(&mut self) -> Result<Option<(B256, V)>, DatabaseError> {
        let CursorMode::InnerRange { end } = self.mode else { return Ok(None) };

        let mut entry = self.inner.next()?;
        while let Some((key, value)) = entry {
            if !key_is_before_end(key, end) {
                self.mode = CursorMode::Sparse;
                return self.next_from_hashed_topology(None)
            }
            if self.current_hashed_key().is_none_or(|current_key| key > current_key) {
                return Ok(Some(self.set_current_hashed(key, value)))
            }
            entry = self.inner.next()?;
        }

        self.mode = CursorMode::Sparse;
        if self.trie.is_none() {
            self.mode = CursorMode::Exhausted;
            return Ok(None)
        }
        self.next_from_hashed_topology(None)
    }

    fn seek_inner_hashed_range(
        &mut self,
        start: B256,
        end: Option<B256>,
    ) -> Result<Option<(B256, V)>, DatabaseError> {
        let mut entry = self.inner.seek(start)?;
        while let Some((key, value)) = entry {
            if !key_is_before_end(key, end) {
                return Ok(None)
            }
            if self.current_hashed_key().is_none_or(|current_key| key > current_key) {
                self.mode = CursorMode::InnerRange { end };
                return Ok(Some(self.set_current_hashed(key, value)))
            }
            entry = self.inner.next()?;
        }
        Ok(None)
    }

    fn next_from_hashed_topology(
        &mut self,
        seek_key: Option<B256>,
    ) -> Result<Option<(B256, V)>, DatabaseError> {
        if matches!(self.mode, CursorMode::Exhausted) {
            return Ok(None)
        }

        if self.trie.is_none() {
            let start = seek_key.unwrap_or(B256::ZERO);
            let result = self.seek_inner_hashed_range(start, None)?;
            if result.is_none() {
                self.mode = CursorMode::Exhausted;
            }
            return Ok(result)
        }

        loop {
            match self.next_sparse_event(false)? {
                CachedCursorEvent::Continue => continue,
                CachedCursorEvent::Done => {
                    self.mode = CursorMode::Exhausted;
                    return Ok(None)
                }
                CachedCursorEvent::Branch => {
                    unreachable!("hashed sparse cursor never emits branch nodes")
                }
                CachedCursorEvent::BlindChild { nibble } => {
                    let Some(arena) = self.active_arena() else {
                        self.mode = CursorMode::Exhausted;
                        return Ok(None)
                    };
                    let head = self.cursor.head().expect("blind child event requires a head");
                    let branch = arena[head.index].branch_ref();
                    let Some((start, end)) =
                        hashed_child_prefix_range(&head.path, &branch.short_key, nibble)
                    else {
                        continue
                    };
                    if seek_key.is_some_and(|seek_key| !key_is_before_end(seek_key, end)) {
                        continue
                    }
                    let inner_seek_key = seek_key.map_or(start, |seek_key| start.max(seek_key));
                    if let Some(entry) = self.seek_inner_hashed_range(inner_seek_key, end)? {
                        return Ok(Some(entry))
                    }
                }
                CachedCursorEvent::NonBranch => {
                    let Some(arena) = self.active_arena() else {
                        self.mode = CursorMode::Exhausted;
                        return Ok(None)
                    };
                    let head = self.cursor.head().expect("non-branch event requires a head");
                    match &arena[head.index] {
                        ArenaSparseNode::EmptyRoot => {}
                        ArenaSparseNode::Leaf { state, value, key } => {
                            ensure_node_not_dirty(state, head.path)?;
                            let Some(key) = hashed_leaf_key(&head.path, key) else { continue };
                            if seek_key.is_none_or(|seek_key| key >= seek_key) &&
                                self.current_hashed_key()
                                    .is_none_or(|current_key| key > current_key)
                            {
                                let value = V::decode(value)?;
                                return Ok(Some(self.set_current_hashed(key, value)))
                            }
                        }
                        ArenaSparseNode::Subtrie(subtrie) => {
                            ensure_cached_subtrie(subtrie)?;
                            self.enter_subtrie(subtrie);
                        }
                        ArenaSparseNode::TakenSubtrie => {
                            let Some((start, end)) = hashed_prefix_range(&head.path) else {
                                continue
                            };
                            if seek_key.is_some_and(|seek_key| !key_is_before_end(seek_key, end)) {
                                continue
                            }
                            let inner_seek_key =
                                seek_key.map_or(start, |seek_key| start.max(seek_key));
                            if let Some(entry) =
                                self.seek_inner_hashed_range(inner_seek_key, end)?
                            {
                                return Ok(Some(entry))
                            }
                        }
                        ArenaSparseNode::Branch(_) => {
                            unreachable!("non-branch event cannot point at a branch")
                        }
                    }
                }
            }
        }
    }
}

impl<C, V> HashedCursor for ArenaCachedCursor<'_, C, B256, V>
where
    C: HashedCursor<Value = V>,
    V: ArenaCachedHashedValue + Clone,
{
    type Value = V;

    fn seek(&mut self, key: B256) -> Result<Option<(B256, Self::Value)>, DatabaseError> {
        if let Some((current_key, current_value)) = self.current.as_ref() {
            match key.cmp(current_key) {
                Ordering::Less => self.reset_for_hashed_seek(),
                Ordering::Equal => return Ok(Some((*current_key, current_value.clone()))),
                Ordering::Greater if matches!(self.mode, CursorMode::Exhausted) => return Ok(None),
                Ordering::Greater => {}
            }
        } else if matches!(self.mode, CursorMode::Exhausted) {
            self.reset_for_hashed_seek();
        }

        match self.mode {
            CursorMode::InnerRange { end } if key_is_before_end(key, end) => {
                if let Some(entry) = self.seek_inner_hashed_range(key, end)? {
                    return Ok(Some(entry))
                }
                self.mode = CursorMode::Sparse;
                if self.trie.is_none() {
                    self.mode = CursorMode::Exhausted;
                    return Ok(None)
                }
                self.next_from_hashed_topology(Some(key))
            }
            CursorMode::InnerRange { .. } => {
                self.mode = CursorMode::Sparse;
                self.next_from_hashed_topology(Some(key))
            }
            CursorMode::Sparse | CursorMode::Exhausted => self.next_from_hashed_topology(Some(key)),
        }
    }

    fn next(&mut self) -> Result<Option<(B256, Self::Value)>, DatabaseError> {
        match self.mode {
            CursorMode::InnerRange { .. } => self.next_inner_in_active_hashed_range(),
            CursorMode::Sparse | CursorMode::Exhausted => self.next_from_hashed_topology(None),
        }
    }

    fn reset(&mut self) {
        self.reset_sparse_cursor();
        self.inner.reset();
    }
}

impl<C> ArenaCachedCursor<'_, C, Nibbles, BranchNodeCompact>
where
    C: TrieCursor,
{
    fn current_trie_key(&self) -> Option<&Nibbles> {
        self.current.as_ref().map(|(key, _)| key)
    }

    fn set_current_trie(
        &mut self,
        key: Nibbles,
        node: BranchNodeCompact,
    ) -> (Nibbles, BranchNodeCompact) {
        self.current = Some((key, node.clone()));
        (key, node)
    }

    fn reset_for_trie_seek(&mut self) {
        self.reset_sparse_cursor();
        self.inner.reset();
    }

    fn next_inner_in_active_trie_range(
        &mut self,
    ) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
        let CursorMode::InnerRange { end } = self.mode else { return Ok(None) };

        let mut entry = self.inner.next()?;
        while let Some((key, node)) = entry {
            if !trie_key_is_before_end(&key, end.as_ref()) {
                self.mode = CursorMode::Sparse;
                return self.next_from_trie_topology(None)
            }
            if self.current_trie_key().is_none_or(|current_key| &key > current_key) {
                return Ok(Some(self.set_current_trie(key, node)))
            }
            entry = self.inner.next()?;
        }

        self.mode = CursorMode::Sparse;
        if self.trie.is_none() {
            self.mode = CursorMode::Exhausted;
            return Ok(None)
        }
        self.next_from_trie_topology(None)
    }

    fn seek_inner_trie_range(
        &mut self,
        start: Nibbles,
        end: Option<Nibbles>,
        exact: bool,
    ) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
        let mut entry =
            if exact { self.inner.seek_exact(start)? } else { self.inner.seek(start)? };
        while let Some((key, node)) = entry {
            if !trie_key_is_before_end(&key, end.as_ref()) {
                return Ok(None)
            }
            if self.current_trie_key().is_none_or(|current_key| &key > current_key) {
                self.mode = CursorMode::InnerRange { end };
                return Ok(Some(self.set_current_trie(key, node)))
            }
            entry = self.inner.next()?;
        }
        Ok(None)
    }

    fn next_from_trie_topology(
        &mut self,
        seek_key: Option<Nibbles>,
    ) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
        if matches!(self.mode, CursorMode::Exhausted) {
            return Ok(None)
        }

        if self.trie.is_none() {
            let start = seek_key.unwrap_or_default();
            let result = self.seek_inner_trie_range(start, None, false)?;
            if result.is_none() {
                self.mode = CursorMode::Exhausted;
            }
            return Ok(result)
        }

        loop {
            match self.next_sparse_event(true)? {
                CachedCursorEvent::Continue => continue,
                CachedCursorEvent::Done => {
                    self.mode = CursorMode::Exhausted;
                    return Ok(None)
                }
                CachedCursorEvent::Branch => {
                    let Some(arena) = self.active_arena() else {
                        self.mode = CursorMode::Exhausted;
                        return Ok(None)
                    };
                    let head = self.cursor.head().expect("branch event requires a head");
                    let branch = arena[head.index].branch_ref();
                    let path = logical_branch_path(arena, head);
                    if seek_key.as_ref().is_none_or(|seek_key| &path >= seek_key) &&
                        self.current_trie_key().is_none_or(|current_key| &path > current_key)
                    {
                        let node = branch_node_compact_for_cursor(arena, branch, path)?;
                        return Ok(Some(self.set_current_trie(path, node)))
                    }
                }
                CachedCursorEvent::BlindChild { nibble } => {
                    let Some(arena) = self.active_arena() else {
                        self.mode = CursorMode::Exhausted;
                        return Ok(None)
                    };
                    let path = self.cursor.child_path(arena, nibble);
                    let end = next_prefix(&path);
                    if seek_key
                        .as_ref()
                        .is_some_and(|seek_key| !trie_key_is_before_end(seek_key, end.as_ref()))
                    {
                        continue
                    }
                    let inner_seek_key = seek_key.map_or(path, |seek_key| path.max(seek_key));
                    if let Some(entry) = self.seek_inner_trie_range(inner_seek_key, end, false)? {
                        return Ok(Some(entry))
                    }
                }
                CachedCursorEvent::NonBranch => {
                    let Some(arena) = self.active_arena() else {
                        self.mode = CursorMode::Exhausted;
                        return Ok(None)
                    };
                    let head = self.cursor.head().expect("non-branch event requires a head");
                    match &arena[head.index] {
                        ArenaSparseNode::EmptyRoot | ArenaSparseNode::Leaf { .. } => {}
                        ArenaSparseNode::Subtrie(subtrie) => {
                            ensure_cached_subtrie(subtrie)?;
                            self.enter_subtrie(subtrie);
                        }
                        ArenaSparseNode::TakenSubtrie => {
                            let path = head.path;
                            let end = next_prefix(&path);
                            if seek_key.as_ref().is_some_and(|seek_key| {
                                !trie_key_is_before_end(seek_key, end.as_ref())
                            }) {
                                continue
                            }
                            let inner_seek_key =
                                seek_key.map_or(path, |seek_key| path.max(seek_key));
                            if let Some(entry) =
                                self.seek_inner_trie_range(inner_seek_key, end, false)?
                            {
                                return Ok(Some(entry))
                            }
                        }
                        ArenaSparseNode::Branch(_) => {
                            unreachable!("non-branch event cannot point at a branch")
                        }
                    }
                }
            }
        }
    }

    fn seek_exact_from_trie_topology(
        &mut self,
        key: Nibbles,
    ) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
        if matches!(self.mode, CursorMode::Exhausted) {
            return Ok(None)
        }

        if self.trie.is_none() {
            let result = self.seek_inner_trie_range(key, None, true)?;
            if result.is_none() {
                self.mode = CursorMode::Exhausted;
            }
            return Ok(result)
        }

        loop {
            match self.next_sparse_event(true)? {
                CachedCursorEvent::Continue => continue,
                CachedCursorEvent::Done => {
                    self.mode = CursorMode::Exhausted;
                    return Ok(None)
                }
                CachedCursorEvent::Branch => {
                    let Some(arena) = self.active_arena() else {
                        self.mode = CursorMode::Exhausted;
                        return Ok(None)
                    };
                    let head = self.cursor.head().expect("branch event requires a head");
                    let branch = arena[head.index].branch_ref();
                    let path = logical_branch_path(arena, head);
                    if path == key {
                        let node = branch_node_compact_for_cursor(arena, branch, path)?;
                        return Ok(Some(self.set_current_trie(path, node)))
                    }
                    if path > key {
                        return Ok(None)
                    }
                }
                CachedCursorEvent::BlindChild { nibble } => {
                    let Some(arena) = self.active_arena() else {
                        self.mode = CursorMode::Exhausted;
                        return Ok(None)
                    };
                    let path = self.cursor.child_path(arena, nibble);
                    let end = next_prefix(&path);
                    if key < path {
                        return Ok(None)
                    }
                    if trie_key_is_before_end(&key, end.as_ref()) {
                        return self.seek_inner_trie_range(key, end, true)
                    }
                }
                CachedCursorEvent::NonBranch => {
                    let Some(arena) = self.active_arena() else {
                        self.mode = CursorMode::Exhausted;
                        return Ok(None)
                    };
                    let head = self.cursor.head().expect("non-branch event requires a head");
                    match &arena[head.index] {
                        ArenaSparseNode::EmptyRoot | ArenaSparseNode::Leaf { .. } => {}
                        ArenaSparseNode::Subtrie(subtrie) => {
                            ensure_cached_subtrie(subtrie)?;
                            self.enter_subtrie(subtrie);
                        }
                        ArenaSparseNode::TakenSubtrie => {
                            let path = head.path;
                            let end = next_prefix(&path);
                            if key < path {
                                return Ok(None)
                            }
                            if trie_key_is_before_end(&key, end.as_ref()) {
                                return self.seek_inner_trie_range(key, end, true)
                            }
                        }
                        ArenaSparseNode::Branch(_) => {
                            unreachable!("non-branch event cannot point at a branch")
                        }
                    }
                }
            }
        }
    }
}

impl<C> TrieCursor for ArenaCachedCursor<'_, C, Nibbles, BranchNodeCompact>
where
    C: TrieCursor,
{
    fn seek_exact(
        &mut self,
        key: Nibbles,
    ) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
        if let Some((current_key, current_node)) = self.current.as_ref() {
            match key.cmp(current_key) {
                Ordering::Less => self.reset_for_trie_seek(),
                Ordering::Equal => return Ok(Some((*current_key, current_node.clone()))),
                Ordering::Greater if matches!(self.mode, CursorMode::Exhausted) => return Ok(None),
                Ordering::Greater => {}
            }
        } else if matches!(self.mode, CursorMode::Exhausted) {
            self.reset_for_trie_seek();
        }

        match self.mode {
            CursorMode::InnerRange { end } if trie_key_is_before_end(&key, end.as_ref()) => {
                self.seek_inner_trie_range(key, end, true)
            }
            CursorMode::InnerRange { .. } => {
                self.mode = CursorMode::Sparse;
                self.seek_exact_from_trie_topology(key)
            }
            CursorMode::Sparse | CursorMode::Exhausted => self.seek_exact_from_trie_topology(key),
        }
    }

    fn seek(
        &mut self,
        key: Nibbles,
    ) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
        if let Some((current_key, current_node)) = self.current.as_ref() {
            match key.cmp(current_key) {
                Ordering::Less => self.reset_for_trie_seek(),
                Ordering::Equal => return Ok(Some((*current_key, current_node.clone()))),
                Ordering::Greater if matches!(self.mode, CursorMode::Exhausted) => return Ok(None),
                Ordering::Greater => {}
            }
        } else if matches!(self.mode, CursorMode::Exhausted) {
            self.reset_for_trie_seek();
        }

        match self.mode {
            CursorMode::InnerRange { end } if trie_key_is_before_end(&key, end.as_ref()) => {
                if let Some(entry) = self.seek_inner_trie_range(key, end, false)? {
                    return Ok(Some(entry))
                }
                self.mode = CursorMode::Sparse;
                if self.trie.is_none() {
                    self.mode = CursorMode::Exhausted;
                    return Ok(None)
                }
                self.next_from_trie_topology(Some(key))
            }
            CursorMode::InnerRange { .. } => {
                self.mode = CursorMode::Sparse;
                self.next_from_trie_topology(Some(key))
            }
            CursorMode::Sparse | CursorMode::Exhausted => self.next_from_trie_topology(Some(key)),
        }
    }

    fn next(&mut self) -> Result<Option<(Nibbles, BranchNodeCompact)>, DatabaseError> {
        match self.mode {
            CursorMode::InnerRange { .. } => self.next_inner_in_active_trie_range(),
            CursorMode::Sparse | CursorMode::Exhausted => self.next_from_trie_topology(None),
        }
    }

    fn current(&mut self) -> Result<Option<Nibbles>, DatabaseError> {
        Ok(self.current.as_ref().map(|(key, _)| *key))
    }

    fn reset(&mut self) {
        self.reset_sparse_cursor();
        self.inner.reset();
    }
}

/// Value decoder for sparse trie leaf values returned by [`ArenaCachedCursor`].
pub trait ArenaCachedHashedValue: Debug {
    /// Decodes an RLP-encoded sparse trie leaf value into the hashed cursor value.
    fn decode(value: &[u8]) -> Result<Self, DatabaseError>
    where
        Self: Sized;
}

impl ArenaCachedHashedValue for Account {
    fn decode(value: &[u8]) -> Result<Self, DatabaseError> {
        let mut value = value;
        let trie_account = TrieAccount::decode(&mut value).map_err(|error| {
            DatabaseError::Other(format!("failed to decode trie account: {error}"))
        })?;
        Ok(trie_account.into())
    }
}

impl ArenaCachedHashedValue for U256 {
    fn decode(value: &[u8]) -> Result<Self, DatabaseError> {
        let mut value = value;
        <U256 as Decodable>::decode(&mut value).map_err(|error| {
            DatabaseError::Other(format!("failed to decode storage value: {error}"))
        })
    }
}

fn ensure_cached_subtrie(subtrie: &super::ArenaSparseSubtrie) -> Result<(), DatabaseError> {
    match &subtrie.arena[subtrie.root] {
        ArenaSparseNode::EmptyRoot => Ok(()),
        node => {
            ensure_node_not_dirty(
                node.state_ref().ok_or_else(|| {
                    DatabaseError::Other(format!(
                        "sparse trie cursor encountered uncached subtrie at {:?}",
                        subtrie.path
                    ))
                })?,
                subtrie.path,
            )?;
            Ok(())
        }
    }
}

fn branch_node_compact_for_cursor(
    arena: &NodeArena,
    branch: &super::ArenaSparseNodeBranch,
    path: Nibbles,
) -> Result<BranchNodeCompact, DatabaseError> {
    let mut tree_mask = TrieMask::default();
    let mut hash_mask = TrieMask::default();
    let mut hashes = Vec::with_capacity(branch.state_mask.count_bits() as usize);

    for (nibble, child) in branch.child_iter() {
        let child_path = {
            let mut child_path = path;
            child_path.push_unchecked(nibble);
            child_path
        };

        let (child_hash_bit, child_tree_bit, child_hash) = match child {
            ArenaSparseNodeBranchChild::Blinded(rlp_node) => {
                let hash_bit = branch.branch_masks.hash_mask.is_bit_set(nibble);
                let hash = hash_bit
                    .then(|| {
                        rlp_node.as_hash().ok_or_else(|| {
                            DatabaseError::Other(format!(
                                "sparse trie cursor encountered non-hash blinded child at {child_path:?}"
                            ))
                        })
                    })
                    .transpose()?;
                (hash_bit, branch.branch_masks.tree_mask.is_bit_set(nibble), hash)
            }
            ArenaSparseNodeBranchChild::Revealed(child_idx) => {
                child_cursor_masks_and_hash(arena, *child_idx, child_path)?
            }
        };

        if child_tree_bit {
            tree_mask.set_bit(nibble);
        }
        if child_hash_bit {
            hash_mask.set_bit(nibble);
        }
        if let Some(hash) = child_hash {
            hashes.push(hash);
        }
    }

    Ok(BranchNodeCompact::new(branch.state_mask, tree_mask, hash_mask, hashes, None))
}

fn child_cursor_masks_and_hash(
    arena: &NodeArena,
    idx: Index,
    path: Nibbles,
) -> Result<(bool, bool, Option<B256>), DatabaseError> {
    match &arena[idx] {
        ArenaSparseNode::EmptyRoot => Ok((false, false, None)),
        ArenaSparseNode::Branch(branch) => {
            ensure_node_not_dirty(&branch.state, path)?;
            let rlp_node = cached_rlp_node_for_cursor(&branch.state, path)?;
            let hash_bit = branch.short_key.is_empty() && rlp_node.is_hash();
            let hash = hash_bit.then(|| {
                rlp_node.as_hash().expect("hash bit set only when RlpNode is already a hash")
            });
            Ok((hash_bit, !branch.branch_masks.is_empty(), hash))
        }
        ArenaSparseNode::Leaf { state, .. } => {
            ensure_node_not_dirty(state, path)?;
            Ok((false, false, None))
        }
        ArenaSparseNode::Subtrie(subtrie) => {
            child_cursor_masks_and_hash(&subtrie.arena, subtrie.root, subtrie.path)
        }
        ArenaSparseNode::TakenSubtrie => Ok((false, true, None)),
    }
}

fn cached_rlp_node_for_cursor(
    state: &ArenaSparseNodeState,
    path: Nibbles,
) -> Result<&RlpNode, DatabaseError> {
    ensure_node_not_dirty(state, path)?;
    state.cached_rlp_node().ok_or_else(|| {
        DatabaseError::Other(format!("sparse trie cursor encountered uncached node at {path:?}"))
    })
}

fn ensure_node_not_dirty(state: &ArenaSparseNodeState, path: Nibbles) -> Result<(), DatabaseError> {
    if matches!(state, ArenaSparseNodeState::Dirty) {
        return Err(DatabaseError::Other(format!(
            "sparse trie cursor encountered dirty node at {path:?}"
        )))
    }
    Ok(())
}

fn hashed_leaf_key(path: &Nibbles, leaf_key: &Nibbles) -> Option<B256> {
    if path.len() + leaf_key.len() != B256::len_bytes() * 2 {
        return None
    }

    let mut bytes = [0u8; 32];
    let mut idx = 0;
    pack_nibbles(&mut bytes, &mut idx, path);
    pack_nibbles(&mut bytes, &mut idx, leaf_key);
    Some(B256::from(bytes))
}

fn hashed_prefix_range(prefix: &Nibbles) -> Option<(B256, Option<B256>)> {
    hashed_prefix_range_parts(prefix, None, None)
}

fn hashed_child_prefix_range(
    path: &Nibbles,
    short_key: &Nibbles,
    child_nibble: u8,
) -> Option<(B256, Option<B256>)> {
    hashed_prefix_range_parts(path, Some(short_key), Some(child_nibble))
}

fn hashed_prefix_range_parts(
    first: &Nibbles,
    second: Option<&Nibbles>,
    child_nibble: Option<u8>,
) -> Option<(B256, Option<B256>)> {
    let len = first.len() + second.map_or(0, Nibbles::len) + usize::from(child_nibble.is_some());
    if len > B256::len_bytes() * 2 {
        return None
    }

    let mut bytes = [0u8; 32];
    let mut idx = 0;
    let mut increment = None;
    pack_prefix_part(&mut bytes, &mut idx, &mut increment, first);
    if let Some(second) = second {
        pack_prefix_part(&mut bytes, &mut idx, &mut increment, second);
    }
    if let Some(nibble) = child_nibble {
        pack_prefix_nibble(&mut bytes, &mut idx, &mut increment, nibble);
    }

    let start = B256::from(bytes);
    let end = increment.map(|(increment_idx, nibble)| {
        let mut end = bytes;
        set_packed_nibble(&mut end, increment_idx, nibble + 1);
        for idx in increment_idx + 1..len {
            set_packed_nibble(&mut end, idx, 0);
        }
        B256::from(end)
    });

    Some((start, end))
}

fn pack_nibbles(bytes: &mut [u8; 32], idx: &mut usize, nibbles: &Nibbles) {
    for offset in 0..nibbles.len() {
        set_packed_nibble(bytes, *idx, nibbles.get_unchecked(offset));
        *idx += 1;
    }
}

fn pack_prefix_part(
    bytes: &mut [u8; 32],
    idx: &mut usize,
    increment: &mut Option<(usize, u8)>,
    nibbles: &Nibbles,
) {
    for offset in 0..nibbles.len() {
        pack_prefix_nibble(bytes, idx, increment, nibbles.get_unchecked(offset));
    }
}

fn set_packed_nibble(bytes: &mut [u8; 32], idx: usize, nibble: u8) {
    let byte = &mut bytes[idx / 2];
    if idx % 2 == 0 {
        *byte = (*byte & 0x0f) | (nibble << 4);
    } else {
        *byte = (*byte & 0xf0) | nibble;
    }
}

fn pack_prefix_nibble(
    bytes: &mut [u8; 32],
    idx: &mut usize,
    increment: &mut Option<(usize, u8)>,
    nibble: u8,
) {
    if nibble < 0x0f {
        *increment = Some((*idx, nibble));
    }
    set_packed_nibble(bytes, *idx, nibble);
    *idx += 1;
}

fn next_prefix(prefix: &Nibbles) -> Option<Nibbles> {
    for idx in (0..prefix.len()).rev() {
        let nibble = prefix.get_unchecked(idx);
        if nibble < 0x0f {
            let mut next = Nibbles::new();
            for prefix_idx in 0..idx {
                next.push_unchecked(prefix.get_unchecked(prefix_idx));
            }
            next.push_unchecked(nibble + 1);
            return Some(next)
        }
    }

    None
}

fn key_is_before_end(key: B256, end: Option<B256>) -> bool {
    match end {
        Some(end) => key < end,
        None => true,
    }
}

fn trie_key_is_before_end(key: &Nibbles, end: Option<&Nibbles>) -> bool {
    match end {
        Some(end) => key < end,
        None => true,
    }
}
