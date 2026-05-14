//! Flattened state trie overlays for in-memory blocks.
//!
//! Payload validation needs a view of the state trie as of an in-memory parent block even when that
//! parent has not been persisted yet. [`StateTrieOverlayManager`] tracks those in-memory blocks and
//! builds reusable flattened state trie overlays on demand.

use crate::{EthPrimitives, ExecutedBlock};
use alloy_primitives::{map::B256Map, B256};
use parking_lot::RwLock;
use reth_primitives_traits::{AlloyBlockHeader, NodePrimitives};
#[cfg(feature = "rayon")]
use reth_tasks::{runtime::DEFAULT_STATE_TRIE_OVERLAY_WORKER_THREADS, WorkerPool};
use reth_trie::{updates::TrieUpdatesSorted, HashedPostStateSorted, TrieInputSorted};
use std::{collections::HashMap, fmt, sync::Arc};
use tracing::trace;

/// Manages flattened state trie overlays for in-memory blocks.
///
/// The manager owns the in-memory block graph and a cache of flattened state trie overlays keyed by
/// `(anchor_hash, tip_hash)`.
#[derive(Clone)]
pub struct StateTrieOverlayManager<N: NodePrimitives = EthPrimitives> {
    inner: Arc<RwLock<StateTrieOverlayManagerInner<N>>>,
    #[cfg(feature = "rayon")]
    worker_pool: Arc<WorkerPool>,
}

impl<N: NodePrimitives> Default for StateTrieOverlayManager<N> {
    fn default() -> Self {
        Self {
            inner: Default::default(),
            #[cfg(feature = "rayon")]
            worker_pool: Arc::new(WorkerPool::new(
                DEFAULT_STATE_TRIE_OVERLAY_WORKER_THREADS,
                "state-ovly",
            )),
        }
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
    /// Create a new [`StateTrieOverlayManager`] backed by the given worker pool.
    #[cfg(feature = "rayon")]
    pub fn new(worker_pool: Arc<WorkerPool>) -> Self {
        Self { inner: Default::default(), worker_pool }
    }

    /// Inserts an executed in-memory block into the state trie overlay manager.
    pub fn insert_block(&self, block: ExecutedBlock<N>) {
        let hash = block.recovered_block().hash();
        let mut inner = self.inner.write();
        inner.blocks.entry(hash).or_insert(block);
    }

    /// Removes blocks from the live block graph and prunes cached overlays that can no longer be
    /// built from the remaining blocks.
    pub fn remove_blocks(&self, hashes: impl IntoIterator<Item = B256>) {
        let removed = {
            let mut inner = self.inner.write();
            let mut removed = false;
            for hash in hashes {
                removed |= inner.blocks.remove(&hash).is_some();
            }
            if removed {
                inner.prune_epoch = inner.prune_epoch.wrapping_add(1);
            }
            removed
        };

        if removed {
            self.prune_overlays();
        }
    }

    /// Clears all managed blocks and cached overlays.
    pub fn clear(&self) {
        let mut inner = self.inner.write();
        inner.blocks.clear();
        inner.overlays.clear();
        inner.prune_epoch = inner.prune_epoch.wrapping_add(1);
    }

    fn prune_overlays(&self)
    where
        N: Send + Sync + 'static,
    {
        #[cfg(feature = "rayon")]
        {
            let inner = Arc::clone(&self.inner);
            self.worker_pool.spawn(move || {
                inner.write().prune_overlays();
            });
        }

        #[cfg(not(feature = "rayon"))]
        {
            self.inner.write().prune_overlays();
        }
    }

    /// Returns the flattened overlay from `anchor_hash` to `parent_hash`.
    pub fn overlay_for_parent(
        &self,
        parent_hash: B256,
        anchor_hash: B256,
    ) -> Result<(Arc<TrieUpdatesSorted>, Arc<HashedPostStateSorted>), StateTrieOverlayError> {
        let input = self.get_overlay(parent_hash, anchor_hash)?;
        Ok((Arc::clone(&input.nodes), Arc::clone(&input.state)))
    }

    fn get_overlay(
        &self,
        tip_hash: B256,
        anchor_hash: B256,
    ) -> Result<Arc<TrieInputSorted>, StateTrieOverlayError> {
        let key = OverlayCacheKey { anchor_hash, tip_hash };

        loop {
            let inner = self.inner.read();
            let prune_epoch = inner.prune_epoch;

            if let Some(input) = inner.overlays.get(&key).and_then(|entry| {
                (entry.prune_epoch == prune_epoch).then(|| Arc::clone(&entry.input))
            }) {
                return Ok(input)
            }

            let blocks = inner
                .blocks_to_anchor(tip_hash, anchor_hash)
                .ok_or(StateTrieOverlayError { tip_hash, anchor_hash })?;
            let parent_input = blocks.first().and_then(|block| {
                if block.recovered_block().parent_hash() == anchor_hash {
                    return None
                }

                let key = OverlayCacheKey {
                    anchor_hash,
                    tip_hash: block.recovered_block().parent_hash(),
                };
                inner.overlays.get(&key).and_then(|entry| {
                    (entry.prune_epoch == prune_epoch).then(|| Arc::clone(&entry.input))
                })
            });

            drop(inner);

            let input = Arc::new(self.compute_overlay(blocks, parent_input, anchor_hash));
            let mut inner = self.inner.write();
            if inner.prune_epoch != prune_epoch {
                continue
            }

            if let Some(input) = inner.overlays.get(&key).and_then(|entry| {
                (entry.prune_epoch == prune_epoch).then(|| Arc::clone(&entry.input))
            }) {
                return Ok(input)
            }

            inner
                .overlays
                .insert(key, OverlayCacheEntry { input: Arc::clone(&input), prune_epoch });
            return Ok(input)
        }
    }

    fn compute_overlay(
        &self,
        blocks: Vec<ExecutedBlock<N>>,
        parent_input: Option<Arc<TrieInputSorted>>,
        anchor_hash: B256,
    ) -> TrieInputSorted
    where
        N: Send + Sync,
    {
        #[cfg(feature = "rayon")]
        {
            self.worker_pool.install_fn(|| compute_overlay(blocks, parent_input, anchor_hash))
        }

        #[cfg(not(feature = "rayon"))]
        {
            compute_overlay(blocks, parent_input, anchor_hash)
        }
    }
}

/// Error returned when a state trie overlay cannot be built from the manager's current block set.
#[derive(Debug)]
pub struct StateTrieOverlayError {
    /// Requested in-memory tip hash.
    tip_hash: B256,
    /// Requested anchor hash.
    anchor_hash: B256,
}

impl fmt::Display for StateTrieOverlayError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "state trie overlay for tip {} cannot be anchored to {} with current blocks",
            self.tip_hash, self.anchor_hash
        )
    }
}

