//! Flattened state trie overlays for in-memory blocks.
//!
//! Payload validation needs a view of the state trie as of an in-memory parent block even when that
//! parent has not been persisted yet. [`StateTrieOverlayManager`] tracks those in-memory blocks and
//! builds reusable flattened state trie overlays on demand.

use crate::{EthPrimitives, ExecutedBlock};
use alloy_primitives::{map::B256Map, B256};
use parking_lot::RwLock;
use reth_primitives_traits::{AlloyBlockHeader, NodePrimitives};
use reth_trie::{updates::TrieUpdatesSorted, HashedPostStateSorted, TrieInputSorted};
use std::{collections::HashMap, fmt, sync::Arc};
use tracing::trace;

/// Manages flattened state trie overlays for in-memory blocks.
///
/// The manager owns the in-memory block graph and a cache of flattened state trie overlays keyed by
/// `(anchor_hash, tip_hash)`. [`StateTrieOverlay`] handles are cheap references back into this
/// manager, so payload validation can pass them around without cloning block segments.
#[derive(Clone)]
pub struct StateTrieOverlayManager<N: NodePrimitives = EthPrimitives> {
    inner: Arc<RwLock<StateTrieOverlayManagerInner<N>>>,
}

impl<N: NodePrimitives> Default for StateTrieOverlayManager<N> {
    fn default() -> Self {
        Self { inner: Default::default() }
    }
}

impl<N: NodePrimitives> std::fmt::Debug for StateTrieOverlayManager<N> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let inner = self.inner.read();
        f.debug_struct("StateTrieOverlayManager")
            .field("blocks", &inner.blocks.len())
            .field("overlays", &inner.overlays.len())
            .finish()
    }
}

impl<N: NodePrimitives> StateTrieOverlayManager<N> {
    /// Inserts an executed in-memory block into the state trie overlay manager.
    pub fn insert_block(&self, block: ExecutedBlock<N>) {
        let hash = block.recovered_block().hash();
        let mut inner = self.inner.write();
        inner.blocks.entry(hash).or_insert(block);
    }

    /// Removes a block from the live block graph and prunes cached overlays that can no longer be
    /// built from the remaining blocks.
    pub fn remove_block(&self, hash: B256) {
        let mut inner = self.inner.write();
        if inner.blocks.remove(&hash).is_some() {
            inner.prune_overlays();
        }
    }

    /// Returns an overlay for the requested in-memory parent hash and the persisted anchor hash it
    /// sits on.
    pub fn overlay_for_parent(
        &self,
        parent_hash: B256,
    ) -> Result<(StateTrieOverlay<N>, B256), StateTrieOverlayError> {
        let inner = self.inner.read();
        inner.overlay_for_parent(self.clone(), parent_hash)
    }

    fn get_overlay(
        &self,
        tip_hash: B256,
        anchor_hash: B256,
    ) -> Result<Arc<TrieInputSorted>, StateTrieOverlayError> {
        let key = OverlayCacheKey { anchor_hash, tip_hash };
        if let Some(input) = self.inner.read().overlays.get(&key).cloned() {
            return Ok(input)
        }

        let compute = {
            let inner = self.inner.read();
            if let Some(input) = inner.overlays.get(&key).cloned() {
                return Ok(input)
            }

            let blocks = inner
                .blocks_to_anchor(tip_hash, anchor_hash)
                .ok_or(StateTrieOverlayError { tip_hash, anchor_hash: Some(anchor_hash) })?;
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

            (blocks, parent_input)
        };

        let input = Arc::new(compute_overlay(compute.0, compute.1, anchor_hash));
        let mut inner = self.inner.write();
        Ok(inner.overlays.entry(key).or_insert_with(|| Arc::clone(&input)).clone())
    }

    fn has_anchor_hash(&self, tip_hash: B256, anchor_hash: B256) -> bool {
        let inner = self.inner.read();
        StateTrieOverlayManagerInner::<N>::has_anchor_hash(&inner.blocks, tip_hash, anchor_hash)
    }
}

/// Error returned when a state trie overlay cannot be built from the manager's current block set.
#[derive(Debug)]
pub struct StateTrieOverlayError {
    /// Requested in-memory tip hash.
    tip_hash: B256,
    /// Requested anchor hash.
    anchor_hash: Option<B256>,
}

