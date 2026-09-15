//! Leaf updates that a sparse trie could not apply because they hit a blinded node.

use crate::LeafUpdate;
use alloc::vec::Vec;
use alloy_primitives::{map::B256Map, B256};
use core::mem;
use reth_trie_common::Nibbles;

/// Leaf updates that hit a blinded node and wait for that node to be revealed.
///
/// A trie takes ownership of the updates it could not apply, so that later batches only have to
/// look at the entries whose blinded node was actually revealed. Entries are kept sorted by key,
/// which for the full-length hashed keys of the state tries is the same order as their unpacked
/// nibble paths.
#[derive(Debug, Clone, Default)]
pub struct BlockedLeafUpdates {
    /// Blocked entries, sorted by key.
    entries: Vec<BlockedLeafUpdate>,
    /// Buffer reused while rebuilding [`Self::entries`].
    scratch: Vec<BlockedLeafUpdate>,
    /// Keys of the [`BlockedOn::Sibling`] entries by the path of the node they wait for, sorted
    /// by that path. Their node is not on their own key path, so they cannot be found by it.
    siblings: Vec<(Nibbles, B256)>,
    /// Number of entries in [`Self::entries`] that are ready to be applied again.
    num_retryable: usize,
}

impl BlockedLeafUpdates {
    /// Creates an empty set of blocked updates.
    pub const fn new() -> Self {
        Self { entries: Vec::new(), scratch: Vec::new(), siblings: Vec::new(), num_retryable: 0 }
    }

    /// Returns the number of blocked updates.
    pub const fn len(&self) -> usize {
        self.entries.len()
    }

    /// Returns `true` if no update is blocked.
    pub const fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Returns `true` if at least one blocked update can be applied again.
    pub const fn has_retryable(&self) -> bool {
        self.num_retryable > 0
    }

    /// Returns the blocked update for the given key, if there is one.
    pub fn get(&self, key: &B256) -> Option<&LeafUpdate> {
        self.position(key).map(|pos| &self.entries[pos].update)
    }

