//! Canonical chain updates of the engine API tree handler.

use crate::tree::{payload_validator::EngineValidator, EngineApiTreeHandler, WaitForCaches};
use alloy_consensus::BlockHeader;
use alloy_eips::{BlockNumHash, NumHash};
use alloy_primitives::B256;
use reth_chain_state::{ExecutedBlock, NewCanonicalChain};
use reth_engine_primitives::ConsensusEngineEvent;
use reth_errors::ProviderResult;
use reth_evm::ConfigureEvm;
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
use reth_trie::ComputedTrieData;
use revm::interpreter::debug_unreachable;
use std::sync::Arc;
use tracing::{debug, error, instrument, trace};

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
    pub(super) fn on_new_head(&self, new_head: B256) -> ProviderResult<NewHeadChain<N>> {
        // Get the executed new head block.
        let Some(new_head_state) = self.state.tree_state.state_by_hash(new_head) else {
            debug!(target: "engine::tree", new_head=?new_head, "New head block not found in inmemory tree state");
            self.metrics.engine.executed_new_block_cache_miss.increment(1);
            return Ok(NewHeadChain::NotInMemory)
        };
        let canonical_head = self.state.tree_state.canonical_head();

        // The in-memory ancestors the new head is linked to, from newest to oldest.
        let mut new_states = new_head_state.chain().skip(1);
        // The oldest block of the new chain.
        let mut state = new_head_state.as_ref();
        let mut new_chain = vec![state.block()];

        // Walk back the new chain until it reaches the block above the canonical height.
        //
        // This is only done for in-memory blocks, because we should not have persisted any blocks
        // that are _above_ the current canonical head.
        while state.number() > canonical_head.number + 1 {
            let Some(parent) = new_states.next() else {
                return Ok(NewHeadChain::Disconnected {
                    oldest: BlockNumHash::new(state.number(), state.hash()),
                })
            };
            state = parent;
            new_chain.push(state.block());
        }

        // If the new chain is built on the current canonical head, then we know this represents
        // an extension of the canonical chain.
        if state.parent_hash() == canonical_head.hash {
            new_chain.reverse();

            // Simple extension of the current chain.
            return Ok(NewHeadChain::Update(NewCanonicalChain::Commit { new: new_chain }))
        }

        // We have a reorg. Walk back both chains to find the fork point.
        //
        // The old chain is the canonical chain from the canonical head down. Its in-memory blocks
        // are the chain of the highest canonical in-memory block, the blocks below them are loaded
        // from the database.
        let old_head_state = self.state.tree_state.canonical_in_memory_state().head_state();
        let mut old_states = old_head_state.iter().flat_map(|state| state.chain());
        let mut old_chain = Vec::<ExecutedBlock<N>>::new();
        let next_old_hash = |old_chain: &[ExecutedBlock<N>]| {
            old_chain
                .last()
                .map_or(canonical_head.hash, |block| block.recovered_block().parent_hash())
        };
        let mut old_block = |hash: B256| match old_states.next() {
            Some(state) if state.hash() == hash => Ok(state.block()),
            _ => self.canonical_block_by_hash(hash),
        };

        // If the canonical chain is ahead of the new chain, gather all blocks down to the height
        // of the oldest block of the new chain.
        for _ in state.number()..=canonical_head.number {
            let block = old_block(next_old_hash(&old_chain))?;
            old_chain.push(block);
        }

        // Both chains are now at the same height. Walk them back together until they build on
        // the same block, the common ancestor (fork block).
        while next_old_hash(&old_chain) != state.parent_hash() {
            let block = old_block(next_old_hash(&old_chain))?;
            old_chain.push(block);

            let Some(parent) = new_states.next() else {
                return Ok(NewHeadChain::Disconnected {
                    oldest: BlockNumHash::new(state.number(), state.hash()),
                })
            };
            state = parent;
            new_chain.push(state.block());
        }
        new_chain.reverse();
        old_chain.reverse();

        Ok(NewHeadChain::Update(NewCanonicalChain::Reorg { new: new_chain, old: old_chain }))
    }

    /// Makes `canonical_header`, a canonical ancestor of the canonical head, the canonical head.
    ///
    /// The canonical in-memory blocks above it stay tracked as non-canonical blocks. If the
    /// ancestor is not in memory, it is loaded from the database, which fails if the data that is
    /// required to rebuild its executed block has been pruned.
    ///
    /// Caution: This unwinds the canonical chain.
    pub(super) fn update_latest_block_to_canonical_ancestor(
        &mut self,
        canonical_header: &SealedHeader<N::BlockHeader>,
    ) -> ProviderResult<()> {
        let current_head = self.state.tree_state.canonical_head();
        debug!(
            target: "engine::tree",
            ?current_head,
            new_head = ?canonical_header.num_hash(),
            "FCU unwind detected: reverting to canonical ancestor"
        );

        // The header is below the canonical head, so this never advances the head or replaces it
        // at its height. `find_canonical_header` returns a header only if it is not above the
        // canonical head and its hash is the canonical hash at its height, which it resolves from
        // the canonical in-memory blocks and, where there is none, from the database. The
        // canonical head is the highest canonical in-memory block or, if no in-memory block is
        // canonical, the persisted block at its height. The only header it returns at the height
        // of the canonical head is therefore the head itself, and a forkchoice update to the
        // canonical head is handled by `handle_canonical_head`.
        debug_assert!(
            canonical_header.number() < current_head.number,
            "canonical ancestor {:?} is not below the canonical head {current_head:?}",
            canonical_header.num_hash()
        );

        let block = self.canonical_block_by_hash(canonical_header.hash())?;
        self.state.canonical_in_memory_state_writer.unwind_canonical_head(block);

        Ok(())
    }

    /// Attempts to make the given target canonical.
    ///
    /// This will update the tracked canonical in memory state and do the necessary housekeeping.
    pub(super) fn make_canonical(&mut self, target: B256) -> ProviderResult<()> {
        match self.on_new_head(target)? {
            NewHeadChain::Update(chain_update) => self.on_canonical_chain_update(chain_update),
            NewHeadChain::NotInMemory => {}
            NewHeadChain::Disconnected { oldest } => {
                error!(target: "engine::tree", head = ?target, ?oldest, "Executed head does not connect to the canonical chain");
            }
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
        if let Some(state) = self.state.tree_state.state_by_hash(hash) {
            return Ok(state.block())
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
        // storage into zero-valued slots.
        let hashed_state = self
            .provider
            .state_by_block_hash(block.parent_hash())?
            .hashed_post_state(bundle_state)?;

        debug!(
            target: "engine::tree",
            number = ?block.number(),
            "computing block trie updates",
        );
        let db_provider = self.provider.database_provider_ro()?;
        let trie_updates = self
            .state
            .tree_state
            .overlay_manager()
            .compute_block_trie_updates(&db_provider, block.number())?;

        let sorted_hashed_state = Arc::new(hashed_state.into_sorted());
        let sorted_trie_updates = Arc::new(trie_updates);
        let trie_data = ComputedTrieData::new(sorted_hashed_state, sorted_trie_updates);

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

        let tip = chain_update.tip().clone_sealed_header();
        let notification = chain_update.to_chain_notification();

        if let NewCanonicalChain::Reorg { new, old } = &chain_update {
            let new_first = new.first().map(|first| first.recovered_block().num_hash());
            let old_first = old.first().map(|first| first.recovered_block().num_hash());
            trace!(target: "engine::tree", ?new_first, ?old_first, "Reorg detected, new and old first blocks");

            self.state.set_pending_sparse_trie_prune(false);
            self.update_reorg_metrics(old.len(), old_first);
        }

        // Update the tracked in-memory state and the tracked canonical head with the new chain.
        self.state.canonical_in_memory_state_writer.update_chain(chain_update);
        self.payload_validator.on_canonical_head_changed(tip.hash(), &self.state);

        // Update metrics based on new tip
        self.metrics.tree.canonical_chain_height.set(tip.number() as f64);

        // sends an event to all active listeners about the new canonical chain
        self.state.canonical_in_memory_state_writer.notify_canon_state(notification);

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
            let canonical_in_memory_state = self.state.tree_state.canonical_in_memory_state();
            if let Some(finalized) = canonical_in_memory_state.get_finalized_num_hash() &&
                first_reorged_block <= finalized.number
            {
                self.metrics.tree.reorgs.finalized.increment(1);
            } else if let Some(safe) = canonical_in_memory_state.get_safe_num_hash() &&
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
}

/// The outcome of resolving the chain a new head would make canonical.
#[derive(Debug)]
pub(super) enum NewHeadChain<N: NodePrimitives> {
    /// The chain update that makes the head canonical.
    Update(NewCanonicalChain<N>),
    /// The head is not an executed in-memory block.
    NotInMemory,
    /// The head is in memory, but its chain does not connect to the canonical chain.
    Disconnected {
        /// The oldest block of the head's chain that could be resolved.
        oldest: BlockNumHash,
    },
}
