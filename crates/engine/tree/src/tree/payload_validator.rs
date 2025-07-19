//! Concrete implementation of the `PayloadValidator` trait.

use crate::tree::{
    cached_state::CachedStateProvider,
    instrumented_state::InstrumentedStateProvider,
    payload_processor::PayloadProcessor,
    precompile_cache::{CachedPrecompile, CachedPrecompileMetrics, PrecompileCacheMap},
    ConsistentDbView, EngineApiMetrics, EngineApiTreeState, InvalidHeaderCache, PersistingKind,
    StateProviderDatabase, TreeConfig,
};
use alloy_eips::eip2718::Decodable2718;
use alloy_evm::{block::BlockExecutor, Evm};
use alloy_primitives::B256;
use alloy_rpc_types_engine::ExecutionData;
use reth_chain_state::CanonicalInMemoryState;
use reth_consensus::{ConsensusError, FullConsensus};
use reth_engine_primitives::{InvalidBlockHook, PayloadValidationOutcome, TreeCtx};
use reth_evm::{ConfigureEvm, SpecFor};
use reth_payload_primitives::NewPayloadError;
use reth_primitives_traits::{
    AlloyBlockHeader, Block, BlockBody, GotExpected, NodePrimitives, RecoveredBlock, SealedHeader,
};
use reth_provider::{
    BlockExecutionOutput, BlockNumReader, BlockReader, DBProvider,
    DatabaseProviderFactory, HashedPostStateProvider, HeaderProvider, ProviderError,
    ProviderResult, StateCommitmentProvider, StateProvider, StateProviderFactory, StateReader,
};
use reth_revm::db::State;
use reth_trie::{updates::TrieUpdates, HashedPostState, TrieInput};
use reth_trie_db::{DatabaseHashedPostState, StateCommitment};
use reth_trie_parallel::root::{ParallelStateRoot, ParallelStateRootError};
use std::{
    collections::HashMap,
    sync::Arc,
    time::Instant,
};
use tracing::{debug, trace};

/// A helper type that provides reusable payload validation logic for network-specific validators.
///
/// This type contains common validation, execution, and state root computation logic that can be
/// used by network-specific payload validators (e.g., Ethereum, Optimism). It is not meant to be
/// used as a standalone component, but rather as a building block for concrete implementations.
#[allow(dead_code)]
pub(crate) struct TreePayloadValidator<N, P, C>
where
    N: NodePrimitives,
    P: DatabaseProviderFactory<Provider: BlockReader + BlockNumReader + HeaderProvider>
        + BlockReader
        + BlockNumReader
        + StateProviderFactory
        + StateReader
        + StateCommitmentProvider
        + HashedPostStateProvider
        + HeaderProvider<Header = N::BlockHeader>
        + Clone
        + 'static,
    C: ConfigureEvm<Primitives = N> + 'static,
{
    /// Provider for database access.
    provider: P,
    /// Consensus implementation for validation.
    consensus: Arc<dyn FullConsensus<N, Error = ConsensusError>>,
    /// EVM configuration.
    evm_config: C,
    /// Configuration for the tree.
    config: TreeConfig,
    /// Payload processor for state root computation.
    payload_processor: PayloadProcessor<N, C>,
    /// Precompile cache map.
    precompile_cache_map: PrecompileCacheMap<SpecFor<C>>,
    /// Precompile cache metrics.
    precompile_cache_metrics: HashMap<alloy_primitives::Address, CachedPrecompileMetrics>,
    /// Tracks invalid headers to prevent duplicate hook calls.
    invalid_headers: InvalidHeaderCache,
    /// Hook to call when invalid blocks are encountered.
    invalid_block_hook: Box<dyn InvalidBlockHook<N>>,
    /// Metrics for the engine api.
    metrics: EngineApiMetrics,
}

