//! Flattened trie overlays for in-memory blocks.
//!
//! Payload validation needs a view of the trie as of an in-memory parent block even when that
//! parent has not been persisted yet. [`TrieOverlayManager`] tracks those in-memory blocks and
//! builds reusable flattened overlays on demand.

use crate::{EthPrimitives, ExecutedBlock};
use alloy_primitives::{map::B256Map, B256};
use parking_lot::RwLock;
use reth_primitives_traits::{AlloyBlockHeader, NodePrimitives};
use reth_trie::{updates::TrieUpdatesSorted, HashedPostStateSorted, TrieInputSorted};
use std::sync::Arc;
use tracing::trace;

/// Manages flattened trie overlays for in-memory blocks.
///
/// The manager keeps a linked overlay for every inserted block. Each overlay points at its parent
/// overlay when the parent is also in memory, so the common "next block on the same chain" case can
/// flatten by extending the parent overlay instead of walking every ancestor again.
#[derive(Clone)]
pub struct TrieOverlayManager<N: NodePrimitives = EthPrimitives> {
    inner: Arc<RwLock<TrieOverlayManagerInner<N>>>,
}

impl<N: NodePrimitives> Default for TrieOverlayManager<N> {
    fn default() -> Self {
        Self { inner: Default::default() }
    }
}

impl<N: NodePrimitives> std::fmt::Debug for TrieOverlayManager<N> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let inner = self.inner.read();
        f.debug_struct("TrieOverlayManager")
            .field("blocks", &inner.blocks.len())
            .field("overlays", &inner.overlays.len())
            .field("dirty", &inner.dirty)
            .finish()
    }
}

impl<N: NodePrimitives> TrieOverlayManager<N> {
    /// Inserts an executed in-memory block into the overlay manager.
    pub fn insert_block(&self, block: ExecutedBlock<N>) {
        let hash = block.recovered_block().hash();
        let parent_hash = block.recovered_block().parent_hash();

        let mut inner = self.inner.write();
        if inner.blocks.contains_key(&hash) {
            return;
        }

        let parent = (!inner.dirty).then(|| inner.overlays.get(&parent_hash).cloned()).flatten();
        let overlay = TrieOverlay::new(block.clone(), parent);

        inner.blocks.insert(hash, block);
        if !inner.dirty {
            inner.overlays.insert(hash, overlay);
        }
    }

    /// Removes a block from the manager.
    ///
    /// Existing overlay handles remain usable because they own the data they need through `Arc`s.
    /// New lookups rebuild the linked overlays from the remaining block set.
    pub fn remove_block(&self, hash: B256) {
        let mut inner = self.inner.write();
        if inner.blocks.remove(&hash).is_some() {
            inner.overlays.remove(&hash);
            inner.dirty = true;
        }
    }

    /// Removes all tracked blocks and overlays.
    pub fn clear(&self) {
        let mut inner = self.inner.write();
        inner.blocks.clear();
        inner.overlays.clear();
        inner.dirty = false;
    }

    /// Returns an overlay for the requested parent hash and the persisted anchor hash it sits on.
    ///
    /// If the parent is not in memory, no overlay is required and the parent itself is returned as
    /// the anchor.
    pub fn overlay_for_parent(&self, parent_hash: B256) -> TrieOverlayTarget<N> {
        {
            let inner = self.inner.read();
            if !inner.dirty {
                return inner.overlay_for_parent(parent_hash);
            }
        }

        let mut inner = self.inner.write();
        if inner.dirty {
            inner.rebuild_overlays();
        }
        inner.overlay_for_parent(parent_hash)
    }
}

/// Overlay information for a payload parent.
#[derive(Clone, Debug)]
pub struct TrieOverlayTarget<N: NodePrimitives = EthPrimitives> {
    /// The persisted ancestor this overlay is based on.
    pub anchor_hash: B256,
    /// The in-memory overlay, if the requested parent has unpersisted ancestors.
    pub overlay: Option<TrieOverlay<N>>,
}

#[derive(Default)]
struct TrieOverlayManagerInner<N: NodePrimitives = EthPrimitives> {
    blocks: B256Map<ExecutedBlock<N>>,
    overlays: B256Map<TrieOverlay<N>>,
    dirty: bool,
}

impl<N: NodePrimitives> TrieOverlayManagerInner<N> {
    fn overlay_for_parent(&self, parent_hash: B256) -> TrieOverlayTarget<N> {
        if let Some(overlay) = self.overlays.get(&parent_hash).cloned() {
            TrieOverlayTarget { anchor_hash: overlay.anchor_hash(), overlay: Some(overlay) }
        } else {
            TrieOverlayTarget { anchor_hash: parent_hash, overlay: None }
        }
    }

