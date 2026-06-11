use super::{
    branch_child_idx::{BranchChildIdx, BranchChildIter},
    ArenaParallelSparseTrie, ArenaSparseNode, ArenaSparseNodeBranchChild, ArenaSparseNodeState,
    Index, NodeArena,
};
use crate::{HashedCursor, HashedStorageCursor};
use alloc::{format, vec::Vec};
use alloy_primitives::{B256, U256};
use alloy_rlp::Decodable;
use core::{cmp::Ordering, fmt::Debug, marker::PhantomData};
use reth_primitives_traits::Account;
use reth_storage_errors::db::DatabaseError;
use reth_trie_common::{Nibbles, TrieAccount};
use tracing::{instrument, trace};

const TRACE_TARGET: &str = "trie::arena::cursor";

/// An entry on the cursor's traversal stack, tracking an ancestor node during trie walks.
#[derive(Debug, Clone)]
pub(super) struct ArenaCursorStackEntry {
    /// The arena index of this node.
    pub(super) index: Index,
    /// The absolute path of this node in the trie (not including its `short_key`).
    pub(super) path: Nibbles,
    /// The dense index at which to resume child iteration in [`ArenaCursor::next`].
    /// Only meaningful when this entry's node is a branch.
    pub(super) next_dense_idx: usize,
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
        self.stack.push(ArenaCursorStackEntry { index: idx, path, next_dense_idx: 0 });
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
                ArenaSparseNode::Branch(b) => matches!(b.state, ArenaSparseNodeState::Dirty),
                ArenaSparseNode::Leaf { state, .. } => matches!(state, ArenaSparseNodeState::Dirty),
                ArenaSparseNode::Subtrie(s) => {
                    let root = &s.arena[s.root];
                    matches!(root, ArenaSparseNode::EmptyRoot) ||
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
            let start = head.next_dense_idx;
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
                        branch_child_idx.get() + 1;
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

/// Hashed-state cursor backed by an arena sparse trie and an inner hashed cursor.
///
/// Revealed sparse-trie leaves are returned directly. Revealed sparse-trie topology also proves
/// gaps are empty, so the inner cursor is only queried for explicit blinded child ranges.
#[derive(Debug)]
pub struct ArenaHashedCursor<'a, C, V> {
    items: Vec<ArenaCursorItem<'a>>,
    idx: usize,
    active_blind_end: Option<Option<B256>>,
    last_key: Option<B256>,
    inner: C,
    _marker: PhantomData<V>,
}

impl<'a, C, V> ArenaHashedCursor<'a, C, V>
where
    C: HashedCursor,
{
    /// Creates a new cursor for `trie`, delegating blinded ranges to `inner`.
    pub fn new(trie: Option<&'a ArenaParallelSparseTrie>, inner: C) -> Self {
        let mut this = Self {
            items: Vec::new(),
            idx: 0,
            active_blind_end: None,
            last_key: None,
            inner,
            _marker: PhantomData,
        };
        this.set_sparse_trie(trie);
        this
    }

    /// Updates the sparse trie topology used by this cursor.
    ///
    /// `None` means the trie is completely unrevealed, so the full keyspace is delegated to the
    /// inner cursor.
    pub fn set_sparse_trie(&mut self, trie: Option<&'a ArenaParallelSparseTrie>) {
        self.items.clear();
        match trie {
            Some(trie) => collect_trie_items(trie, &mut self.items),
            None => self.items.push(ArenaCursorItem::Blind { start: B256::ZERO, end: None }),
        }
        self.items.sort_unstable_by(compare_items);
        self.idx = 0;
        self.active_blind_end = None;
        self.last_key = None;
        self.inner.reset();
    }

    /// Returns a mutable reference to the inner cursor.
    pub const fn inner_mut(&mut self) -> &mut C {
        &mut self.inner
    }

    fn first_item_idx(&self, key: B256) -> usize {
        self.items
            .iter()
            .position(|item| item.may_contain_key_at_or_after(key))
            .unwrap_or(self.items.len())
    }
}

impl<C, V> ArenaHashedCursor<'_, C, V>
where
    C: HashedCursor<Value = V>,
    V: ArenaHashedCursorValue,
{
    fn next_inner_in_active_range(&mut self) -> Result<Option<(B256, V)>, DatabaseError> {
        let Some(end) = self.active_blind_end else { return Ok(None) };

        let mut entry = self.inner.next()?;
        while let Some((key, value)) = entry {
            if !key_is_before_end(key, end) {
                self.active_blind_end = None;
                self.idx += 1;
                return self.next_from_items(None)
            }
            if self.last_key.is_none_or(|last_key| key > last_key) {
                self.last_key = Some(key);
                return Ok(Some((key, value)))
            }
            entry = self.inner.next()?;
        }

        self.active_blind_end = None;
        self.idx += 1;
        self.next_from_items(None)
    }

    fn next_from_items(
        &mut self,
        seek_key: Option<B256>,
    ) -> Result<Option<(B256, V)>, DatabaseError> {
        while let Some(item) = self.items.get(self.idx) {
            match *item {
                ArenaCursorItem::Leaf { key, value } => {
                    self.idx += 1;
                    if seek_key.is_none_or(|seek_key| key >= seek_key) &&
                        self.last_key.is_none_or(|last_key| key > last_key)
                    {
                        let value = V::decode(value)?;
                        self.last_key = Some(key);
                        return Ok(Some((key, value)))
                    }
                }
                ArenaCursorItem::Blind { start, end } => {
                    let inner_seek_key = seek_key.map_or(start, |seek_key| start.max(seek_key));
                    let mut entry = self.inner.seek(inner_seek_key)?;
                    while let Some((key, value)) = entry {
                        if !key_is_before_end(key, end) {
                            break
                        }
                        if self.last_key.is_none_or(|last_key| key > last_key) {
                            self.active_blind_end = Some(end);
                            self.last_key = Some(key);
                            return Ok(Some((key, value)))
                        }
                        entry = self.inner.next()?;
                    }

                    self.idx += 1;
                    self.active_blind_end = None;
                }
            }
        }

        Ok(None)
    }
}

impl<C, V> HashedCursor for ArenaHashedCursor<'_, C, V>
where
    C: HashedCursor<Value = V>,
    V: ArenaHashedCursorValue,
{
    type Value = V;

    fn seek(&mut self, key: B256) -> Result<Option<(B256, Self::Value)>, DatabaseError> {
        self.idx = self.first_item_idx(key);
        self.active_blind_end = None;
        self.last_key = None;
        self.next_from_items(Some(key))
    }

    fn next(&mut self) -> Result<Option<(B256, Self::Value)>, DatabaseError> {
        if self.active_blind_end.is_some() {
            self.next_inner_in_active_range()
        } else {
            self.next_from_items(None)
        }
    }

    fn reset(&mut self) {
        self.idx = 0;
        self.active_blind_end = None;
        self.last_key = None;
        self.inner.reset();
    }
}

/// Hashed storage cursor backed by an arena sparse trie and an inner hashed storage cursor.
#[derive(Debug)]
pub struct ArenaHashedStorageCursor<'a, C> {
    cursor: ArenaHashedCursor<'a, C, U256>,
}

