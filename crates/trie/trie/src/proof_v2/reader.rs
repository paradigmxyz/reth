use super::{
    node::{ProofNode, ProofNodeKind},
    target::TargetDepths,
    LeafValueEncoder, SubTrieTargets, TRACE_TARGET,
};
use crate::{hashed_cursor::HashedCursor, trie_cursor::TrieCursor};
use alloy_primitives::B256;
use alloy_trie::{BranchNodeCompact, TrieMask};
use reth_execution_errors::trie::StateProofError;
use reth_trie_common::{prefix_set::PrefixSet, Nibbles};
use tracing::trace;

/// Reads one bounded part of the current trie as ordered, disjoint leaves and branch hashes.
///
/// Changed keys can create branches or split extensions outside the cached paths. The reader
/// covers those gaps before it returns later items, so the builder can finish nodes using only
/// the order of the items.
pub(super) struct ProofReader<'a, TC, HC, VE: LeafValueEncoder> {
    trie_cursor: &'a mut TC,
    hashed_cursor: &'a mut HC,
    value_encoder: &'a mut VE,
    prefix_set: &'a mut PrefixSet,
    targets: &'a SubTrieTargets<'a>,
    target_depths: TargetDepths<'a>,
    frames: &'a mut Vec<CachedBranch>,
    cursor_state: &'a mut ProofCursorState<VE::DeferredEncoder>,
    lower_bound: Option<Nibbles>,
    mode: ReadMode,
}