impl fmt::Display for StateTrieOverlayError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.anchor_hash {
            Some(anchor_hash) => write!(
                f,
                "state trie overlay for tip {} cannot be anchored to {} with current blocks",
                self.tip_hash, anchor_hash
            ),
            None => {
                write!(f, "state trie overlay tip {} is not tracked by the manager", self.tip_hash)
            }
        }
    }
}

impl std::error::Error for StateTrieOverlayError {}

#[derive(Default)]
struct StateTrieOverlayManagerInner<N: NodePrimitives = EthPrimitives> {
    blocks: B256Map<ExecutedBlock<N>>,
    overlays: HashMap<OverlayCacheKey, Arc<TrieInputSorted>>,
}

impl<N: NodePrimitives> StateTrieOverlayManagerInner<N> {
    fn overlay_for_parent(
        &self,
        manager: StateTrieOverlayManager<N>,
        parent_hash: B256,
    ) -> Result<(StateTrieOverlay<N>, B256), StateTrieOverlayError> {
        let anchor_hash = self
            .anchor_for_tip(parent_hash)
            .ok_or(StateTrieOverlayError { tip_hash: parent_hash, anchor_hash: None })?;
        Ok((StateTrieOverlay { manager, tip_hash: parent_hash }, anchor_hash))
    }

    fn anchor_for_tip(&self, tip_hash: B256) -> Option<B256> {
        let mut hash = tip_hash;

        loop {
            let block = self.blocks.get(&hash)?;

            let parent_hash = block.recovered_block().parent_hash();
            if !self.blocks.contains_key(&parent_hash) {
                return Some(parent_hash)
            }
            hash = parent_hash;
        }
    }

    fn blocks_to_anchor(&self, tip_hash: B256, anchor_hash: B256) -> Option<Vec<ExecutedBlock<N>>> {
        let mut hash = tip_hash;
        let mut blocks = Vec::new();

        loop {
            let block = self.blocks.get(&hash)?.clone();
            let parent_hash = block.recovered_block().parent_hash();
            blocks.push(block);

            if parent_hash == anchor_hash {
                return Some(blocks)
            }
            hash = parent_hash;
        }
    }

    fn has_anchor_hash(
        blocks: &B256Map<ExecutedBlock<N>>,
        tip_hash: B256,
        anchor_hash: B256,
    ) -> bool {
        let mut hash = tip_hash;

        while let Some(block) = blocks.get(&hash) {
            let parent_hash = block.recovered_block().parent_hash();
            if parent_hash == anchor_hash {
                return true
            }
            hash = parent_hash;
        }

        false
    }

    fn prune_overlays(&mut self) {
        let blocks = &self.blocks;
        self.overlays.retain(|key, _| Self::has_anchor_hash(blocks, key.tip_hash, key.anchor_hash));
    }
}

/// Flattened state trie overlay for one in-memory parent block.
///
/// This is a lightweight manager-backed handle. It does not own the block segment it represents;
/// the manager owns the in-memory blocks and cached flattened state trie overlays.
#[derive(Clone)]
pub struct StateTrieOverlay<N: NodePrimitives = EthPrimitives> {
    manager: StateTrieOverlayManager<N>,
    tip_hash: B256,
}

impl<N: NodePrimitives> std::fmt::Debug for StateTrieOverlay<N> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StateTrieOverlay").field("head_hash", &self.tip_hash).finish()
    }
}

impl<N: NodePrimitives> StateTrieOverlay<N> {
    /// Returns true if this overlay can be re-anchored to `hash`.
    ///
    /// This is true for the parent hash of any block in the current in-memory segment.
    pub fn has_anchor_hash(&self, hash: B256) -> bool {
        self.manager.has_anchor_hash(self.tip_hash, hash)
    }

    /// Returns the overlay as `(trie updates, hashed state)` for the requested anchor.
    pub fn as_overlay(
        &self,
        anchor_hash: B256,
    ) -> Result<(Arc<TrieUpdatesSorted>, Arc<HashedPostStateSorted>), StateTrieOverlayError> {
        let input = self.get(anchor_hash)?;
        Ok((Arc::clone(&input.nodes), Arc::clone(&input.state)))
    }