impl<'a, C> ArenaHashedStorageCursor<'a, C>
where
    C: HashedStorageCursor<Value = U256>,
{
    /// Creates a new storage cursor for `trie`, delegating blinded ranges to `inner`.
    pub fn new(trie: Option<&'a ArenaParallelSparseTrie>, inner: C) -> Self {
        Self { cursor: ArenaHashedCursor::new(trie, inner) }
    }

    /// Updates the sparse trie topology used by this cursor.
    pub fn set_sparse_trie(&mut self, trie: Option<&'a ArenaParallelSparseTrie>) {
        self.cursor.set_sparse_trie(trie);
    }

    /// Sets the hashed address on the inner cursor and updates the sparse trie topology.
    pub fn set_hashed_address_with_sparse_trie(
        &mut self,
        hashed_address: B256,
        trie: Option<&'a ArenaParallelSparseTrie>,
    ) {
        self.cursor.inner_mut().set_hashed_address(hashed_address);
        self.cursor.set_sparse_trie(trie);
    }
}

impl<C> HashedCursor for ArenaHashedStorageCursor<'_, C>
where
    C: HashedStorageCursor<Value = U256>,
{
    type Value = U256;

    fn seek(&mut self, key: B256) -> Result<Option<(B256, Self::Value)>, DatabaseError> {
        self.cursor.seek(key)
    }

    fn next(&mut self) -> Result<Option<(B256, Self::Value)>, DatabaseError> {
        self.cursor.next()
    }

    fn reset(&mut self) {
        self.cursor.reset();
    }
}

impl<C> HashedStorageCursor for ArenaHashedStorageCursor<'_, C>
where
    C: HashedStorageCursor<Value = U256>,
{
    fn is_storage_empty(&mut self) -> Result<bool, DatabaseError> {
        let empty = self.seek(B256::ZERO)?.is_none();
        self.reset();
        Ok(empty)
    }

    fn set_hashed_address(&mut self, hashed_address: B256) {
        self.cursor.inner_mut().set_hashed_address(hashed_address);
        self.cursor.set_sparse_trie(None);
    }
}

/// Value decoder for sparse trie leaf values returned by [`ArenaHashedCursor`].
pub trait ArenaHashedCursorValue: Debug {
    /// Decodes an RLP-encoded sparse trie leaf value into the hashed cursor value.
    fn decode(value: &[u8]) -> Result<Self, DatabaseError>
    where
        Self: Sized;
}