    /// Returns the keys of all blocked updates.
    pub fn keys(&self) -> impl Iterator<Item = B256> + '_ {
        self.entries.iter().map(|entry| entry.key)
    }

    /// Drops all blocked updates, keeping the allocations.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.scratch.clear();
        self.siblings.clear();
        self.num_retryable = 0;
    }

    /// Marks every entry waiting for a node at `path` as retryable.
    pub(crate) fn mark_revealed(&mut self, path: &Nibbles) {
        // No entry can wait on the root, and entries waiting on a deeper node are only reachable
        // once their own blinded node was revealed.
        if self.entries.is_empty() || path.is_empty() {
            return
        }

        let waiting_on = BlockedOn::Reveal(path.len() as u8);
        let (lower, upper) = prefix_range(path);
        let start = self.entries.partition_point(|entry| entry.key < lower);
        for entry in &mut self.entries[start..] {
            if entry.key > upper {
                break
            }
            if entry.blocked_on == waiting_on {
                entry.blocked_on = BlockedOn::Retryable;
                self.num_retryable += 1;
            }
        }

        if !self.siblings.is_empty() {
            self.mark_sibling_revealed(path);
        }
    }

    /// Marks the entries whose branch collapse waits for the node at `path` as retryable.
    fn mark_sibling_revealed(&mut self, path: &Nibbles) {
        let start = self.siblings.partition_point(|(sibling, _)| sibling < path);
        let mut end = start;
        while self.siblings.get(end).is_some_and(|(sibling, _)| sibling == path) {
            end += 1;
        }

        for index in start..end {
            let key = self.siblings[index].1;
            let Some(pos) = self.position(&key) else { continue };
            let entry = &mut self.entries[pos];
            if matches!(entry.blocked_on, BlockedOn::Sibling { .. }) {
                entry.blocked_on = BlockedOn::Retryable;
                self.num_retryable += 1;
            }
        }

        self.siblings.drain(start..end);
    }

    /// Moves `updates` and all retryable entries into `batch`, sorted by key and with their
    /// nibble path unpacked.
    ///
    /// An update for a key that is already blocked replaces the blocked update in place instead
    /// of being applied: the blinded node the entry stopped at does not depend on the update
    /// value, so the entry keeps waiting for the same node. A [`LeafUpdate::Touched`] never
    /// replaces a known value.
    pub(crate) fn take_batch(
        &mut self,
        updates: &mut B256Map<LeafUpdate>,
        batch: &mut Vec<(B256, Nibbles, LeafUpdate)>,
    ) {
        batch.clear();
        batch.reserve(updates.len() + self.num_retryable);

        for (key, update) in updates.drain() {
            if let Some(pos) = self.position(&key) {
                if update.is_changed() {
                    self.entries[pos].update = update;
                }
                continue
            }
            batch.push((key, Nibbles::default(), update));
        }

        if self.num_retryable > 0 {
            let mut entries = mem::take(&mut self.entries);
            let mut kept = mem::take(&mut self.scratch);
            kept.clear();
            #[expect(clippy::iter_with_drain, reason = "retain the scratch buffer allocation")]
            for entry in entries.drain(..) {
                match entry.blocked_on {
                    BlockedOn::Retryable => {
                        batch.push((entry.key, Nibbles::default(), entry.update))
                    }
                    BlockedOn::Reveal(_) | BlockedOn::Sibling { .. } => kept.push(entry),
                }
            }
            self.entries = kept;
            self.scratch = entries;
            self.num_retryable = 0;
        }

        batch.sort_unstable_by_key(|&(key, ..)| key);
        for (key, path, _) in batch.iter_mut() {
            *path = Nibbles::unpack(key);
        }
    }

    /// Blocks the entries of `batch` at the given indices, taking their update out of the batch.
    ///
    /// `indices` does not need to be sorted and may contain the same index twice, because a
    /// removal can require two proofs.
    pub(crate) fn block(
        &mut self,
        batch: &mut [(B256, Nibbles, LeafUpdate)],
        indices: &mut [(u32, BlockedOn)],
    ) {
        if indices.is_empty() {
            return
        }
        indices.sort_unstable_by_key(|(index, _)| *index);

        // The batch is sorted by key, so walking it by index merges into the sorted entries.
        let mut entries = mem::take(&mut self.entries);
        let mut merged = mem::take(&mut self.scratch);
        merged.clear();
        merged.reserve(entries.len() + indices.len());

        #[expect(clippy::iter_with_drain, reason = "retain the scratch buffer allocation")]
        let mut blocked = entries.drain(..).peekable();
        let mut last_index = None;
        let num_siblings = self.siblings.len();
        for &(index, blocked_on) in indices.iter() {
            if last_index == Some(index) {
                continue
            }
            last_index = Some(index);

            let (key, path, update) = &mut batch[index as usize];
            let key = *key;
            let path = *path;
            while blocked.peek().is_some_and(|entry| entry.key < key) {
                merged.push(blocked.next().expect("peeked"));
            }
            debug_assert!(
                blocked.peek().is_none_or(|entry| entry.key != key),
                "leaf update blocked twice for the same key",
            );

            match blocked_on {
                BlockedOn::Retryable => self.num_retryable += 1,
                // The sibling is not on this key's path, so index the entry by it.
                BlockedOn::Sibling { depth, nibble } => {
                    let mut sibling = path.slice(..depth as usize - 1);
                    sibling.push_unchecked(nibble);
                    self.siblings.push((sibling, key));
                }
                BlockedOn::Reveal(_) => {}
            }
            merged.push(BlockedLeafUpdate {
                key,
                update: mem::replace(update, LeafUpdate::Touched),
                blocked_on,
            });
        }
        merged.extend(blocked);

        self.entries = merged;
        self.scratch = entries;
        if self.siblings.len() != num_siblings {
            self.siblings.sort_unstable();
        }
    }

    /// Returns the position of the given key in [`Self::entries`].
    fn position(&self, key: &B256) -> Option<usize> {
        if self.entries.is_empty() {
            return None
        }
        self.entries.binary_search_by(|entry| entry.key.cmp(key)).ok()
    }
}

/// A leaf update waiting for a blinded node to be revealed.
#[derive(Debug, Clone)]
struct BlockedLeafUpdate {
    /// Full hashed key of the leaf.
    key: B256,
    /// The update to apply once the trie is revealed deeply enough.
    update: LeafUpdate,
    /// What this entry is waiting for.
    blocked_on: BlockedOn,
}

/// Describes when a [`BlockedLeafUpdate`] should be applied again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BlockedOn {
    /// A node covering the first N nibbles of the entry's key must be revealed.
    Reveal(u8),
    /// The blinded sibling a branch collapse needs must be revealed. It sits at `depth`, where
    /// it shares the first `depth - 1` nibbles of the entry's key and continues with `nibble`.
    Sibling { depth: u8, nibble: u8 },
    /// The entry can be applied again with the next batch.
    Retryable,
}

impl BlockedOn {
    /// Waits for the blinded sibling at the given path.
    pub(crate) fn sibling(path: &Nibbles) -> Self {
        Self::Sibling {
            depth: path.len() as u8,
            nibble: path.last().expect("sibling path has a child nibble"),
        }
    }
}