    /// Returns the flattened trie input for the requested anchor.
    fn get(&self, anchor_hash: B256) -> Result<Arc<TrieInputSorted>, StateTrieOverlayError> {
        self.manager.get_overlay(self.tip_hash, anchor_hash)
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct OverlayCacheKey {
    anchor_hash: B256,
    tip_hash: B256,
}

fn compute_overlay<N: NodePrimitives>(
    blocks: Vec<ExecutedBlock<N>>,
    parent_input: Option<Arc<TrieInputSorted>>,
    anchor_hash: B256,
) -> TrieInputSorted {
    let Some(parent_input) = parent_input else {
        trace!(
            target: "chain_state::state_trie_overlay",
            %anchor_hash,
            blocks = blocks.len(),
            "building state trie overlay from blocks"
        );
        let trie_data = blocks.iter().map(ExecutedBlock::trie_data).collect::<Vec<_>>();

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

    let block = &blocks[0];
    let trie_data = block.trie_data();

    trace!(
        target: "chain_state::state_trie_overlay",
        %anchor_hash,
        head = %block.recovered_block().hash(),
        "extending cached parent state trie overlay"
    );

    let mut overlay = parent_input.as_ref().clone();
    extend_overlay(&mut overlay, &trie_data.hashed_state, &trie_data.trie_updates);
    overlay
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
    fn errors_for_unknown_parent() {
        let manager = StateTrieOverlayManager::<EthPrimitives>::default();
        let parent = B256::random();

        let err = manager.overlay_for_parent(parent).unwrap_err();

        assert_eq!(err.tip_hash, parent);
        assert_eq!(err.anchor_hash, None);
    }

    #[test]
    fn builds_managed_overlay_for_inserted_blocks() {
        let manager = StateTrieOverlayManager::default();
        let blocks = test_blocks();
        for block in &blocks {
            manager.insert_block(block.clone());
        }

        let (overlay, anchor_hash) =
            manager.overlay_for_parent(blocks[2].recovered_block().hash()).unwrap();

        assert_eq!(anchor_hash, blocks[0].recovered_block().parent_hash());

        let full = overlay.get(anchor_hash).unwrap();
        assert_eq!(full.state.accounts.len(), 3);

        let short_anchor = blocks[1].recovered_block().hash();
        let short = overlay.get(short_anchor).unwrap();
        assert_eq!(short.state.accounts.len(), 1);
        assert!(Arc::ptr_eq(&short, &overlay.get(short_anchor).unwrap()));
    }

    #[test]
    fn prunes_cached_overlays_after_removing_blocks() {
        let manager = StateTrieOverlayManager::default();
        let blocks = test_blocks();
        for block in &blocks {
            manager.insert_block(block.clone());
        }

        let (overlay, original_anchor) =
            manager.overlay_for_parent(blocks[2].recovered_block().hash()).unwrap();
        overlay.get(original_anchor).unwrap();

        manager.remove_block(blocks[0].recovered_block().hash());

        let (overlay, anchor_hash) =
            manager.overlay_for_parent(blocks[2].recovered_block().hash()).unwrap();

        assert_eq!(anchor_hash, blocks[0].recovered_block().hash());
        assert!(!overlay.has_anchor_hash(original_anchor));

        let full = overlay.get(anchor_hash).unwrap();
        assert_eq!(full.state.accounts.len(), 2);
    }

    #[test]
    fn existing_overlay_handle_errors_after_pruning_before_resolution() {
        let manager = StateTrieOverlayManager::default();
        let blocks = test_blocks();
        for block in &blocks {
            manager.insert_block(block.clone());
        }

        let (overlay, anchor_hash) =
            manager.overlay_for_parent(blocks[2].recovered_block().hash()).unwrap();

        for block in &blocks {
            manager.remove_block(block.recovered_block().hash());
        }

        assert!(manager.overlay_for_parent(blocks[2].recovered_block().hash()).is_err());

        let err = overlay.get(anchor_hash).unwrap_err();
        assert_eq!(err.tip_hash, blocks[2].recovered_block().hash());
        assert_eq!(err.anchor_hash, Some(anchor_hash));

        drop(overlay);
        let inner = manager.inner.read();
        assert!(inner.blocks.is_empty());
        assert!(inner.overlays.is_empty());
    }
}