    fn rebuild_overlays(&mut self) {
        let mut blocks = self.blocks.values().cloned().collect::<Vec<_>>();
        blocks.sort_unstable_by_key(|block| block.recovered_block().number());

        self.overlays.clear();
        for block in blocks {
            let hash = block.recovered_block().hash();
            let parent = self.overlays.get(&block.recovered_block().parent_hash()).cloned();
            self.overlays.insert(hash, TrieOverlay::new(block, parent));
        }

        self.dirty = false;
    }
}

/// Flattened trie overlay for one in-memory parent block.
#[derive(Clone)]
pub struct TrieOverlay<N: NodePrimitives = EthPrimitives> {
    inner: Arc<TrieOverlayInner<N>>,
}

impl<N: NodePrimitives> std::fmt::Debug for TrieOverlay<N> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TrieOverlay")
            .field("head_hash", &self.head_hash())
            .field("anchor_hash", &self.anchor_hash())
            .field("num_blocks", &self.num_blocks())
            .field("cached_anchors", &self.inner.computed.read().len())
            .finish()
    }
}

struct TrieOverlayInner<N: NodePrimitives = EthPrimitives> {
    block: ExecutedBlock<N>,
    parent: Option<TrieOverlay<N>>,
    anchor_hash: B256,
    num_blocks: usize,
    computed: RwLock<B256Map<Arc<TrieInputSorted>>>,
}

impl<N: NodePrimitives> TrieOverlay<N> {
    fn new(block: ExecutedBlock<N>, parent: Option<Self>) -> Self {
        debug_assert!(
            parent
                .as_ref()
                .is_none_or(|parent| parent.head_hash() == block.recovered_block().parent_hash()),
            "parent overlay must match block parent"
        );

        let anchor_hash = parent
            .as_ref()
            .map_or_else(|| block.recovered_block().parent_hash(), Self::anchor_hash);
        let num_blocks = parent.as_ref().map_or(1, |parent| parent.num_blocks() + 1);

        Self {
            inner: Arc::new(TrieOverlayInner {
                block,
                parent,
                anchor_hash,
                num_blocks,
                computed: Default::default(),
            }),
        }
    }

    /// Returns the hash of the in-memory block this overlay represents.
    pub fn head_hash(&self) -> B256 {
        self.inner.block.recovered_block().hash()
    }

    /// Returns the persisted ancestor hash for the full overlay.
    pub fn anchor_hash(&self) -> B256 {
        self.inner.anchor_hash
    }

    /// Returns the number of in-memory blocks covered by the full overlay.
    pub fn num_blocks(&self) -> usize {
        self.inner.num_blocks
    }

    /// Returns true if this overlay can be re-anchored to `hash`.
    ///
    /// This is true for the parent hash of any block in the linked in-memory segment.
    pub fn has_anchor_hash(&self, hash: B256) -> bool {
        self.inner.block.recovered_block().parent_hash() == hash ||
            self.inner.parent.as_ref().is_some_and(|parent| parent.has_anchor_hash(hash))
    }

    /// Pre-computes the full overlay and returns it.
    pub fn precompute(&self) -> Arc<TrieInputSorted> {
        self.get(self.anchor_hash())
    }

    /// Returns the overlay as `(trie updates, hashed state)` for the requested anchor.
    pub fn as_overlay(
        &self,
        anchor_hash: B256,
    ) -> (Arc<TrieUpdatesSorted>, Arc<HashedPostStateSorted>) {
        let input = self.get(anchor_hash);
        (Arc::clone(&input.nodes), Arc::clone(&input.state))
    }

    /// Returns the flattened trie input for the requested anchor.
    pub fn get(&self, anchor_hash: B256) -> Arc<TrieInputSorted> {
        if let Some(input) = self.inner.computed.read().get(&anchor_hash).cloned() {
            return input;
        }

        let input = self.compute(anchor_hash);
        let mut computed = self.inner.computed.write();
        computed.entry(anchor_hash).or_insert_with(|| Arc::clone(&input)).clone()
    }

    fn compute(&self, anchor_hash: B256) -> Arc<TrieInputSorted> {
        let block_parent_hash = self.inner.block.recovered_block().parent_hash();
        let trie_data = self.inner.block.trie_data();

        if block_parent_hash == anchor_hash {
            trace!(
                target: "chain_state::trie_overlay",
                %anchor_hash,
                head = %self.head_hash(),
                "building single-block trie overlay"
            );
            return Arc::new(TrieInputSorted::new(
                trie_data.trie_updates,
                trie_data.hashed_state,
                Default::default(),
            ));
        }

        let Some(parent) = &self.inner.parent else {
            panic!("TrieOverlay for head {} cannot be anchored to {anchor_hash}", self.head_hash());
        };

        if !parent.has_anchor_hash(anchor_hash) {
            panic!(
                "TrieOverlay for head {} does not contain requested anchor {anchor_hash}",
                self.head_hash()
            );
        }

        trace!(
            target: "chain_state::trie_overlay",
            %anchor_hash,
            head = %self.head_hash(),
            "extending parent trie overlay"
        );

        let mut overlay = parent.get(anchor_hash).as_ref().clone();
        extend_overlay(&mut overlay, &trie_data.hashed_state, &trie_data.trie_updates);
        Arc::new(overlay)
    }
}

