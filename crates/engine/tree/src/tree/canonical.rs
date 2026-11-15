//! Canonical chain updates of the engine API tree handler.

use crate::tree::{payload_validator::EngineValidator, EngineApiTreeHandler, WaitForCaches};
use alloy_consensus::BlockHeader;
use alloy_eips::NumHash;
use alloy_primitives::B256;
use reth_chain_state::{ExecutedBlock, NewCanonicalChain};
use reth_engine_primitives::ConsensusEngineEvent;
use reth_errors::ProviderResult;
use reth_evm::{debug_unreachable, ConfigureEvm};
use reth_payload_primitives::{BuiltPayload, PayloadTypes};
use reth_primitives_traits::{
    FastInstant as Instant, NodePrimitives, RecoveredBlock, SealedHeader,
};
use reth_provider::{
    BalProvider, BlockExecutionOutput, BlockExecutionResult, BlockReader, ChangeSetReader,
    DatabaseProviderFactory, ProviderError, PruneCheckpointReader, StageCheckpointReader,
    StateProviderFactory, StateReader, StorageChangeSetReader, StorageSettingsCache,
    TransactionVariant,
};
use reth_trie::{HashedPostState, KeccakKeyHasher, SortedTrieData};
use std::sync::Arc;
use tracing::{debug, instrument, trace, warn};