/// Returns the lowest and highest key that start with the given path.
fn prefix_range(path: &Nibbles) -> (B256, B256) {
    let mut lower = [0u8; 32];
    path.pack_to(&mut lower);

    let mut upper = lower;
    let len = path.len();
    if len % 2 == 1 {
        upper[len / 2] |= 0x0f;
    }
    for byte in &mut upper[len.div_ceil(2)..] {
        *byte = 0xff;
    }

    (lower.into(), upper.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(byte: u8) -> B256 {
        B256::repeat_byte(byte)
    }

    #[test]
    fn prefix_range_covers_all_keys_with_prefix() {
        let (lower, upper) = prefix_range(&Nibbles::from_nibbles([0x1, 0x2, 0x3]));
        assert_eq!(lower, B256::from_slice(&[[0x12, 0x30].as_slice(), &[0u8; 30]].concat()));
        assert_eq!(upper, B256::from_slice(&[[0x12, 0x3f].as_slice(), &[0xffu8; 30]].concat()));
    }

    #[test]
    fn only_entries_waiting_on_the_revealed_path_become_retryable() {
        let mut blocked = BlockedLeafUpdates::new();
        let mut batch = vec![
            (key(0x11), Nibbles::unpack(key(0x11)), LeafUpdate::Touched),
            (key(0x12), Nibbles::unpack(key(0x12)), LeafUpdate::Changed(vec![1])),
            (key(0x22), Nibbles::unpack(key(0x22)), LeafUpdate::Touched),
        ];
        blocked.block(
            &mut batch,
            &mut [(0, BlockedOn::Reveal(2)), (1, BlockedOn::Reveal(2)), (2, BlockedOn::Retryable)],
        );
        assert_eq!(blocked.len(), 3);
        assert!(blocked.has_retryable());

        // The retryable entry is taken even though nothing was revealed for it.
        let mut updates = B256Map::default();
        blocked.take_batch(&mut updates, &mut batch);
        assert_eq!(batch.len(), 1);
        assert_eq!(batch[0].0, key(0x22));
        assert_eq!(blocked.len(), 2);

        // Revealing an unrelated path leaves both entries blocked.
        blocked.mark_revealed(&Nibbles::from_nibbles([0x2, 0x2]));
        assert!(!blocked.has_retryable());

        blocked.mark_revealed(&Nibbles::from_nibbles([0x1, 0x1]));
        blocked.take_batch(&mut updates, &mut batch);
        assert_eq!(batch.len(), 1);
        assert_eq!(batch[0].0, key(0x11));
        assert_eq!(blocked.len(), 1);
    }

    #[test]
    fn sibling_entries_wait_for_the_node_they_collapse_onto() {
        let sibling = Nibbles::from_nibbles([0x1, 0x2]);
        let mut blocked = BlockedLeafUpdates::new();
        let mut batch =
            vec![(key(0x11), Nibbles::unpack(key(0x11)), LeafUpdate::Changed(Vec::new()))];
        blocked.block(&mut batch, &mut [(0, BlockedOn::sibling(&sibling))]);
        assert!(!blocked.has_retryable());

        // Revealing the entry's own path does not unblock the collapse.
        blocked.mark_revealed(&Nibbles::from_nibbles([0x1, 0x1]));
        assert!(!blocked.has_retryable());

        blocked.mark_revealed(&sibling);
        assert!(blocked.has_retryable());

        let mut updates = B256Map::default();
        blocked.take_batch(&mut updates, &mut batch);
        assert_eq!(batch.len(), 1);
        assert_eq!(batch[0].0, key(0x11));
        assert!(blocked.is_empty());
    }

    #[test]
    fn newer_changed_update_replaces_the_blocked_one() {
        let mut blocked = BlockedLeafUpdates::new();
        let mut batch = vec![(key(0x11), Nibbles::unpack(key(0x11)), LeafUpdate::Changed(vec![1]))];
        blocked.block(&mut batch, &mut [(0, BlockedOn::Reveal(2))]);

        let mut updates = B256Map::from_iter([(key(0x11), LeafUpdate::Touched)]);
        blocked.take_batch(&mut updates, &mut batch);
        assert!(batch.is_empty(), "a touch must not be applied ahead of the blocked update");
        assert_eq!(blocked.get(&key(0x11)), Some(&LeafUpdate::Changed(vec![1])));

        let mut updates = B256Map::from_iter([(key(0x11), LeafUpdate::Changed(vec![2]))]);
        blocked.take_batch(&mut updates, &mut batch);
        assert!(batch.is_empty(), "the entry stays blocked on the same node");
        assert_eq!(blocked.get(&key(0x11)), Some(&LeafUpdate::Changed(vec![2])));

        blocked.mark_revealed(&Nibbles::from_nibbles([0x1, 0x1]));
        blocked.take_batch(&mut updates, &mut batch);
        assert_eq!(batch.len(), 1);
        assert_eq!(batch[0].2, LeafUpdate::Changed(vec![2]));
        assert!(blocked.is_empty());
    }
}
