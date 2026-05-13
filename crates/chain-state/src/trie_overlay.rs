//! Flattened trie overlays for in-memory blocks.
//!
//! Payload validation needs a view of the trie as of an in-memory parent block even when that
//! parent has not been persisted yet. [`TrieOverlayManager`] tracks those in-memory blocks and
//! builds reusable flattened overlays on demand.

use crate::{EthPrimitives, ExecutedBlock};
use alloy_primitives::{
    map::{B256Map, B256Set},
    B256,
};
use parking_lot::RwLock;
use reth_primitives_traits::{AlloyBlockHeader, NodePrimitives};
use reth_trie::{updates::TrieUpdatesSorted, HashedPostStateSorted, TrieInputSorted};
use std::{collections::HashMap, sync::Arc};
use tracing::trace;

/// Manages flattened trie overlays for in-memory blocks.
///
/// The manager owns the in-memory block graph and a cache of flattened overlays keyed by
/// `(anchor_hash, tip_hash)`. [`TrieOverlay`] handles are cheap references back into this manager,
/// so payload validation can pass them around without cloning block segments.
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
            .finish()
    }
}

impl<N: NodePrimitives> TrieOverlayManager<N> {
    /// Inserts an executed in-memory block into the overlay manager.
    pub fn insert_block(&self, block: ExecutedBlock<N>) {
        let hash = block.recovered_block().hash();
        let mut inner = self.inner.write();
        if let Some(entry) = inner.blocks.get_mut(&hash) {
            if entry.removed {
                entry.block = block;
                entry.removed = false;
            }
            return
        }

        inner.blocks.insert(hash, ManagedBlock::new(block));
    }

    /// Removes a block from the live block graph.
    ///
    /// Leased blocks are kept until all overlay handles that reference them are dropped.
    pub fn remove_block(&self, hash: B256) {
        let mut inner = self.inner.write();
        if let Some(entry) = inner.blocks.get_mut(&hash) {
            entry.removed = true;
            inner.prune_removed_blocks();
            inner.prune_overlays();
        }
    }

    /// Removes all live tracked blocks and prunes cached overlays no longer reachable from retained
    /// block data.
    pub fn clear(&self) {
        let mut inner = self.inner.write();
        for entry in inner.blocks.values_mut() {
            entry.removed = true;
        }
        inner.prune_removed_blocks();
        inner.prune_overlays();
    }

    /// Returns an overlay for the requested parent hash and the persisted anchor hash it sits on.
    ///
    /// If the parent is not in memory, no overlay is required and the parent itself is returned as
    /// the anchor.
    pub fn overlay_for_parent(&self, parent_hash: B256) -> TrieOverlayTarget<N> {
        let mut inner = self.inner.write();
        inner.overlay_for_parent(self.clone(), parent_hash)
    }

    fn get_overlay(&self, tip_hash: B256, anchor_hash: B256) -> Arc<TrieInputSorted> {
        let key = OverlayCacheKey { anchor_hash, tip_hash };
        if let Some(input) = self.inner.read().overlays.get(&key).cloned() {
            return input;
        }

        let compute = {
            let inner = self.inner.read();
            if let Some(input) = inner.overlays.get(&key).cloned() {
                return input;
            }

            let blocks = inner.blocks_to_anchor(tip_hash, anchor_hash).unwrap_or_else(|| {
                panic!("TrieOverlay for head {tip_hash} cannot be anchored to {anchor_hash}")
            });
            let parent_input = blocks.first().and_then(|block| {
                if block.recovered_block().parent_hash() == anchor_hash {
                    return None
                }

                inner
                    .overlays
                    .get(&OverlayCacheKey {
                        anchor_hash,
                        tip_hash: block.recovered_block().parent_hash(),
                    })
                    .cloned()
            });

            OverlayCompute { blocks, parent_input }
        };

        let input = Arc::new(compute.compute(anchor_hash));
        let mut inner = self.inner.write();
        inner.overlays.entry(key).or_insert_with(|| Arc::clone(&input)).clone()
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
    blocks: B256Map<ManagedBlock<N>>,
    overlays: HashMap<OverlayCacheKey, Arc<TrieInputSorted>>,
}

impl<N: NodePrimitives> TrieOverlayManagerInner<N> {
    fn overlay_for_parent(
        &mut self,
        manager: TrieOverlayManager<N>,
        parent_hash: B256,
    ) -> TrieOverlayTarget<N> {
        match self.retain_live_segment(parent_hash) {
            Some(segment) => {
                let num_blocks = segment.block_hashes.len();
                let anchor_hash = segment.anchor_hash;
                let lease = Arc::new(TrieOverlayLease {
                    inner: Arc::clone(&manager.inner),
                    block_hashes: segment.block_hashes,
                    valid_anchors: segment.valid_anchors,
                });

                TrieOverlayTarget {
                    anchor_hash,
                    overlay: Some(TrieOverlay {
                        manager,
                        tip_hash: parent_hash,
                        anchor_hash,
                        num_blocks,
                        lease,
                    }),
                }
            }
            None => TrieOverlayTarget { anchor_hash: parent_hash, overlay: None },
        }
    }