impl<N, P, T, V, C> EngineApiTreeHandler<N, P, T, V, C>
where
    N: NodePrimitives,
    P: DatabaseProviderFactory
        + BlockReader<Block = N::Block, Header = N::BlockHeader>
        + StateProviderFactory
        + StateReader<Receipt = N::Receipt>
        + BalProvider
        + Clone
        + 'static,
    P::Provider: BlockReader<Block = N::Block, Header = N::BlockHeader>
        + PruneCheckpointReader
        + StageCheckpointReader
        + ChangeSetReader
        + StorageChangeSetReader
        + StorageSettingsCache
        + 'static,
    C: ConfigureEvm<Primitives = N> + 'static,
    T: PayloadTypes<BuiltPayload: BuiltPayload<Primitives = N>>,
    V: EngineValidator<T> + WaitForCaches,
{
    /// Returns the new chain for the given head.
    ///
    /// This also handles reorgs.
    ///
    /// Note: This does not update the tracked state and instead returns the new chain based on the
    /// given head.
    pub(super) fn on_new_head(
        &self,
        new_head: B256,
    ) -> ProviderResult<Option<NewCanonicalChain<N>>> {
        // get the executed new head block
        let Some(new_head_block) = self.state.tree_state.blocks_by_hash.get(&new_head) else {
            debug!(target: "engine::tree", new_head=?new_head, "New head block not found in inmemory tree state");
            self.metrics.engine.executed_new_block_cache_miss.increment(1);
            return Ok(None)
        };

        let new_head_number = new_head_block.recovered_block().number();
        let mut current_canonical_number = self.state.tree_state.current_canonical_head.number;

        let mut new_chain = vec![new_head_block.clone()];
        let mut current_hash = new_head_block.recovered_block().parent_hash();
        let mut current_number = new_head_number - 1;

        // Walk back the new chain until we reach a block we know about
        //
        // This is only done for in-memory blocks, because we should not have persisted any blocks
        // that are _above_ the current canonical head.
        while current_number > current_canonical_number {
            if let Some(block) = self.state.tree_state.executed_block_by_hash(current_hash).cloned()
            {
                current_hash = block.recovered_block().parent_hash();
                current_number -= 1;
                new_chain.push(block);
            } else {
                warn!(target: "engine::tree", current_hash=?current_hash, "Sidechain block not found in TreeState");
                // This should never happen as we're walking back a chain that should connect to
                // the canonical chain
                return Ok(None)
            }
        }

        // If we have reached the current canonical head by walking back from the target, then we
        // know this represents an extension of the canonical chain.
        if current_hash == self.state.tree_state.current_canonical_head.hash {
            new_chain.reverse();

            // Simple extension of the current chain
            return Ok(Some(NewCanonicalChain::Commit { new: new_chain }))
        }

        // We have a reorg. Walk back both chains to find the fork point.
        let mut old_chain = Vec::new();
        let mut old_hash = self.state.tree_state.current_canonical_head.hash;

        // If the canonical chain is ahead of the new chain,
        // gather all blocks until new head number.
        while current_canonical_number > current_number {
            let block = self.canonical_block_by_hash(old_hash)?;
            old_hash = block.recovered_block().parent_hash();
            old_chain.push(block);
            current_canonical_number -= 1;
        }

        // Both new and old chain pointers are now at the same height.
        debug_assert_eq!(current_number, current_canonical_number);

        // Walk both chains from specified hashes at same height until
        // a common ancestor (fork block) is reached.
        while old_hash != current_hash {
            let block = self.canonical_block_by_hash(old_hash)?;
            old_hash = block.recovered_block().parent_hash();
            old_chain.push(block);

            if let Some(block) = self.state.tree_state.executed_block_by_hash(current_hash).cloned()
            {
                current_hash = block.recovered_block().parent_hash();
                new_chain.push(block);
            } else {
                // This shouldn't happen as we've already walked this path
                warn!(target: "engine::tree", invalid_hash=?current_hash, "New chain block not found in TreeState");
                return Ok(None)
            }
        }
        new_chain.reverse();
        old_chain.reverse();

        Ok(Some(NewCanonicalChain::Reorg { new: new_chain, old: old_chain }))
    }

    /// Updates the latest block state to the specified canonical ancestor.
    ///
    /// This method ensures that the latest block tracks the given canonical header by resetting
    ///
    /// # Arguments
    /// * `canonical_header` - The canonical header to set as the new head
    ///
    /// # Returns
    /// * `ProviderResult<()>` - Ok(()) on success, error if state update fails
    ///
    /// Caution: This unwinds the canonical chain
    pub(super) fn update_latest_block_to_canonical_ancestor(
        &mut self,
        canonical_header: &SealedHeader<N::BlockHeader>,
    ) -> ProviderResult<()> {
        debug!(target: "engine::tree", head = ?canonical_header.num_hash(), "Update latest block to canonical ancestor");
        let current_head_number = self.state.tree_state.canonical_block_number();
        let new_head_number = canonical_header.number();
        let new_head_hash = canonical_header.hash();

        // Update tree state with the new canonical head
        self.state.tree_state.set_canonical_head(canonical_header.num_hash());

        // Handle the state update based on whether this is an unwind scenario
        if new_head_number < current_head_number {
            debug!(
                target: "engine::tree",
                current_head = current_head_number,
                new_head = new_head_number,
                new_head_hash = ?new_head_hash,
                "FCU unwind detected: reverting to canonical ancestor"
            );

            self.handle_canonical_chain_unwind(current_head_number, canonical_header)
        } else {
            debug!(
                target: "engine::tree",
                previous_head = current_head_number,
                new_head = new_head_number,
                new_head_hash = ?new_head_hash,
                "Advancing latest block to canonical ancestor"
            );
            self.handle_chain_advance_or_same_height(canonical_header)
        }
    }

    /// Handles chain unwind scenarios by collecting blocks to remove and performing an unwind back
    /// to the canonical header
    fn handle_canonical_chain_unwind(
        &self,
        current_head_number: u64,
        canonical_header: &SealedHeader<N::BlockHeader>,
    ) -> ProviderResult<()> {
        let new_head_number = canonical_header.number();
        debug!(
            target: "engine::tree",
            from = current_head_number,
            to = new_head_number,
            "Handling unwind: collecting blocks to remove from in-memory state"
        );

        // Collect blocks that need to be removed from memory
        let old_blocks =
            self.collect_blocks_for_canonical_unwind(new_head_number, current_head_number);

        // Load and apply the canonical ancestor block
        self.apply_canonical_ancestor_via_reorg(canonical_header, old_blocks)
    }

    /// Collects blocks from memory that need to be removed during an unwind to a canonical block.
    fn collect_blocks_for_canonical_unwind(
        &self,
        new_head_number: u64,
        current_head_number: u64,
    ) -> Vec<ExecutedBlock<N>> {
        let mut old_blocks =
            Vec::with_capacity((current_head_number.saturating_sub(new_head_number)) as usize);

        for block_num in (new_head_number + 1)..=current_head_number {
            if let Some(block_state) = self.canonical_in_memory_state.state_by_number(block_num) {
                let executed_block = block_state.block_ref().clone();
                old_blocks.push(executed_block);
                debug!(
                    target: "engine::tree",
                    block_number = block_num,
                    "Collected block for removal from in-memory state"
                );
            }
        }

        if old_blocks.is_empty() {
            debug!(
                target: "engine::tree",
                "No blocks found in memory to remove, will clear and reset state"
            );
        }

        old_blocks
    }

    /// Applies the canonical ancestor block via a reorg operation.
    fn apply_canonical_ancestor_via_reorg(
        &self,
        canonical_header: &SealedHeader<N::BlockHeader>,
        old_blocks: Vec<ExecutedBlock<N>>,
    ) -> ProviderResult<()> {
        let new_head_hash = canonical_header.hash();
        let new_head_number = canonical_header.number();

        // Load the canonical ancestor's block
        let executed_block = self.canonical_block_by_hash(new_head_hash)?;
        // Perform the reorg to properly handle the unwind
        self.canonical_in_memory_state
            .update_chain(NewCanonicalChain::Reorg { new: vec![executed_block], old: old_blocks });

        // CRITICAL: Update the canonical head after the reorg
        // This ensures get_canonical_head() returns the correct block
        self.canonical_in_memory_state.set_canonical_head(canonical_header.clone());

        debug!(
            target: "engine::tree",
            block_number = new_head_number,
            block_hash = ?new_head_hash,
            "Successfully loaded canonical ancestor into memory via reorg"
        );

        Ok(())
    }

    /// Handles chain advance or same height scenarios.
    fn handle_chain_advance_or_same_height(
        &self,
        canonical_header: &SealedHeader<N::BlockHeader>,
    ) -> ProviderResult<()> {
        // Load the block into memory if it's not already present
        self.ensure_block_in_memory(canonical_header.number(), canonical_header.hash())?;

        // Update the canonical head header
        self.canonical_in_memory_state.set_canonical_head(canonical_header.clone());

        Ok(())
    }

    /// Ensures a block is loaded into memory if not already present.
    fn ensure_block_in_memory(&self, block_number: u64, block_hash: B256) -> ProviderResult<()> {
        // Check if block is already in memory
        if self.canonical_in_memory_state.state_by_number(block_number).is_some() {
            return Ok(());
        }

        // Load the block from storage
        let executed_block = self.canonical_block_by_hash(block_hash)?;
        self.canonical_in_memory_state
            .update_chain(NewCanonicalChain::Commit { new: vec![executed_block] });

        debug!(
            target: "engine::tree",
            block_number,
            block_hash = ?block_hash,
            "Added canonical block to in-memory state"
        );

        Ok(())
    }

    /// Attempts to make the given target canonical.
    ///
    /// This will update the tracked canonical in memory state and do the necessary housekeeping.
    pub(super) fn make_canonical(&mut self, target: B256) -> ProviderResult<()> {
        if let Some(chain_update) = self.on_new_head(target)? {
            self.on_canonical_chain_update(chain_update);
        }

        self.on_canonicalized_sync_target(target);

        Ok(())
    }

    /// Applies the tracked forkchoice state once its sync target head becomes canonical.
    pub(super) fn on_canonicalized_sync_target(&mut self, target: B256) {
        let Some(sync_target_state) = self
            .state
            .forkchoice_state_tracker
            .sync_target_state()
            .filter(|state| state.head_block_hash == target)
        else {
            return;
        };

        if let Err(outcome) = self.ensure_consistent_forkchoice_state(sync_target_state) {
            debug!(
                target: "engine::tree",
                head = %sync_target_state.head_block_hash,
                safe = %sync_target_state.safe_block_hash,
                finalized = %sync_target_state.finalized_block_hash,
                ?outcome,
                "Canonicalized sync target head before safe/finalized could be applied"
            );
            return;
        }

        self.state.forkchoice_state_tracker.promote_sync_target_to_valid(sync_target_state);
    }

    /// Return an [`ExecutedBlock`] from database or in-memory state by hash.
    ///
    /// Note: This function attempts to fetch the `ExecutedBlock` from either in-memory state
    /// or the database. If the required historical data (such as trie change sets) has been
    /// pruned for a given block, this operation will return an error. On archive nodes, it
    /// can retrieve any block.
    #[instrument(level = "debug", target = "engine::tree", skip(self))]
    fn canonical_block_by_hash(&self, hash: B256) -> ProviderResult<ExecutedBlock<N>> {
        trace!(target: "engine::tree", ?hash, "Fetching executed block by hash");
        // check memory first
        if let Some(block) = self.state.tree_state.executed_block_by_hash(hash) {
            return Ok(block.clone())
        }

        let (block, senders) = self
            .provider
            .sealed_block_with_senders(hash.into(), TransactionVariant::WithHash)?
            .ok_or_else(|| ProviderError::HeaderNotFound(hash.into()))?
            .split_sealed();
        let mut execution_output = self
            .provider
            .get_state(block.header().number())?
            .ok_or_else(|| ProviderError::StateForNumberNotFound(block.header().number()))?;
        let bundle_state = execution_output.state();
        // `get_state` can return an in-memory execution outcome that retains destruction statuses.
        // Hashing it requires the parent provider to expand a pre-existing destroyed account's
        // storage into zero-valued slots. Genesis has no parent state, and no account existed
        // before it to be destroyed.
        let hashed_state = if block.parent_hash().is_zero() {
            HashedPostState::from_bundle_state::<KeccakKeyHasher>(bundle_state.state())
        } else {
            self.provider
                .state_by_block_hash(block.parent_hash())?
                .hashed_post_state(bundle_state)?
        };

        debug!(
            target: "engine::tree",
            number = ?block.number(),
            "computing block trie updates",
        );
        let db_provider = self.provider.database_provider_ro()?;
        let trie_updates = self
            .state
            .tree_state
            .overlay_manager
            .compute_block_trie_updates(&db_provider, block.number())?;

        let sorted_hashed_state = Arc::new(hashed_state.into_sorted());
        let sorted_trie_updates = Arc::new(trie_updates);
        let trie_data = SortedTrieData::new(sorted_hashed_state, sorted_trie_updates);

        let execution_output = Arc::new(BlockExecutionOutput {
            state: execution_output.bundle,
            result: BlockExecutionResult {
                receipts: execution_output.receipts.pop().unwrap_or_default(),
                requests: execution_output.requests.pop().unwrap_or_default(),
                gas_used: block.gas_used(),
                blob_gas_used: block.blob_gas_used().unwrap_or_default(),
            },
        });

        Ok(ExecutedBlock::new(
            Arc::new(RecoveredBlock::new_sealed(block, senders)),
            execution_output,
            trie_data,
        ))
    }

    /// Invoked when we the canonical chain has been updated.
    ///
    /// This is invoked on a valid forkchoice update, or if we can make the target block canonical.
    pub(super) fn on_canonical_chain_update(&mut self, chain_update: NewCanonicalChain<N>) {
        trace!(target: "engine::tree", new_blocks = %chain_update.new_block_count(), reorged_blocks =  %chain_update.reorged_block_count(), "applying new chain update");
        let start = Instant::now();

        // update the tracked canonical head
        self.state.tree_state.set_canonical_head(chain_update.tip().num_hash());

        let tip = chain_update.tip().clone_sealed_header();
        let notification = chain_update.to_chain_notification();

        // reinsert any missing reorged blocks
        if let NewCanonicalChain::Reorg { new, old } = &chain_update {
            let new_first = new.first().map(|first| first.recovered_block().num_hash());
            let old_first = old.first().map(|first| first.recovered_block().num_hash());
            trace!(target: "engine::tree", ?new_first, ?old_first, "Reorg detected, new and old first blocks");

            self.state.set_pending_sparse_trie_prune(false);
            self.update_reorg_metrics(old.len(), old_first);
            self.reinsert_reorged_blocks(new.clone());
            self.reinsert_reorged_blocks(old.clone());
        }

        // update the tracked in-memory state with the new chain
        self.canonical_in_memory_state.update_chain(chain_update);
        self.canonical_in_memory_state.set_canonical_head(tip.clone());
        self.payload_validator.on_canonical_head_changed(tip.hash(), &self.state);

        // Update metrics based on new tip
        self.metrics.tree.canonical_chain_height.set(tip.number() as f64);

        // sends an event to all active listeners about the new canonical chain
        self.canonical_in_memory_state.notify_canon_state(notification);

        // emit event
        self.emit_event(ConsensusEngineEvent::CanonicalChainCommitted(
            Box::new(tip),
            start.elapsed(),
        ));
    }

    /// This updates metrics based on the given reorg length and first reorged block number.
    pub(super) fn update_reorg_metrics(
        &self,
        old_chain_length: usize,
        first_reorged_block: Option<NumHash>,
    ) {
        if let Some(first_reorged_block) = first_reorged_block.map(|block| block.number) {
            if let Some(finalized) = self.canonical_in_memory_state.get_finalized_num_hash() &&
                first_reorged_block <= finalized.number
            {
                self.metrics.tree.reorgs.finalized.increment(1);
            } else if let Some(safe) = self.canonical_in_memory_state.get_safe_num_hash() &&
                first_reorged_block <= safe.number
            {
                self.metrics.tree.reorgs.safe.increment(1);
            } else {
                self.metrics.tree.reorgs.head.increment(1);
            }
        } else {
            debug_unreachable!("Reorged chain doesn't have any blocks");
        }
        self.metrics.tree.latest_reorg_depth.set(old_chain_length as f64);
    }

    /// This reinserts any blocks in the new chain that do not already exist in the tree
    fn reinsert_reorged_blocks(&mut self, new_chain: Vec<ExecutedBlock<N>>) {
        for block in new_chain {
            if self
                .state
                .tree_state
                .executed_block_by_hash(block.recovered_block().hash())
                .is_none()
            {
                trace!(target: "engine::tree", num=?block.recovered_block().number(), hash=?block.recovered_block().hash(), "Reinserting block into tree state");
                self.state.tree_state.insert_executed(block);
            }
        }
        self.metrics.engine.executed_blocks.set(self.state.tree_state.block_count() as f64);
    }
}