impl<'a, TC, HC, VE> ProofReader<'a, TC, HC, VE>
where
    TC: TrieCursor,
    HC: HashedCursor,
    VE: LeafValueEncoder<Value = HC::Value>,
{
    #[inline]
    pub(super) fn new(
        trie_cursor: &'a mut TC,
        hashed_cursor: &'a mut HC,
        value_encoder: &'a mut VE,
        prefix_set: &'a mut PrefixSet,
        targets: &'a SubTrieTargets<'a>,
        frames: &'a mut Vec<CachedBranch>,
        cursor_state: &'a mut ProofCursorState<VE::DeferredEncoder>,
    ) -> Self {
        if cursor_state.upper_bound.is_none_or(|upper| targets.lower_bound < upper) {
            trie_cursor.reset();
            hashed_cursor.reset();
            cursor_state.trie_entry = CursorEntry::Unseeked;
            cursor_state.leaf_entry = CursorEntry::Unseeked;
        }
        cursor_state.upper_bound = targets.upper_bound;
        frames.clear();
        Self {
            trie_cursor,
            hashed_cursor,
            value_encoder,
            prefix_set,
            targets,
            target_depths: TargetDepths::new(targets.targets),
            frames,
            cursor_state,
            lower_bound: Some(targets.lower_bound),
            mode: ReadMode::Branches,
        }
    }

    #[inline]
    pub(super) fn next(
        &mut self,
    ) -> Result<Option<ProofNode<VE::DeferredEncoder>>, StateProofError> {
        while let Some(lower) = self.lower_bound {
            if let ReadMode::Leaves { upper } = self.mode {
                if self
                    .cursor_state
                    .leaf_entry
                    .get()
                    .is_some_and(|(key, _)| upper.is_none_or(|upper| *key < upper))
                {
                    let (key, value) = self.cursor_state.leaf_entry.take();
                    let next = self.hashed_cursor.next()?;
                    self.cursor_state.leaf_entry = self.encode_leaf(next);
                    let retain_depth = self.target_depths.next(&key);
                    return Ok(Some(ProofNode {
                        path: key,
                        retain_depth,
                        kind: ProofNodeKind::Leaf(value),
                    }))
                }

                self.lower_bound = upper;
                self.mode = ReadMode::Branches;
                if self.cursor_state.leaf_entry.get().is_none_or(|(key, _)| {
                    self.targets.upper_bound.is_some_and(|upper| *key >= upper)
                }) {
                    self.lower_bound = None;
                }
                continue
            }

            let Some(cached) = self.frames.last_mut() else {
                // Each frame lies wholly inside the target range. Its children use local bounds.
                if self.targets.upper_bound.is_some_and(|upper| lower >= upper) {
                    self.lower_bound = None;
                    break
                }
                self.seek_trie(lower)?;
                let Some((path, _)) = self.cursor_state.trie_entry.get() else {
                    self.read_leaves(lower, self.targets.upper_bound)?;
                    continue
                };
                let path = *path;
                if self.targets.upper_bound.is_some_and(|upper| path >= upper) {
                    self.read_leaves(lower, self.targets.upper_bound)?;
                    continue
                }

                self.push_cached_branch(path.len());
                // A top-level gap has no parent mask to describe its leaves.
                if lower < path && !path.is_zeroes() {
                    self.read_leaves(lower, Some(path))?;
                }
                continue
            };

            if lower < cached.path && self.prefix_set.contains_range(&lower..&cached.path) {
                let upper = cached.path;
                self.read_leaves(lower, Some(upper))?;
                continue
            }

            let Some(nibble) = cached.children.first_set_bit_index() else {
                let path = cached.path;
                let upper = path.slice_unchecked(0, cached.range_prefix_len).next_without_prefix();
                self.frames.pop();
                // The cached branch can be below an extension. Its enclosing child range also
                // covers keys after the branch which can split that extension.
                if let Some(branch_end) = path.next_without_prefix() {
                    let tail = lower.max(branch_end);
                    if self.has_changes(tail, upper) {
                        self.read_leaves(tail, upper)?;
                        continue
                    }
                }
                self.lower_bound = upper;
                continue
            };
            cached.children.unset_bit(nibble);

            let mut child_path = cached.path;
            child_path.push_unchecked(nibble);
            debug_assert!(lower <= child_path);
            let child_upper = child_path.next_without_prefix();

            if cached.branch.hash_mask.is_bit_set(nibble) &&
                !cached.reveal_children &&
                !self.prefix_set.contains(&child_path)
            {
                let hash = cached.branch.hash_for_nibble(nibble);
                let retain_depth = self.target_depths.next(&child_path);
                if hash != B256::ZERO && retain_depth < child_path.len() {
                    let stored = cached.branch.tree_mask.is_bit_set(nibble);
                    self.lower_bound = child_upper;
                    trace!(target: TRACE_TARGET, ?child_path, "Reading cached branch hash");
                    return Ok(Some(ProofNode {
                        path: child_path,
                        retain_depth,
                        kind: ProofNodeKind::Hash { hash, stored },
                    }))
                }
            }

            if !cached.branch.tree_mask.is_bit_set(nibble) {
                self.read_leaves(child_path, child_upper)?;
                continue
            }

            self.seek_trie(child_path)?;
            if self
                .cursor_state
                .trie_entry
                .get()
                .is_some_and(|(path, _)| path.starts_with(&child_path))
            {
                self.push_cached_branch(child_path.len());
                self.lower_bound = Some(child_path);
            } else {
                self.read_leaves(child_path, child_upper)?;
            }
        }
        Ok(None)
    }

    fn has_changes(&mut self, lower: Nibbles, upper: Option<Nibbles>) -> bool {
        match upper {
            Some(upper) => lower < upper && self.prefix_set.contains_range(&lower..&upper),
            None => self.prefix_set.contains_from(&lower),
        }
    }

    fn seek_trie(&mut self, lower: Nibbles) -> Result<(), StateProofError> {
        if self.cursor_state.trie_entry.needs_seek(|(path, _)| *path < lower) {
            let entry = self.trie_cursor.seek(lower)?;
            if entry.as_ref().is_some_and(|(path, _)| *path < lower) {
                return Err(StateProofError::TrieInconsistency(format!(
                    "trie cursor returned a path before {lower:?}",
                )))
            }
            self.cursor_state.trie_entry = entry.into();
        }
        Ok(())
    }

    fn push_cached_branch(&mut self, range_prefix_len: usize) {
        let (path, branch) = self.cursor_state.trie_entry.take();
        let mut children = branch.state_mask;
        let reveal_children = if self.prefix_set.contains(&path) {
            let mut unchanged = children;
            for nibble in 0..16 {
                let mut child_path = path;
                child_path.push_unchecked(nibble);
                if self.prefix_set.contains(&child_path) {
                    children.set_bit(nibble);
                    unchanged.unset_bit(nibble);
                }
            }
            // A sole surviving child must be available when deletions collapse this branch.
            unchanged.count_ones() < 2
        } else {
            false
        };
        self.frames.push(CachedBranch {
            path,
            branch,
            children,
            range_prefix_len,
            reveal_children,
        });
    }

    #[inline]
    fn read_leaves(
        &mut self,
        lower: Nibbles,
        upper: Option<Nibbles>,
    ) -> Result<(), StateProofError> {
        debug_assert!(upper.is_none_or(|upper| lower < upper));
        if self.cursor_state.leaf_entry.needs_seek(|(key, _)| *key < lower) {
            let key = B256::right_padding_from(&lower.pack());
            let entry = self.hashed_cursor.seek(key)?;
            self.cursor_state.leaf_entry = self.encode_leaf(entry);
        }
        self.lower_bound = Some(lower);
        self.mode = ReadMode::Leaves { upper };
        Ok(())
    }

    fn encode_leaf(
        &mut self,
        entry: Option<(B256, HC::Value)>,
    ) -> CursorEntry<(Nibbles, VE::DeferredEncoder)> {
        entry
            .map(|(key, value)| {
                (Nibbles::unpack(key), self.value_encoder.deferred_encoder(key, value))
            })
            .into()
    }
}