    fn retain_live_segment(&mut self, tip_hash: B256) -> Option<OverlaySegment> {
        let segment = self.live_segment_for_tip(tip_hash)?;
        for hash in &segment.block_hashes {
            self.blocks.get_mut(hash).expect("segment block must exist").leases += 1;
        }

        Some(segment)
    }

    fn live_segment_for_tip(&self, tip_hash: B256) -> Option<OverlaySegment> {
        let mut hash = tip_hash;
        let mut block_hashes = Vec::new();
        let mut valid_anchors = B256Set::default();

        loop {
            let block = self.blocks.get(&hash)?;
            if block.removed {
                return None
            }

            block_hashes.push(hash);

            let parent_hash = block.block.recovered_block().parent_hash();
            valid_anchors.insert(parent_hash);
            match self.blocks.get(&parent_hash) {
                Some(parent) if !parent.removed => {
                    hash = parent_hash;
                }
                _ => {
                    return Some(OverlaySegment {
                        anchor_hash: parent_hash,
                        block_hashes,
                        valid_anchors,
                    })
                }
            }
        }
    }

    fn blocks_to_anchor(&self, tip_hash: B256, anchor_hash: B256) -> Option<Vec<ExecutedBlock<N>>> {
        let mut hash = tip_hash;
        let mut blocks = Vec::new();

        loop {
            let block = self.blocks.get(&hash)?.block.clone();
            let parent_hash = block.recovered_block().parent_hash();
            blocks.push(block);

            if parent_hash == anchor_hash {
                return Some(blocks)
            }
            hash = parent_hash;
        }
    }

    fn has_anchor_hash(
        blocks: &B256Map<ManagedBlock<N>>,
        tip_hash: B256,
        anchor_hash: B256,
    ) -> bool {
        let mut hash = tip_hash;

        while let Some(block) = blocks.get(&hash) {
            let parent_hash = block.block.recovered_block().parent_hash();
            if parent_hash == anchor_hash {
                return true
            }
            hash = parent_hash;
        }

        false
    }

    fn prune_removed_blocks(&mut self) {
        let removed = self
            .blocks
            .iter()
            .filter_map(|(hash, block)| (block.removed && block.leases == 0).then_some(*hash))
            .collect::<Vec<_>>();

        for hash in removed {
            self.blocks.remove(&hash);
        }
    }

    fn prune_overlays(&mut self) {
        let blocks = &self.blocks;
        self.overlays.retain(|key, _| Self::has_anchor_hash(blocks, key.tip_hash, key.anchor_hash));
    }
}

struct ManagedBlock<N: NodePrimitives = EthPrimitives> {
    block: ExecutedBlock<N>,
    leases: usize,
    removed: bool,
}

impl<N: NodePrimitives> ManagedBlock<N> {
    const fn new(block: ExecutedBlock<N>) -> Self {
        Self { block, leases: 0, removed: false }
    }
}

struct OverlaySegment {
    anchor_hash: B256,
    block_hashes: Vec<B256>,
    valid_anchors: B256Set,
}

/// Flattened trie overlay for one in-memory parent block.
///
/// This is a lightweight manager-backed handle. It does not own the block segment it represents;
/// the manager owns the in-memory blocks and cached flattened overlays.
#[derive(Clone)]
pub struct TrieOverlay<N: NodePrimitives = EthPrimitives> {
    manager: TrieOverlayManager<N>,
    tip_hash: B256,
    anchor_hash: B256,
    num_blocks: usize,
    lease: Arc<TrieOverlayLease<N>>,
}

impl<N: NodePrimitives> std::fmt::Debug for TrieOverlay<N> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TrieOverlay")
            .field("head_hash", &self.head_hash())
            .field("anchor_hash", &self.anchor_hash())
            .field("num_blocks", &self.num_blocks())
            .finish()
    }
}

impl<N: NodePrimitives> TrieOverlay<N> {
    /// Returns the hash of the in-memory block this overlay represents.
    pub const fn head_hash(&self) -> B256 {
        self.tip_hash
    }

    /// Returns the persisted ancestor hash for the full overlay.
    pub const fn anchor_hash(&self) -> B256 {
        self.anchor_hash
    }