impl ArenaHashedCursorValue for Account {
    fn decode(value: &[u8]) -> Result<Self, DatabaseError> {
        let mut value = value;
        let trie_account = TrieAccount::decode(&mut value).map_err(|error| {
            DatabaseError::Other(format!("failed to decode trie account: {error}"))
        })?;
        Ok(trie_account.into())
    }
}

impl ArenaHashedCursorValue for U256 {
    fn decode(value: &[u8]) -> Result<Self, DatabaseError> {
        let mut value = value;
        <U256 as Decodable>::decode(&mut value).map_err(|error| {
            DatabaseError::Other(format!("failed to decode storage value: {error}"))
        })
    }
}

#[derive(Debug, Clone, Copy)]
enum ArenaCursorItem<'a> {
    Leaf { key: B256, value: &'a [u8] },
    Blind { start: B256, end: Option<B256> },
}

impl ArenaCursorItem<'_> {
    fn start_key(&self) -> B256 {
        match *self {
            Self::Leaf { key, .. } => key,
            Self::Blind { start, .. } => start,
        }
    }

    fn may_contain_key_at_or_after(&self, key: B256) -> bool {
        match *self {
            Self::Leaf { key: leaf_key, .. } => leaf_key >= key,
            Self::Blind { end, .. } => match end {
                Some(end) => end > key,
                None => true,
            },
        }
    }
}

fn compare_items(a: &ArenaCursorItem<'_>, b: &ArenaCursorItem<'_>) -> Ordering {
    a.start_key().cmp(&b.start_key()).then_with(|| match (a, b) {
        (ArenaCursorItem::Leaf { .. }, ArenaCursorItem::Blind { .. }) => Ordering::Greater,
        (ArenaCursorItem::Blind { .. }, ArenaCursorItem::Leaf { .. }) => Ordering::Less,
        _ => Ordering::Equal,
    })
}

fn collect_trie_items<'a>(trie: &'a ArenaParallelSparseTrie, items: &mut Vec<ArenaCursorItem<'a>>) {
    collect_arena_items(&trie.upper_arena, trie.root, Nibbles::default(), items);
}

fn collect_arena_items<'a>(
    arena: &'a NodeArena,
    idx: Index,
    path: Nibbles,
    items: &mut Vec<ArenaCursorItem<'a>>,
) {
    match &arena[idx] {
        ArenaSparseNode::EmptyRoot => {}
        ArenaSparseNode::Leaf { value, key, .. } => {
            let mut full_path = path;
            full_path.extend(key);
            debug_assert_eq!(
                full_path.len(),
                B256::len_bytes() * 2,
                "leaf path must contain a full hashed key"
            );
            if full_path.len() == B256::len_bytes() * 2 {
                items.push(ArenaCursorItem::Leaf {
                    key: B256::from_slice(&full_path.pack()),
                    value,
                });
            }
        }
        ArenaSparseNode::Branch(branch) => {
            let mut logical_path = path;
            logical_path.extend(&branch.short_key);
            for (nibble, child) in branch.child_iter() {
                let mut child_path = logical_path;
                child_path.push_unchecked(nibble);
                match child {
                    ArenaSparseNodeBranchChild::Revealed(child_idx) => {
                        collect_arena_items(arena, *child_idx, child_path, items);
                    }
                    ArenaSparseNodeBranchChild::Blinded(_) => {
                        push_blind_range(child_path, items);
                    }
                }
            }
        }
        ArenaSparseNode::Subtrie(subtrie) => {
            collect_arena_items(&subtrie.arena, subtrie.root, subtrie.path, items);
        }
        ArenaSparseNode::TakenSubtrie => {
            push_blind_range(path, items);
        }
    }
}

fn push_blind_range(path: Nibbles, items: &mut Vec<ArenaCursorItem<'_>>) {
    if path.len() > B256::len_bytes() * 2 {
        return
    }

    items.push(ArenaCursorItem::Blind { start: prefix_start(&path), end: prefix_end(&path) });
}

fn prefix_start(prefix: &Nibbles) -> B256 {
    B256::right_padding_from(&prefix.pack())
}

fn prefix_end(prefix: &Nibbles) -> Option<B256> {
    let mut nibbles = Vec::with_capacity(prefix.len());
    for idx in 0..prefix.len() {
        nibbles.push(prefix.get_unchecked(idx));
    }

    while let Some(nibble) = nibbles.pop() {
        if nibble < 0x0f {
            nibbles.push(nibble + 1);
            let next_prefix = Nibbles::from_nibbles_unchecked(nibbles);
            return Some(prefix_start(&next_prefix))
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