/// A stored branch and the whole child range in which it was found.
#[derive(Debug)]
pub(super) struct CachedBranch {
    path: Nibbles,
    branch: BranchNodeCompact,
    children: TrieMask,
    /// The enclosing child prefix can be shorter than this branch's path across an extension.
    range_prefix_len: usize,
    reveal_children: bool,
}

/// Cursor lookahead survives between disjoint ranges. Overlapping ranges restart both cursors.
pub(super) struct ProofCursorState<D> {
    upper_bound: Option<Nibbles>,
    trie_entry: CursorEntry<(Nibbles, BranchNodeCompact)>,
    leaf_entry: CursorEntry<(Nibbles, D)>,
}

impl<D> Default for ProofCursorState<D> {
    fn default() -> Self {
        Self {
            upper_bound: None,
            trie_entry: CursorEntry::Unseeked,
            leaf_entry: CursorEntry::Unseeked,
        }
    }
}

#[derive(Clone, Copy)]
enum ReadMode {
    Branches,
    Leaves { upper: Option<Nibbles> },
}

enum CursorEntry<T> {
    Unseeked,
    Available(T),
    Exhausted,
}

impl<T> From<Option<T>> for CursorEntry<T> {
    fn from(value: Option<T>) -> Self {
        value.map_or(Self::Exhausted, Self::Available)
    }
}

impl<T> CursorEntry<T> {
    const fn get(&self) -> Option<&T> {
        match self {
            Self::Available(entry) => Some(entry),
            Self::Unseeked | Self::Exhausted => None,
        }
    }

    fn needs_seek(&self, before: impl FnOnce(&T) -> bool) -> bool {
        match self {
            Self::Unseeked => true,
            Self::Available(entry) => before(entry),
            Self::Exhausted => false,
        }
    }

    fn take(&mut self) -> T {
        let Self::Available(entry) = core::mem::replace(self, Self::Unseeked) else {
            panic!("cursor entry is unavailable")
        };
        entry
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        hashed_cursor::HashedCursorFactory,
        proof_v2::{iter_sub_trie_targets, StorageValueEncoder},
        test_utils::TrieTestHarness,
        trie_cursor::TrieCursorFactory,
    };
    use alloy_primitives::U256;
    use reth_trie_common::{ProofV2Target, ProofV2TargetParent};