#[allow(dead_code)]
impl<N, P, C> TreePayloadValidator<N, P, C>
where
    N: NodePrimitives,
    P: DatabaseProviderFactory<Provider: BlockReader + BlockNumReader + HeaderProvider>
        + BlockReader
        + BlockNumReader
        + StateProviderFactory
        + StateReader
        + StateCommitmentProvider
        + HashedPostStateProvider
        + HeaderProvider<Header = N::BlockHeader>
        + Clone
        + 'static,
    C: ConfigureEvm<Primitives = N> + 'static,
{
    /// Creates a new `TreePayloadValidator`.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        provider: P,
        consensus: Arc<dyn FullConsensus<N, Error = ConsensusError>>,
        evm_config: C,
        config: TreeConfig,
        payload_processor: PayloadProcessor<N, C>,
        precompile_cache_map: PrecompileCacheMap<SpecFor<C>>,
        invalid_headers_cache_size: u32,
        invalid_block_hook: Box<dyn InvalidBlockHook<N>>,
        metrics: EngineApiMetrics,
    ) -> Self {
        Self {
            provider,
            consensus,
            evm_config,
            config,
            payload_processor,
            precompile_cache_map,
            precompile_cache_metrics: HashMap::new(),
            invalid_headers: InvalidHeaderCache::new(invalid_headers_cache_size),
            invalid_block_hook,
            metrics,
        }
    }

    /// Ensures the payload is well-formed and can be converted to a recovered block.
    ///
    /// This method is intended to be used by network-specific validators as part of their
    /// payload validation flow.
    pub(crate) fn ensure_well_formed_payload(
        &self,
        _payload: ExecutionData,
    ) -> Result<RecoveredBlock<N::Block>, NewPayloadError>
    where
        N::Block: Block<Body: BlockBody<Transaction = N::SignedTx>>
            + From<alloy_consensus::Block<N::SignedTx>>,
        N::SignedTx: Decodable2718,
    {
        Err(NewPayloadError::Other("Not implemented yet".to_string().into()))
    }

    /// Validates a payload and returns the validation outcome.
    ///
    /// This method performs full payload validation including:
    /// - Block conversion and consensus validation
    /// - Block execution
    /// - State root computation
    /// - Fork detection
    ///
    /// This method is intended to be used by network-specific validators as part of their
    /// payload validation flow.
    pub(crate) fn validate_payload(
        &mut self,
        payload: ExecutionData,
        ctx: TreeCtx<'_, EngineApiTreeState<N>, N>,
    ) -> Result<PayloadValidationOutcome<N::Block>, NewPayloadError>
    where
        N::Block: Block<Body: BlockBody<Transaction = N::SignedTx>>
            + From<alloy_consensus::Block<N::SignedTx>>,
        N::SignedTx: Decodable2718,
    {
        // First, convert the payload to a block
        let block = self.payload_to_block(payload)?;

        // Helper macro to preserve block context when returning errors
        macro_rules! ensure_ok {
            ($expr:expr) => {
                match $expr {
                    Ok(val) => val,
                    Err(e) => {
                        let error = NewPayloadError::Other(Box::new(e));
                        return Ok(PayloadValidationOutcome::Invalid { block, error });
                    }
                }
            };
        }

        // Extract references we need before moving ctx
        let tree_state = ctx.state();
        let persistence_info = *ctx.persistence_info();
        let canonical_in_memory_state = ctx.canonical_in_memory_state();

        // Then validate the block using the validate_block method
        if let Err(consensus_error) = self.validate_block(&block, ctx) {
            trace!(target: "engine::tree", block=?block.num_hash(), ?consensus_error, "Block validation failed");
            let payload_error = NewPayloadError::Other(Box::new(consensus_error));
            return Ok(PayloadValidationOutcome::Invalid { block, error: payload_error });
        }

        // Get the parent block's state to execute against
        let parent_hash = block.header().parent_hash();

        // Get parent header for error context
        let parent_header = ensure_ok!(self.get_parent_header(parent_hash, tree_state));

        // Create StateProviderBuilder
        let provider_builder = if let Some((historical, blocks)) =
            tree_state.tree_state.blocks_by_hash(parent_hash)
        {
            // Parent is in memory, create builder with overlay
            crate::tree::StateProviderBuilder::new(self.provider.clone(), historical, Some(blocks))
        } else {
            // Parent is not in memory, check if it's persisted
            match self.provider.header(&parent_hash) {
                Ok(Some(_)) => {
                    // Parent is persisted, create builder without overlay
                    crate::tree::StateProviderBuilder::new(self.provider.clone(), parent_hash, None)
                }
                Ok(None) => {
                    let error = NewPayloadError::Other(Box::new(ProviderError::HeaderNotFound(
                        parent_hash.into(),
                    )));
                    return Ok(PayloadValidationOutcome::Invalid { block, error });
                }
                Err(e) => {
                    let error = NewPayloadError::Other(Box::new(e));
                    return Ok(PayloadValidationOutcome::Invalid { block, error });
                }
            }
        };

        // Determine persisting kind and state root task decision early for handle creation
        let persisting_kind =
            self.persisting_kind_for(block.header(), &persistence_info, tree_state);
        let run_parallel_state_root =
            persisting_kind.can_run_parallel_state_root() && !self.config.state_root_fallback();
        let has_ancestors_with_missing_trie_updates =
            self.has_ancestors_with_missing_trie_updates(block.sealed_header(), tree_state);
        let use_state_root_task = run_parallel_state_root &&
            self.config.use_state_root_task() &&
            !has_ancestors_with_missing_trie_updates;

        // Build the state provider
        let state_provider = ensure_ok!(provider_builder.build());

        // Create a PayloadHandle for state hook support
        let (mut handle, use_state_root_task) = self.create_payload_handle(
            &block,
            provider_builder,
            use_state_root_task,
            parent_hash,
            tree_state,
            &persistence_info,
        );

        // Execute the block with proper state provider wrapping
        let (output, execution_time) = match self.execute_block_with_state_provider(
            state_provider,
            &block,
            &handle,
        ) {
            Ok(result) => result,
            Err(error) => {
                trace!(target: "engine::tree", block=?block.num_hash(), ?error, "Block execution failed");
                return Ok(PayloadValidationOutcome::Invalid { block, error });
            }
        };

        debug!(target: "engine::tree", block=?block.num_hash(), ?execution_time, "Block executed");

        // Stop prewarming after execution
        handle.stop_prewarming_execution();

        // Perform post-execution validation
        if let Err(consensus_error) = self.consensus.validate_block_post_execution(&block, &output)
        {
            trace!(target: "engine::tree", block=?block.num_hash(), ?consensus_error, "Post-execution validation failed");
            let error = NewPayloadError::Other(Box::new(consensus_error));
            return Ok(PayloadValidationOutcome::Invalid { block, error });
        }

        // Compute hashed post state
        let hashed_state = self.provider.hashed_post_state(&output.state);

        debug!(target: "engine::tree", block=?block.num_hash(), "Calculating block state root");

        debug!(
            target: "engine::tree",
            block=?block.num_hash(),
            ?persisting_kind,
            run_parallel_state_root,
            has_ancestors_with_missing_trie_updates,
            use_state_root_task,
            config_allows_state_root_task=self.config.use_state_root_task(),
            "Deciding which state root algorithm to run"
        );

        let state_root_start = Instant::now();
        let (state_root, trie_updates) = match self.compute_state_root_with_strategy(
            &block,
            parent_hash,
            &hashed_state,
            tree_state,
            persisting_kind,
            run_parallel_state_root,
            use_state_root_task,
            &mut handle,
            execution_time,
        ) {
            Ok(result) => result,
            Err(error) => return Ok(PayloadValidationOutcome::Invalid { block, error }),
        };

        let state_root_elapsed = state_root_start.elapsed();
        self.metrics
            .block_validation
            .record_state_root(&trie_updates, state_root_elapsed.as_secs_f64());

        debug!(target: "engine::tree", ?state_root, ?state_root_elapsed, block=?block.num_hash(), "Calculated state root");

        // Ensure state root matches
        if state_root != block.header().state_root() {
            // call post-block hook
            self.on_invalid_block(
                &parent_header,
                &block,
                &output,
                Some((&trie_updates, state_root)),
            );
            let error = NewPayloadError::Other(Box::new(ConsensusError::BodyStateRootDiff(
                GotExpected { got: state_root, expected: block.header().state_root() }.into(),
            )));
            return Ok(PayloadValidationOutcome::Invalid { block, error });
        }

        // Check if the block is a fork
        let is_fork =
            ensure_ok!(self.is_fork(block.sealed_header(), tree_state, canonical_in_memory_state));

        debug!(target: "engine::tree", block=?block.num_hash(), is_fork, "Fork detection complete");

        Ok(PayloadValidationOutcome::Valid { block, trie_updates, is_fork })
    }

    /// Validates a block according to consensus rules.
    ///
    /// This method performs:
    /// - Header validation
    /// - Pre-execution validation
    /// - Parent header validation
    ///
    /// This method is intended to be used by network-specific validators as part of their
    /// block validation flow.
    pub(crate) fn validate_block(
        &self,
        block: &RecoveredBlock<N::Block>,
        ctx: TreeCtx<'_, EngineApiTreeState<N>, N>,
    ) -> Result<(), ConsensusError>
    where
        N::Block: Block,
    {
        let block_num_hash = block.num_hash();
        debug!(target: "engine::tree", block=?block_num_hash, parent = ?block.header().parent_hash(), "Validating downloaded block");

        // Validate block consensus rules
        trace!(target: "engine::tree", block=?block_num_hash, "Validating block header");
        self.consensus.validate_header(block.sealed_header())?;

        trace!(target: "engine::tree", block=?block_num_hash, "Validating block pre-execution");
        self.consensus.validate_block_pre_execution(block)?;

        // Get parent header for validation
        let parent_hash = block.header().parent_hash();
        let parent_header =
            if let Some(parent_block) = ctx.state().tree_state.executed_block_by_hash(parent_hash) {
                parent_block.block.recovered_block.sealed_header().clone()
            } else {
                // Fallback to database if not in tree state
                let header: N::BlockHeader = self
                    .provider
                    .header(&parent_hash)
                    .map_err(|e| ConsensusError::Other(e.to_string()))?
                    .ok_or_else(|| {
                        ConsensusError::Other(format!("Parent header not found: {parent_hash}"))
                    })?;
                SealedHeader::seal_slow(header)
            };

        // Validate against parent
        trace!(target: "engine::tree", block=?block_num_hash, "Validating block against parent");
        self.consensus.validate_header_against_parent(block.sealed_header(), &parent_header)?;

        debug!(target: "engine::tree", block=?block_num_hash, "Block validation complete");
        Ok(())
    }

    /// Converts an execution payload to a recovered block.
    fn payload_to_block(
        &self,
        payload: ExecutionData,
    ) -> Result<RecoveredBlock<N::Block>, NewPayloadError>
    where
        N::Block: Block<Body: BlockBody<Transaction = N::SignedTx>>
            + From<alloy_consensus::Block<N::SignedTx>>,
        N::SignedTx: Decodable2718,
    {
        let ExecutionData { payload, sidecar } = payload;

        let expected_hash = payload.block_hash();

        // Parse the block from the payload
        let alloy_block: alloy_consensus::Block<N::SignedTx> =
            payload.try_into_block_with_sidecar(&sidecar).map_err(NewPayloadError::Eth)?;

        // Convert to N::Block
        let block: N::Block = alloy_block.into();

        // Seal the block
        let sealed_block = block.seal_slow();

        // Ensure the hash included in the payload matches the block hash
        if expected_hash != sealed_block.hash() {
            return Err(NewPayloadError::Eth(alloy_rpc_types_engine::PayloadError::BlockHash {
                execution: sealed_block.hash(),
                consensus: expected_hash,
            }));
        }

        // Recover senders for the block
        let recovered_block = sealed_block
            .try_recover()
            .map_err(|_| NewPayloadError::Other("Failed to recover senders".to_string().into()))?;

        Ok(recovered_block)
    }

    /// Executes the given block using the provided state provider.
    fn execute_block<S>(
        &mut self,
        state_provider: &S,
        block: &RecoveredBlock<N::Block>,
        handle: &crate::tree::PayloadHandle,
    ) -> Result<(BlockExecutionOutput<N::Receipt>, Instant), NewPayloadError>
    where
        S: StateProvider,
        N::Block: Block<Body: BlockBody<Transaction = N::SignedTx>>,
    {
        trace!(target: "engine::tree", block = ?block.num_hash(), "Executing block");

        // Create state database
        let mut db = State::builder()
            .with_database(StateProviderDatabase::new(state_provider))
            .with_bundle_update()
            .without_state_clear()
            .build();

        // Configure executor for the block
        let mut executor = self.evm_config.executor_for_block(&mut db, block);

        // Configure precompile caching if enabled
        if !self.config.precompile_cache_disabled() {
            // Get the spec id before the closure
            let spec_id = *self.evm_config.evm_env(block.header()).spec_id();

            executor.evm_mut().precompiles_mut().map_precompiles(|address, precompile| {
                let metrics = self
                    .precompile_cache_metrics
                    .entry(*address)
                    .or_insert_with(|| CachedPrecompileMetrics::new_with_address(*address))
                    .clone();
                let cache = self.precompile_cache_map.cache_for_address(*address);
                CachedPrecompile::wrap(precompile, cache, spec_id, Some(metrics))
            });
        }

        // Execute the block
        let start = Instant::now();
        let output = self
            .metrics
            .executor
            .execute_metered(executor, block, Box::new(handle.state_hook()))
            .map_err(|e| NewPayloadError::Other(Box::new(e)))?;

        Ok((output, start))
    }

    /// Executes a block with proper state provider wrapping and optional instrumentation.
    ///
    /// This method wraps the base state provider with:
    /// 1. `CachedStateProvider` for cache support
    /// 2. `InstrumentedStateProvider` for metrics (if enabled)
    fn execute_block_with_state_provider<S>(
        &mut self,
        state_provider: S,
        block: &RecoveredBlock<N::Block>,
        handle: &crate::tree::PayloadHandle,
    ) -> Result<(BlockExecutionOutput<N::Receipt>, Instant), NewPayloadError>
    where
        S: StateProvider,
        N::Block: Block<Body: BlockBody<Transaction = N::SignedTx>>,
    {
        // Wrap state provider with cached state provider for execution
        let cached_state_provider = CachedStateProvider::new_with_caches(
            state_provider,
            handle.caches(),
            handle.cache_metrics(),
        );

        // Execute the block with optional instrumentation
        if self.config.state_provider_metrics() {
            let instrumented_provider =
                InstrumentedStateProvider::from_state_provider(&cached_state_provider);
            let result = self.execute_block(&instrumented_provider, block, handle);
            instrumented_provider.record_total_latency();
            result
        } else {
            self.execute_block(&cached_state_provider, block, handle)
        }
    }

    /// Computes the state root for the given block.
    ///
    /// This method attempts to compute the state root in parallel if configured and conditions
    /// allow, otherwise falls back to synchronous computation.
    fn compute_state_root(
        &self,
        parent_hash: B256,
        hashed_state: &HashedPostState,
    ) -> Result<(B256, TrieUpdates), NewPayloadError>
    {
        // Get the state provider for the parent block
        let state_provider = self
            .provider
            .history_by_block_hash(parent_hash)
            .map_err(|e| NewPayloadError::Other(Box::new(e)))?;

        // Compute the state root with trie updates
        let (state_root, trie_updates) = state_provider
            .state_root_with_updates(hashed_state.clone())
            .map_err(|e| NewPayloadError::Other(Box::new(e)))?;

        Ok((state_root, trie_updates))
    }

    /// Attempts to get the state root from the background task.
    fn try_state_root_from_task(
        &self,
        handle: &mut crate::tree::PayloadHandle,
        block: &RecoveredBlock<N::Block>,
        execution_time: Instant,
    ) -> Option<(B256, TrieUpdates)>
    where
        N::Block: Block,
    {
        match handle.state_root() {
            Ok(crate::tree::payload_processor::sparse_trie::StateRootComputeOutcome {
                state_root,
                trie_updates,
            }) => {
                let elapsed = execution_time.elapsed();
                debug!(target: "engine::tree", ?state_root, ?elapsed, "State root task finished");

                // Double check the state root matches what we expect
                if state_root == block.header().state_root() {
                    Some((state_root, trie_updates))
                } else {
                    debug!(
                        target: "engine::tree",
                        ?state_root,
                        block_state_root = ?block.header().state_root(),
                        "State root task returned incorrect state root"
                    );
                    None
                }
            }
            Err(error) => {
                debug!(target: "engine::tree", %error, "Background state root computation failed");
                None
            }
        }
    }

    /// Computes state root with appropriate strategy based on configuration.
    #[allow(clippy::too_many_arguments)]
    fn compute_state_root_with_strategy(
        &self,
        block: &RecoveredBlock<N::Block>,
        parent_hash: B256,
        hashed_state: &HashedPostState,
        tree_state: &EngineApiTreeState<N>,
        persisting_kind: PersistingKind,
        run_parallel_state_root: bool,
        use_state_root_task: bool,
        handle: &mut crate::tree::PayloadHandle,
        execution_time: Instant,
    ) -> Result<(B256, TrieUpdates), NewPayloadError>
    where
        N::Block: Block,
    {
        if !run_parallel_state_root {
            // Use synchronous computation
            return self.compute_state_root(parent_hash, hashed_state);
        }

        // Parallel state root is enabled
        if use_state_root_task {
            debug!(target: "engine::tree", block=?block.num_hash(), "Using sparse trie state root algorithm");

            // Try to get state root from background task first
            if let Some((state_root, trie_updates)) =
                self.try_state_root_from_task(handle, block, execution_time)
            {
                return Ok((state_root, trie_updates));
            }

            // Background task failed or returned incorrect root, fall back to parallel
            debug!(target: "engine::tree", "Falling back to parallel state root computation");
        } else {
            debug!(target: "engine::tree", block=?block.num_hash(), "Using parallel state root algorithm");
        }

        // Try parallel computation
        match self.compute_state_root_parallel(
            parent_hash,
            hashed_state,
            tree_state,
            persisting_kind,
        ) {
            Ok(result) => Ok(result),
            Err(ParallelStateRootError::Provider(ProviderError::ConsistentView(error))) => {
                debug!(target: "engine::tree", %error, "Parallel state root computation failed consistency check, falling back to synchronous");
                self.metrics.block_validation.state_root_parallel_fallback_total.increment(1);
                self.compute_state_root(parent_hash, hashed_state)
            }
            Err(error) => Err(NewPayloadError::Other(Box::new(error))),
        }
    }

    /// Computes state root in parallel.
    ///
    /// # Returns
    ///
    /// Returns `Ok(_)` if computed successfully.
    /// Returns `Err(_)` if error was encountered during computation.
    /// `Err(ProviderError::ConsistentView(_))` can be safely ignored and fallback computation
    /// should be used instead.
    fn compute_state_root_parallel(
        &self,
        parent_hash: B256,
        hashed_state: &HashedPostState,
        tree_state: &EngineApiTreeState<N>,
        persisting_kind: PersistingKind,
    ) -> Result<(B256, TrieUpdates), ParallelStateRootError>
    {
        let consistent_view = ConsistentDbView::new_with_latest_tip(self.provider.clone())?;

        // Compute trie input using the tree state
        let mut input = self.compute_trie_input(
            consistent_view.provider_ro()?,
            parent_hash,
            tree_state,
            persisting_kind,
        )?;

        // Extend with block we are validating root for
        input.append_ref(hashed_state);

        ParallelStateRoot::new(consistent_view, input).incremental_root_with_updates()
    }

    /// Check if the given block has any ancestors with missing trie updates.
    ///
    /// This walks back through the chain starting from the parent of the target block
    /// and checks if any ancestor blocks are missing trie updates.
    fn has_ancestors_with_missing_trie_updates(
        &self,
        target_header: &SealedHeader<N::BlockHeader>,
        tree_state: &EngineApiTreeState<N>,
    ) -> bool {
        // Walk back through the chain starting from the parent of the target block
        let mut current_hash = target_header.parent_hash();
        while let Some(block) = tree_state.tree_state.executed_block_by_hash(current_hash) {
            // Check if this block is missing trie updates
            if block.trie.is_missing() {
                return true;
            }

            // Move to the parent block
            current_hash = block.block.recovered_block.parent_hash();
        }

        false
    }

    /// Determines the persisting kind for the given block based on persistence info.
    ///
    /// This is adapted from the `persisting_kind_for` method in `EngineApiTreeHandler`.
    fn persisting_kind_for(
        &self,
        block: &N::BlockHeader,
        persistence_info: &reth_engine_primitives::PersistenceInfo,
        tree_state: &EngineApiTreeState<N>,
    ) -> PersistingKind {
        use reth_engine_primitives::PersistenceAction;

        // Check that we're currently persisting
        let Some(action) = &persistence_info.current_action else {
            return PersistingKind::NotPersisting;
        };

        // Check that the persistence action is saving blocks, not removing them
        let PersistenceAction::SavingBlocks { highest } = action else {
            return PersistingKind::PersistingNotDescendant;
        };

        // The block being validated can only be a descendant if its number is higher than
        // the highest block persisting. Otherwise, it's likely a fork of a lower block.
        if block.number() > highest.number && tree_state.tree_state.is_descendant(*highest, block) {
            PersistingKind::PersistingDescendant
        } else {
            PersistingKind::PersistingNotDescendant
        }
    }

    /// Creates a payload handle for the given block.
    ///
    /// This method decides whether to use full spawn (with background state root tasks)
    /// or cache-only spawn based on the current conditions.
    ///
    /// Returns a tuple of (`PayloadHandle`, `use_state_root_task`) where `use_state_root_task`
    /// indicates whether the state root task was actually enabled (it may be disabled
    /// if prefix sets are non-empty).
    fn create_payload_handle(
        &mut self,
        block: &RecoveredBlock<N::Block>,
        provider_builder: crate::tree::StateProviderBuilder<N, P>,
        use_state_root_task: bool,
        parent_hash: B256,
        tree_state: &EngineApiTreeState<N>,
        persistence_info: &reth_engine_primitives::PersistenceInfo,
    ) -> (crate::tree::PayloadHandle, bool)
    where
        N::Block: Block<Body: BlockBody<Transaction = N::SignedTx>>,
    {
        let header = block.clone_sealed_header();
        let txs = block.clone_transactions_recovered().collect();

        if !use_state_root_task {
            // Use cache-only spawn when state root tasks are not needed
            let handle = self.payload_processor.spawn_cache_exclusive(
                header,
                txs,
                provider_builder,
            );
            return (handle, false);
        }

        // Try to use full spawn with background state root computation support
        let Ok(consistent_view) = ConsistentDbView::new_with_latest_tip(self.provider.clone())
        else {
            // Fall back to cache-only spawn if consistent view fails
            let handle = self.payload_processor.spawn_cache_exclusive(
                header,
                txs,
                provider_builder,
            );
            return (handle, false);
        };

        let Ok(provider_ro) = consistent_view.provider_ro() else {
            // Fall back to cache-only spawn if provider creation fails
            let handle = self.payload_processor.spawn_cache_exclusive(
                header,
                txs,
                provider_builder,
            );
            return (handle, false);
        };

        // For the handle creation, we need to determine persisting kind again
        // This could be optimized by passing it from validate_payload
        let persisting_kind =
            self.persisting_kind_for(block.header(), persistence_info, tree_state);

        let trie_input_start = Instant::now();
        let Ok(trie_input) =
            self.compute_trie_input(provider_ro, parent_hash, tree_state, persisting_kind)
        else {
            // Fall back to cache-only spawn if trie input computation fails
            let handle = self.payload_processor.spawn_cache_exclusive(
                header,
                txs,
                provider_builder,
            );
            return (handle, false);
        };
        let trie_input_elapsed = trie_input_start.elapsed();
        self.metrics.block_validation.trie_input_duration.record(trie_input_elapsed.as_secs_f64());

        // Use state root task only if prefix sets are empty, otherwise proof generation is too
        // expensive because it requires walking over the paths in the prefix set in every
        // proof.
        if trie_input.prefix_sets.is_empty() {
            let handle = self.payload_processor.spawn(
                header,
                txs,
                provider_builder,
                consistent_view,
                trie_input,
                &self.config,
            );
            (handle, true)
        } else {
            debug!(target: "engine::tree", block=?block.num_hash(), "Disabling state root task due to non-empty prefix sets");
            let handle = self.payload_processor.spawn_cache_exclusive(
                header,
                txs,
                provider_builder,
            );
            (handle, false)
        }
    }

    /// Retrieves the parent header from tree state or database.
    fn get_parent_header(
        &self,
        parent_hash: B256,
        tree_state: &EngineApiTreeState<N>,
    ) -> Result<SealedHeader<N::BlockHeader>, ProviderError>
    {
        // First try to get from tree state
        if let Some(parent_block) = tree_state.tree_state.executed_block_by_hash(parent_hash) {
            Ok(parent_block.block.recovered_block.sealed_header().clone())
        } else {
            // Fallback to database
            let header = self
                .provider
                .header(&parent_hash)?
                .ok_or_else(|| ProviderError::HeaderNotFound(parent_hash.into()))?;
            Ok(SealedHeader::seal_slow(header))
        }
    }

    /// Called when an invalid block is encountered during validation.
    fn on_invalid_block(
        &mut self,
        parent_header: &SealedHeader<N::BlockHeader>,
        block: &RecoveredBlock<N::Block>,
        output: &BlockExecutionOutput<N::Receipt>,
        trie_updates: Option<(&TrieUpdates, B256)>,
    ) {
        if self.invalid_headers.get(&block.hash()).is_some() {
            // we already marked this block as invalid
            return;
        }
        self.invalid_block_hook.on_invalid_block(parent_header, block, output, trie_updates);
    }

    /// Return sealed block header from database or in-memory state by hash.
    fn sealed_header_by_hash(
        &self,
        hash: B256,
        tree_state: &EngineApiTreeState<N>,
    ) -> ProviderResult<Option<SealedHeader<N::BlockHeader>>>
    {
        // check memory first
        let block = tree_state
            .tree_state
            .block_by_hash(hash)
            .map(|block| block.as_ref().clone_sealed_header());
        if block.is_some() {
            Ok(block)
        } else {
            self.provider.sealed_header_by_hash(hash)
        }
    }

    /// Checks if the given block is a fork by verifying that it's not part of the canonical chain.
    ///
    /// A block is considered a fork if:
    /// 1. It's not part of an extension of the canonical chain (by walking back to canonical head)
    /// 2. It's not already in the canonical in-memory state
    /// 3. It's not already persisted in the canonical chain
    fn is_fork(
        &self,
        target_header: &SealedHeader<N::BlockHeader>,
        tree_state: &EngineApiTreeState<N>,
        canonical_in_memory_state: &CanonicalInMemoryState<N>,
    ) -> Result<bool, ProviderError>
    {
        let target_hash = target_header.hash();

        // Verify that the given hash is not part of an extension of the canon chain.
        let canonical_head = tree_state.tree_state.canonical_head();
        let mut current_hash;
        let mut current_block = std::borrow::Cow::Borrowed(target_header);

        loop {
            if current_block.hash() == canonical_head.hash {
                return Ok(false);
            }
            // We already passed the canonical head
            if current_block.number() <= canonical_head.number {
                break;
            }
            current_hash = current_block.parent_hash();
            let Some(next_block) = self.sealed_header_by_hash(current_hash, tree_state)? else {
                break
            };
            current_block = std::borrow::Cow::Owned(next_block);
        }

        // Verify that the given hash is not already part of canonical chain stored in memory
        if canonical_in_memory_state.header_by_hash(target_hash).is_some() {
            return Ok(false);
        }

        // Verify that the given hash is not already part of persisted canonical chain
        if self.provider.block_number(target_hash)?.is_some() {
            return Ok(false);
        }

        Ok(true)
    }

    /// Computes the trie input at the provided parent hash.
    fn compute_trie_input<TP>(
        &self,
        provider: TP,
        parent_hash: B256,
        tree_state: &EngineApiTreeState<N>,
        persisting_kind: PersistingKind,
    ) -> Result<TrieInput, ParallelStateRootError>
    where
        TP: DBProvider + BlockNumReader,
    {
        let mut input = TrieInput::default();

        let best_block_number =
            provider.best_block_number().map_err(ParallelStateRootError::Provider)?;

        // Get blocks from tree state
        let (historical, mut blocks) = tree_state
            .tree_state
            .blocks_by_hash(parent_hash)
            .map_or_else(|| (parent_hash.into(), vec![]), |(hash, blocks)| (hash.into(), blocks));

        // Filter blocks based on persisting kind
        if matches!(persisting_kind, PersistingKind::PersistingDescendant) {
            // If we are persisting a descendant, filter out upto the last persisted block
            let last_persisted_block_number = provider
                .convert_hash_or_number(historical)
                .map_err(ParallelStateRootError::Provider)?
                .ok_or_else(|| {
                    ParallelStateRootError::Provider(ProviderError::BlockHashNotFound(
                        historical.as_hash().unwrap(),
                    ))
                })?;

            blocks.retain(|b| b.recovered_block().number() > last_persisted_block_number);
        }

        if blocks.is_empty() {
            debug!(target: "engine::tree", %parent_hash, "Parent found on disk");
        } else {
            debug!(target: "engine::tree", %parent_hash, %historical, blocks = blocks.len(), "Parent found in memory");
        }

        // Convert the historical block to the block number
        let block_number = provider
            .convert_hash_or_number(historical)
            .map_err(ParallelStateRootError::Provider)?
            .ok_or_else(|| {
                ParallelStateRootError::Provider(ProviderError::BlockHashNotFound(
                    historical.as_hash().unwrap(),
                ))
            })?;

        // Retrieve revert state for historical block
        let revert_state = if block_number == best_block_number {
            // No revert state needed if we're at the best block
            debug!(target: "engine::tree", block_number, best_block_number, "Empty revert state");
            HashedPostState::default()
        } else {
            let revert_state = HashedPostState::from_reverts::<
                <P::StateCommitment as StateCommitment>::KeyHasher,
            >(provider.tx_ref(), block_number + 1)
            .map_err(|e| ParallelStateRootError::Provider(ProviderError::from(e)))?;
            debug!(
                target: "engine::tree",
                block_number,
                best_block_number,
                accounts = revert_state.accounts.len(),
                storages = revert_state.storages.len(),
                "Non-empty revert state"
            );
            revert_state
        };
        input.append(revert_state);

        // Extend with contents of parent in-memory blocks
        input.extend_with_blocks(
            blocks.iter().rev().map(|block| (block.hashed_state(), block.trie_updates())),
        );

        Ok(input)
    }
}