    /// Returns the number of in-memory blocks covered by the full overlay.
    pub const fn num_blocks(&self) -> usize {
        self.num_blocks
    }

    /// Returns true if this overlay can be re-anchored to `hash`.
    ///
    /// This is true for the parent hash of any block in the leased in-memory segment.
    pub fn has_anchor_hash(&self, hash: B256) -> bool {
        self.lease.valid_anchors.contains(&hash)
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
        assert!(
            self.has_anchor_hash(anchor_hash),
            "TrieOverlay for head {} cannot be anchored to {anchor_hash}",
            self.head_hash()
        );

        self.manager.get_overlay(self.tip_hash, anchor_hash)
    }
}

struct TrieOverlayLease<N: NodePrimitives = EthPrimitives> {
    inner: Arc<RwLock<TrieOverlayManagerInner<N>>>,
    block_hashes: Vec<B256>,
    valid_anchors: B256Set,
}

impl<N: NodePrimitives> Drop for TrieOverlayLease<N> {
    fn drop(&mut self) {
        let mut inner = self.inner.write();
        for hash in &self.block_hashes {
            if let Some(block) = inner.blocks.get_mut(hash) {
                block.leases = block.leases.saturating_sub(1);
            }
        }

        inner.prune_removed_blocks();
        inner.prune_overlays();
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct OverlayCacheKey {
    anchor_hash: B256,
    tip_hash: B256,
}

struct OverlayCompute<N: NodePrimitives = EthPrimitives> {
    blocks: Vec<ExecutedBlock<N>>,
    parent_input: Option<Arc<TrieInputSorted>>,
}

impl<N: NodePrimitives> OverlayCompute<N> {
    fn compute(self, anchor_hash: B256) -> TrieInputSorted {
        let Some(parent_input) = self.parent_input else {
            trace!(
                target: "chain_state::trie_overlay",
                %anchor_hash,
                blocks = self.blocks.len(),
                "building trie overlay from blocks"
            );
            let trie_data = self.blocks.iter().map(ExecutedBlock::trie_data).collect::<Vec<_>>();

            return TrieInputSorted::new(
                TrieUpdatesSorted::merge_batch(
                    trie_data.iter().map(|data| Arc::clone(&data.trie_updates)),
                ),
                HashedPostStateSorted::merge_batch(
                    trie_data.iter().map(|data| Arc::clone(&data.hashed_state)),
                ),
                Default::default(),
            )
        };

        let block = &self.blocks[0];
        let trie_data = block.trie_data();

        trace!(
            target: "chain_state::trie_overlay",
            %anchor_hash,
            head = %block.recovered_block().hash(),
            "extending cached parent trie overlay"
        );

        let mut overlay = parent_input.as_ref().clone();
        extend_overlay(&mut overlay, &trie_data.hashed_state, &trie_data.trie_updates);
        overlay
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
    fn builds_managed_overlay_for_inserted_blocks() {
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
    fn prunes_cached_overlays_after_removing_blocks() {
        let manager = TrieOverlayManager::default();
        let blocks = test_blocks();
        for block in &blocks {
            manager.insert_block(block.clone());
        }

        let target = manager.overlay_for_parent(blocks[2].recovered_block().hash());
        let overlay = target.overlay.expect("overlay exists");
        let original_anchor = target.anchor_hash;
        overlay.precompute();

        manager.remove_block(blocks[0].recovered_block().hash());

        let target = manager.overlay_for_parent(blocks[2].recovered_block().hash());
        let overlay = target.overlay.expect("overlay exists");

        assert_eq!(target.anchor_hash, blocks[0].recovered_block().hash());
        assert_eq!(overlay.num_blocks(), 2);
        assert!(!overlay.has_anchor_hash(original_anchor));

        let full = overlay.get(target.anchor_hash);
        assert_eq!(full.state.accounts.len(), 2);
    }

    #[test]
    fn existing_overlay_handle_survives_pruning_before_resolution() {
        let manager = TrieOverlayManager::default();
        let blocks = test_blocks();
        for block in &blocks {
            manager.insert_block(block.clone());
        }

        let target = manager.overlay_for_parent(blocks[2].recovered_block().hash());
        let overlay = target.overlay.expect("overlay exists");
        let anchor_hash = target.anchor_hash;

        for block in &blocks {
            manager.remove_block(block.recovered_block().hash());
        }

        let target = manager.overlay_for_parent(blocks[2].recovered_block().hash());
        assert!(target.overlay.is_none());

        let full = overlay.get(anchor_hash);
        assert_eq!(full.state.accounts.len(), 3);

        drop(overlay);
        let inner = manager.inner.read();
        assert!(inner.blocks.is_empty());
        assert!(inner.overlays.is_empty());
    }
}