fn extend_overlay(
    overlay: &mut TrieInputSorted,
    hashed_state: &HashedPostStateSorted,
    trie_updates: &TrieUpdatesSorted,
) {
    #[cfg(feature = "rayon")]
    {
        rayon::join(
            || {
                if !hashed_state.is_empty() {
                    Arc::make_mut(&mut overlay.state).extend_ref_and_sort(hashed_state);
                }
            },
            || {
                if !trie_updates.is_empty() {
                    Arc::make_mut(&mut overlay.nodes).extend_ref_and_sort(trie_updates);
                }
            },
        );
    }

    #[cfg(not(feature = "rayon"))]
    {
        if !hashed_state.is_empty() {
            Arc::make_mut(&mut overlay.state).extend_ref_and_sort(hashed_state);
        }
        if !trie_updates.is_empty() {
            Arc::make_mut(&mut overlay.nodes).extend_ref_and_sort(trie_updates);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{test_utils::TestBlockBuilder, ComputedTrieData, EthPrimitives, ExecutedBlock};
    use alloy_primitives::U256;
    use reth_primitives_traits::Account;
    use reth_trie::{updates::TrieUpdatesSorted, HashedPostState, HashedStorage};
    use std::sync::Arc;

    fn with_unique_state(
        block: &ExecutedBlock<EthPrimitives>,
        id: u8,
    ) -> ExecutedBlock<EthPrimitives> {
        let hashed_address = B256::with_last_byte(id);
        let hashed_slot = B256::with_last_byte(id.saturating_add(32));
        let hashed_state = HashedPostState::default()
            .with_accounts([(hashed_address, Some(Account::default()))])
            .with_storages([(
                hashed_address,
                HashedStorage::from_iter(false, [(hashed_slot, U256::from(id))]),
            )])
            .into_sorted();

        ExecutedBlock::new(
            Arc::clone(&block.recovered_block),
            Arc::clone(&block.execution_output),
            ComputedTrieData::new(Arc::new(hashed_state), Arc::new(TrieUpdatesSorted::default())),
        )
    }

    fn test_blocks() -> Vec<ExecutedBlock<EthPrimitives>> {
        TestBlockBuilder::eth()
            .get_executed_blocks(1..4)
            .enumerate()
            .map(|(index, block)| with_unique_state(&block, index as u8 + 1))
            .collect()
    }

    #[test]
    fn returns_no_overlay_for_persisted_parent() {
        let manager = TrieOverlayManager::<EthPrimitives>::default();
        let parent = B256::random();

        let target = manager.overlay_for_parent(parent);

        assert_eq!(target.anchor_hash, parent);
        assert!(target.overlay.is_none());
    }

    #[test]
    fn builds_linked_overlay_for_inserted_blocks() {
        let manager = TrieOverlayManager::default();
        let blocks = test_blocks();
        for block in &blocks {
            manager.insert_block(block.clone());
        }

        let target = manager.overlay_for_parent(blocks[2].recovered_block().hash());
        let overlay = target.overlay.expect("overlay exists");

        assert_eq!(target.anchor_hash, blocks[0].recovered_block().parent_hash());
        assert_eq!(overlay.num_blocks(), 3);

        let full = overlay.get(target.anchor_hash);
        assert_eq!(full.state.accounts.len(), 3);

        let short_anchor = blocks[1].recovered_block().hash();
        let short = overlay.get(short_anchor);
        assert_eq!(short.state.accounts.len(), 1);
        assert!(Arc::ptr_eq(&short, &overlay.get(short_anchor)));
    }

    #[test]
    fn rebuilds_after_pruning_blocks() {
        let manager = TrieOverlayManager::default();
        let blocks = test_blocks();
        for block in &blocks {
            manager.insert_block(block.clone());
        }

        manager.remove_block(blocks[0].recovered_block().hash());

        let target = manager.overlay_for_parent(blocks[2].recovered_block().hash());
        let overlay = target.overlay.expect("overlay exists");

        assert_eq!(target.anchor_hash, blocks[0].recovered_block().hash());
        assert_eq!(overlay.num_blocks(), 2);

        let full = overlay.get(target.anchor_hash);
        assert_eq!(full.state.accounts.len(), 2);
    }
}