impl std::error::Error for StateTrieOverlayError {}

#[derive(Default)]
struct StateTrieOverlayManagerInner<N: NodePrimitives = EthPrimitives> {
    blocks: B256Map<ExecutedBlock<N>>,
    overlays: HashMap<OverlayCacheKey, OverlayCacheEntry>,
    prune_epoch: u64,
}

impl<N: NodePrimitives> StateTrieOverlayManagerInner<N> {
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
        let prune_epoch = self.prune_epoch;
        self.overlays.retain(|key, entry| {
            if Self::has_anchor_hash(blocks, key.tip_hash, key.anchor_hash) {
                entry.prune_epoch = prune_epoch;
                true
            } else {
                false
            }
        });
    }
}

struct OverlayCacheEntry {
    input: Arc<TrieInputSorted>,
    prune_epoch: u64,
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
        let anchor = B256::random();

        let err = manager.overlay_for_parent(parent, anchor).unwrap_err();

        assert_eq!(err.tip_hash, parent);
        assert_eq!(err.anchor_hash, anchor);
    }

    #[test]
    fn builds_managed_overlay_for_inserted_blocks() {
        let manager = StateTrieOverlayManager::default();
        let blocks = test_blocks();
        for block in &blocks {
            manager.insert_block(block.clone());
        }

        let anchor_hash = blocks[0].recovered_block().parent_hash();

        let (_, state) =
            manager.overlay_for_parent(blocks[2].recovered_block().hash(), anchor_hash).unwrap();
        assert_eq!(state.accounts.len(), 3);

        let short_anchor = blocks[1].recovered_block().hash();
        let (_, short) =
            manager.overlay_for_parent(blocks[2].recovered_block().hash(), short_anchor).unwrap();
        assert_eq!(short.accounts.len(), 1);
        let (_, cached_short) =
            manager.overlay_for_parent(blocks[2].recovered_block().hash(), short_anchor).unwrap();
        assert!(Arc::ptr_eq(&short, &cached_short));
    }

    #[test]
    fn prunes_cached_overlays_after_removing_blocks() {
        let manager = StateTrieOverlayManager::default();
        let blocks = test_blocks();
        for block in &blocks {
            manager.insert_block(block.clone());
        }

        let original_anchor = blocks[0].recovered_block().parent_hash();
        manager.overlay_for_parent(blocks[2].recovered_block().hash(), original_anchor).unwrap();

        manager.remove_blocks([
            blocks[0].recovered_block().hash(),
            blocks[1].recovered_block().hash(),
        ]);

        let anchor_hash = blocks[1].recovered_block().hash();
        assert!(manager
            .overlay_for_parent(blocks[2].recovered_block().hash(), original_anchor)
            .is_err());

        let (_, state) =
            manager.overlay_for_parent(blocks[2].recovered_block().hash(), anchor_hash).unwrap();
        assert_eq!(state.accounts.len(), 1);
    }
}