    #[test]
    fn reader_yields_disjoint_hashes_and_target_leaves() {
        let key = |prefix, child| B256::right_padding_from(&[prefix, child]);
        let storage = [0x20, 0x24, 0x28]
            .into_iter()
            .flat_map(|prefix| [0x00, 0x10].map(|child| (key(prefix, child), U256::from(1))))
            .collect();
        let harness = TrieTestHarness::new(storage);
        let mut trie_cursor =
            harness.trie_cursor_factory().storage_trie_cursor(harness.hashed_address()).unwrap();
        let mut hashed_cursor = harness
            .hashed_cursor_factory()
            .hashed_storage_cursor(harness.hashed_address())
            .unwrap();
        let mut encoder = StorageValueEncoder;
        let mut prefix_set = PrefixSet::default();
        let mut frames = Vec::new();
        let mut cursor_state = ProofCursorState::default();
        let mut targets =
            [ProofV2Target::new(key(0x24, 0x10)).with_parent(ProofV2TargetParent::new(0))];
        let group = iter_sub_trie_targets(&mut targets).next().unwrap();
        let mut reader = ProofReader::new(
            &mut trie_cursor,
            &mut hashed_cursor,
            &mut encoder,
            &mut prefix_set,
            &group,
            &mut frames,
            &mut cursor_state,
        );
        let mut items = Vec::new();
        while let Some(item) = reader.next().unwrap() {
            items.push(match item.kind {
                ProofNodeKind::Leaf(_) => ("leaf", item.path),
                ProofNodeKind::Hash { .. } => ("hash", item.path),
                ProofNodeKind::Branch { .. } => unreachable!(),
            });
        }
        assert_eq!(
            items,
            vec![
                ("hash", Nibbles::from_nibbles([2, 0])),
                ("leaf", Nibbles::unpack(key(0x24, 0x00))),
                ("leaf", Nibbles::unpack(key(0x24, 0x10))),
                ("hash", Nibbles::from_nibbles([2, 8])),
            ]
        );
        assert!(reader.next().unwrap().is_none());
    }

    #[test]
    fn reader_skips_untargeted_children_of_known_parent() {
        let key = |prefix, child| B256::right_padding_from(&[prefix, child]);
        let storage = [0x20, 0x24, 0x28]
            .into_iter()
            .flat_map(|prefix| [0x00, 0x10].map(|child| (key(prefix, child), U256::from(1))))
            .collect();
        let harness = TrieTestHarness::new(storage);
        let address = harness.hashed_address();
        let mut trie_cursor = harness.trie_cursor_factory().storage_trie_cursor(address).unwrap();
        let mut hashed_cursor =
            harness.hashed_cursor_factory().hashed_storage_cursor(address).unwrap();
        let mut encoder = StorageValueEncoder;
        let mut prefix_set = PrefixSet::default();
        let mut frames = Vec::new();
        let mut cursor_state = ProofCursorState::default();
        let mut targets = [0x20, 0x28].map(|prefix| {
            ProofV2Target::new(key(prefix, 0x10)).with_parent(ProofV2TargetParent::new(1))
        });
        let mut paths = Vec::new();
        for group in iter_sub_trie_targets(&mut targets) {
            let mut reader = ProofReader::new(
                &mut trie_cursor,
                &mut hashed_cursor,
                &mut encoder,
                &mut prefix_set,
                &group,
                &mut frames,
                &mut cursor_state,
            );
            while let Some(item) = reader.next().unwrap() {
                paths.push(item.path);
            }
        }
        assert_eq!(
            paths,
            [key(0x20, 0x00), key(0x20, 0x10), key(0x28, 0x00), key(0x28, 0x10)]
                .map(Nibbles::unpack),
        );
    }
}
