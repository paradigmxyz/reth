//! State access and background trie preparation for payload validation.

use super::BasicEngineValidator;
use crate::tree::{
    state_root_strategy::{
        LazyHashedPostState, PayloadStateRootHandle, PayloadStateRootJobContext,
    },
    CacheWaitDurations, EngineApiTreeState, WaitForCaches,
};
use alloy_primitives::B256;
use reth_chain_state::ExecutedBlock;
use reth_errors::ProviderResult;
use reth_evm::ConfigureEvm;
use reth_primitives_traits::{
    AlloyBlockHeader, FastInstant as Instant, NodePrimitives, RecoveredBlock,
};
use reth_provider::{BlockExecutionOutput, BlockReader};
use reth_storage_overlay::{OverlayContext, OverlayStateProviderFactory};
use reth_trie::{updates::TrieUpdates, LazyTrieData};
use std::sync::Arc;
use tracing::{debug, debug_span, warn};

/// Worker name for deferred trie data preparation.
const DEFERRED_TRIE_WORKER_NAME: &str = "deferred-trie";

impl<N, P, Evm, V> BasicEngineValidator<P, Evm, V>
where
    N: NodePrimitives,
    P: BlockReader<Header = N::BlockHeader> + Clone,
    Evm: ConfigureEvm<Primitives = N>,
{
    /// Creates an overlay state provider factory for the given parent hash.
    ///
    /// Returns `None` when the parent is neither in memory nor persisted.
    pub(super) fn overlay_state_provider_factory(
        &self,
        hash: B256,
        state: &EngineApiTreeState<N>,
    ) -> ProviderResult<Option<OverlayStateProviderFactory<P, N>>> {
        let tree_state = state.tree_state();
        if !tree_state.contains_hash(&hash) && self.provider.header(hash)?.is_none() {
            debug!(target: "engine::tree::payload_validator", %hash, "no canonical state found for block");
            return Ok(None)
        }

        Ok(Some(OverlayStateProviderFactory::new(
            self.provider.clone(),
            hash,
            tree_state.overlay_context.clone(),
        )))
    }

    /// Prepares the optional payload-builder state-root handle through the installed
    /// [`StateRootStrategy`](crate::tree::state_root_strategy::StateRootStrategy).
    pub(super) fn payload_state_root_handle_for(
        &self,
        parent_hash: B256,
        parent_header: &N::BlockHeader,
        timestamp: u64,
        state: &mut EngineApiTreeState<N>,
    ) -> Option<PayloadStateRootHandle> {
        let state_provider_factory = match self.overlay_state_provider_factory(parent_hash, state) {
            Ok(Some(state_provider_factory)) => state_provider_factory,
            Ok(None) => return None,
            Err(err) => {
                warn!(
                    target: "engine::tree::payload_validator",
                    %err,
                    %parent_hash,
                    "failed to prepare payload-builder state-root provider"
                );
                return None
            }
        };
        match self.state_root_strategy.prepare_payload_builder(PayloadStateRootJobContext::new(
            &self.runtime,
            parent_hash,
            parent_header,
            timestamp,
            state,
            state_provider_factory,
            &self.config,
        )) {
            Ok(handle) => handle,
            Err(err) => {
                warn!(
                    target: "engine::tree::payload_validator",
                    %err,
                    %parent_hash,
                    "failed to prepare payload-builder state-root job"
                );
                None
            }
        }
    }

    /// Spawns a background task to compute and sort trie data for the executed block.
    ///
    /// This function creates a [`LazyTrieData`] handle and spawns a blocking task that:
    /// 1. Sort the block's hashed state and trie updates
    /// 2. Publishes the result so subsequent calls return immediately
    ///
    /// If the background task hasn't completed when `trie_data()` is called, callers wait for the
    /// publishing task instead of computing synchronously.
    ///
    /// The validation hot path can return immediately after state root verification,
    /// while consumers (DB writes, overlay providers, proofs) get trie data from the completed
    /// task.
    pub(super) fn spawn_deferred_trie_task(
        &self,
        block: Arc<RecoveredBlock<N::Block>>,
        execution_outcome: Arc<BlockExecutionOutput<N::Receipt>>,
        hashed_state: LazyHashedPostState,
        trie_output: Arc<TrieUpdates>,
    ) -> ExecutedBlock<N> {
        // Create deferred handle and task that owns the unsorted inputs.
        // Resolve the lazy handle into Arc<HashedPostState>. By this point the hashed state has
        // already been computed and used for state root verification, so .get() returns instantly.
        let hashed_state = match hashed_state.try_into_inner() {
            Ok(state) => state,
            Err(handle) => handle.get().clone(),
        };
        let (deferred_trie_data, deferred_trie_task) =
            LazyTrieData::pending(hashed_state, trie_output);
        let block_validation_metrics = self.metrics.block_validation.clone();

        // Capture block info for tracing.
        let block_number = block.number();

        // Spawn background task to compute trie data.
        let compute_trie_input_task = move || {
            let _span = debug_span!(
                target: "engine::tree::payload_validator",
                "compute_trie_input_task",
                block_number
            )
            .entered();

            let compute_start = Instant::now();
            let computed = deferred_trie_task.compute_and_publish();
            block_validation_metrics
                .deferred_trie_compute_duration
                .record(compute_start.elapsed().as_secs_f64());

            // Record sizes of the computed trie data
            block_validation_metrics
                .hashed_post_state_size
                .record(computed.hashed_state.total_len() as f64);
            block_validation_metrics
                .trie_updates_sorted_size
                .record(computed.trie_updates.total_len() as f64);
        };

        // Spawn task that computes trie data asynchronously.
        self.runtime.spawn_blocking_named(DEFERRED_TRIE_WORKER_NAME, compute_trie_input_task);

        ExecutedBlock::with_deferred_trie_data(block, execution_outcome, deferred_trie_data)
    }
}

impl<P, Evm, V> WaitForCaches<Evm::Primitives> for BasicEngineValidator<P, Evm, V>
where
    Evm: ConfigureEvm,
{
    fn wait_for_caches(
        &self,
        overlay_context: &OverlayContext<Evm::Primitives>,
    ) -> CacheWaitDurations {
        debug!(target: "engine::tree::payload_validator", "Waiting for execution cache and sparse trie locks");

        let execution_cache = self.payload_processor.execution_cache();
        let overlay_context = overlay_context.clone();
        let (execution_tx, execution_rx) = std::sync::mpsc::channel();
        let (sparse_trie_tx, sparse_trie_rx) = std::sync::mpsc::channel();

        self.runtime.spawn_blocking_named("wait-exec-cache", move || {
            let _ = execution_tx.send(execution_cache.wait_for_availability());
        });
        self.runtime.spawn_blocking_named("wait-sparse-tri", move || {
            let _ = sparse_trie_tx.send(overlay_context.wait_for_sparse_trie_availability());
        });

        let execution_cache =
            execution_rx.recv().expect("execution cache wait task failed to send result");
        let sparse_trie =
            sparse_trie_rx.recv().expect("sparse trie wait task failed to send result");
        debug!(
            target: "engine::tree::payload_validator",
            ?execution_cache,
            ?sparse_trie,
            "Execution cache and sparse trie locks acquired"
        );
        CacheWaitDurations { execution_cache, sparse_trie }
    }
}
