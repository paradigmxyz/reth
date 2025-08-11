//! Types and traits for validating blocks and payloads.

use crate::tree::{
    cached_state::CachedStateProvider,
    error::{InsertBlockError, InsertBlockErrorKind, InsertPayloadError},
    executor::WorkloadExecutor,
    instrumented_state::InstrumentedStateProvider,
    payload_processor::PayloadProcessor,
    persistence_state::CurrentPersistenceAction,
    precompile_cache::{CachedPrecompile, CachedPrecompileMetrics, PrecompileCacheMap},
    sparse_trie::StateRootComputeOutcome,
    ConsistentDbView, EngineApiMetrics, EngineApiTreeState, ExecutionEnv, PayloadHandle,
    PersistenceState, PersistingKind, StateProviderBuilder, StateProviderDatabase, TreeConfig,
};
use alloy_consensus::transaction::Either;
use alloy_eips::{eip1898::BlockWithParent, NumHash};
use alloy_evm::Evm;
use alloy_primitives::B256;
use reth_chain_state::{
    CanonicalInMemoryState, ExecutedBlock, ExecutedBlockWithTrieUpdates, ExecutedTrieUpdates,
};
use reth_consensus::{ConsensusError, FullConsensus};
use reth_engine_primitives::{
    ConfigureEngineEvm, ExecutableTxIterator, ExecutionPayload, InvalidBlockHook, PayloadValidator,
};
use reth_errors::{BlockExecutionError, ProviderResult};
use reth_evm::{
    block::BlockExecutor, execute::ExecutableTxFor, ConfigureEvm, EvmEnvFor, ExecutionCtxFor,
    SpecFor,
};
use reth_payload_primitives::{
    BuiltPayload, InvalidPayloadAttributesError, NewPayloadError, PayloadTypes,
};
use reth_primitives_traits::{
    AlloyBlockHeader, BlockTy, GotExpected, NodePrimitives, RecoveredBlock, SealedHeader,
};
use reth_provider::{
    BlockExecutionOutput, BlockNumReader, BlockReader, DBProvider, DatabaseProviderFactory,
    ExecutionOutcome, HashedPostStateProvider, ProviderError, StateCommitmentProvider,
    StateProvider, StateProviderFactory, StateReader, StateRootProvider,
};
use reth_revm::db::State;
use reth_trie::{updates::TrieUpdates, HashedPostState, TrieInput};
use reth_trie_db::{DatabaseHashedPostState, StateCommitment};
use reth_trie_parallel::root::{ParallelStateRoot, ParallelStateRootError};
use std::{collections::HashMap, sync::Arc, time::Instant};
use tracing::{debug, error, info, trace, warn};

/// Context providing access to tree state during validation.
///
/// This context is provided to the [`EngineValidator`] and includes the state of the tree's
/// internals
pub struct TreeCtx<'a, N: NodePrimitives> {
    /// The engine API tree state
    state: &'a mut EngineApiTreeState<N>,
    /// Information about the current persistence state
    persistence: &'a PersistenceState,
    /// Reference to the canonical in-memory state
    canonical_in_memory_state: &'a CanonicalInMemoryState<N>,
    /// Whether the currently validated block is on a fork chain.
    is_fork: bool,
}

impl<'a, N: NodePrimitives> std::fmt::Debug for TreeCtx<'a, N> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TreeCtx")
            .field("state", &"EngineApiTreeState")
            .field("persistence_info", &self.persistence)
            .field("canonical_in_memory_state", &self.canonical_in_memory_state)
            .finish()
    }
}

impl<'a, N: NodePrimitives> TreeCtx<'a, N> {
    /// Creates a new tree context
    pub const fn new(
        state: &'a mut EngineApiTreeState<N>,
        persistence: &'a PersistenceState,
        canonical_in_memory_state: &'a CanonicalInMemoryState<N>,
        is_fork: bool,
    ) -> Self {
        Self { state, persistence, canonical_in_memory_state, is_fork }
    }

    /// Returns a reference to the engine tree state
    pub const fn state(&self) -> &EngineApiTreeState<N> {
        &*self.state
    }

    /// Returns a mutable reference to the engine tree state
    pub const fn state_mut(&mut self) -> &mut EngineApiTreeState<N> {
        self.state
    }

    /// Returns a reference to the persistence info
    pub const fn persistence(&self) -> &PersistenceState {
        self.persistence
    }

    /// Returns a reference to the canonical in-memory state
    pub const fn canonical_in_memory_state(&self) -> &'a CanonicalInMemoryState<N> {
        self.canonical_in_memory_state
    }

    /// Returns whether the currently validated block is on a fork chain.
    pub const fn is_fork(&self) -> bool {
        self.is_fork
    }

    /// Determines the persisting kind for the given block based on persistence info.
    ///
    /// Based on the given header it returns whether any conflicting persistence operation is
    /// currently in progress.
    ///
    /// This is adapted from the `persisting_kind_for` method in `EngineApiTreeHandler`.
    pub fn persisting_kind_for(&self, block: BlockWithParent) -> PersistingKind {
        // Check that we're currently persisting.
        let Some(action) = self.persistence().current_action() else {
            return PersistingKind::NotPersisting
        };
        // Check that the persistince action is saving blocks, not removing them.
        let CurrentPersistenceAction::SavingBlocks { highest } = action else {
            return PersistingKind::PersistingNotDescendant
        };

        // The block being validated can only be a descendant if its number is higher than
        // the highest block persisting. Otherwise, it's likely a fork of a lower block.
        if block.block.number > highest.number &&
            self.state().tree_state.is_descendant(*highest, block)
        {
            return PersistingKind::PersistingDescendant
        }

        // In all other cases, the block is not a descendant.
        PersistingKind::PersistingNotDescendant
    }
}

/// A helper type that provides reusable payload validation logic for network-specific validators.
///
/// This type satisfies [`EngineValidator`] and is responsible for executing blocks/payloads.
///
/// This type contains common validation, execution, and state root computation logic that can be
/// used by network-specific payload validators (e.g., Ethereum, Optimism). It is not meant to be
/// used as a standalone component, but rather as a building block for concrete implementations.
#[derive(derive_more::Debug)]
pub struct BasicEngineValidator<P, Evm, V>
where
    Evm: ConfigureEvm,
{
    /// Provider for database access.
    provider: P,
    /// Consensus implementation for validation.
    consensus: Arc<dyn FullConsensus<Evm::Primitives, Error = ConsensusError>>,
    /// EVM configuration.
    evm_config: Evm,
    /// Configuration for the tree.
    config: TreeConfig,
    /// Payload processor for state root computation.
    payload_processor: PayloadProcessor<Evm>,
    /// Precompile cache map.
    precompile_cache_map: PrecompileCacheMap<SpecFor<Evm>>,
    /// Precompile cache metrics.
    precompile_cache_metrics: HashMap<alloy_primitives::Address, CachedPrecompileMetrics>,
    /// Hook to call when invalid blocks are encountered.
    #[debug(skip)]
    invalid_block_hook: Box<dyn InvalidBlockHook<Evm::Primitives>>,
    /// Metrics for the engine api.
    metrics: EngineApiMetrics,
    /// Validator for the payload.
    validator: V,
}

impl<N, P, Evm, V> BasicEngineValidator<P, Evm, V>
where
    N: NodePrimitives,
    P: DatabaseProviderFactory<Provider: BlockReader>
        + BlockReader<Header = N::BlockHeader>
        + StateProviderFactory
        + StateReader
        + StateCommitmentProvider
        + HashedPostStateProvider
        + Clone
        + 'static,
    Evm: ConfigureEvm<Primitives = N> + 'static,
{
    /// Creates a new `TreePayloadValidator`.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        provider: P,
        consensus: Arc<dyn FullConsensus<N, Error = ConsensusError>>,
        evm_config: Evm,
        validator: V,
        config: TreeConfig,
        invalid_block_hook: Box<dyn InvalidBlockHook<N>>,
    ) -> Self {
        let precompile_cache_map = PrecompileCacheMap::default();
        let payload_processor = PayloadProcessor::new(
            WorkloadExecutor::default(),
            evm_config.clone(),
            &config,
            precompile_cache_map.clone(),
        );
        Self {
            provider,
            consensus,
            evm_config,
            payload_processor,
            precompile_cache_map,
            precompile_cache_metrics: HashMap::new(),
            config,
            invalid_block_hook,
            metrics: EngineApiMetrics::default(),
            validator,
        }
    }

    /// Converts a [`BlockOrPayload`] to a recovered block.
    pub fn convert_to_block<T: PayloadTypes<BuiltPayload: BuiltPayload<Primitives = N>>>(
        &self,
        input: BlockOrPayload<T>,
    ) -> Result<RecoveredBlock<N::Block>, NewPayloadError>
    where
        V: PayloadValidator<T, Block = N::Block>,
    {
        match input {
            BlockOrPayload::Payload(payload) => self.validator.ensure_well_formed_payload(payload),
            BlockOrPayload::Block(block) => Ok(block),
        }
    }

    /// Returns EVM environment for the given payload or block.
    pub fn evm_env_for<T: PayloadTypes<BuiltPayload: BuiltPayload<Primitives = N>>>(
        &self,
        input: &BlockOrPayload<T>,
    ) -> EvmEnvFor<Evm>
    where
        V: PayloadValidator<T, Block = N::Block>,
        Evm: ConfigureEngineEvm<T::ExecutionData, Primitives = N>,
    {
        match input {
            BlockOrPayload::Payload(payload) => self.evm_config.evm_env_for_payload(payload),
            BlockOrPayload::Block(block) => self.evm_config.evm_env(block.header()),
        }
    }

    /// Returns [`ExecutableTxIterator`] for the given payload or block.
    pub fn tx_iterator_for<'a, T: PayloadTypes<BuiltPayload: BuiltPayload<Primitives = N>>>(
        &'a self,
        input: &'a BlockOrPayload<T>,
    ) -> Result<impl ExecutableTxIterator<Evm> + 'a, NewPayloadError>
    where
        V: PayloadValidator<T, Block = N::Block>,
        Evm: ConfigureEngineEvm<T::ExecutionData, Primitives = N>,
    {
        match input {
            BlockOrPayload::Payload(payload) => Ok(Either::Left(
                self.evm_config.tx_iterator_for_payload(payload).map(|res| res.map(Either::Left)),
            )),
            BlockOrPayload::Block(block) => {
                let transactions = block.clone_transactions_recovered().collect::<Vec<_>>();
                Ok(Either::Right(transactions.into_iter().map(|tx| Ok(Either::Right(tx)))))
            }
        }
    }

    /// Returns a [`ExecutionCtxFor`] for the given payload or block.
    pub fn execution_ctx_for<'a, T: PayloadTypes<BuiltPayload: BuiltPayload<Primitives = N>>>(
        &self,
        input: &'a BlockOrPayload<T>,
    ) -> ExecutionCtxFor<'a, Evm>
    where
        V: PayloadValidator<T, Block = N::Block>,
        Evm: ConfigureEngineEvm<T::ExecutionData, Primitives = N>,
    {
        match input {
            BlockOrPayload::Payload(payload) => self.evm_config.context_for_payload(payload),
            BlockOrPayload::Block(block) => self.evm_config.context_for_block(block),
        }
    }

    /// Validates a block that has already been converted from a payload.
    ///
    /// This method performs:
    /// - Consensus validation
    /// - Block execution
    /// - State root computation
    /// - Fork detection
    pub fn validate_block_with_state<T: PayloadTypes<BuiltPayload: BuiltPayload<Primitives = N>>>(
        &mut self,
        input: BlockOrPayload<T>,
        mut ctx: TreeCtx<'_, N>,
    ) -> ValidationOutcome<N, InsertPayloadError<N::Block>>
    where
        V: PayloadValidator<T, Block = N::Block>,
        Evm: ConfigureEngineEvm<T::ExecutionData, Primitives = N>,
    {
        /// A helper macro that returns the block in case there was an error
        macro_rules! ensure_ok {
            ($expr:expr) => {
                match $expr {
                    Ok(val) => val,
                    Err(e) => {
                        let block = self.convert_to_block(input)?;
                        return Err(InsertBlockError::new(block.into_sealed_block(), e.into()).into())
                    }
                }
            };
        }

        let parent_hash = input.parent_hash();
        let block_num_hash = input.num_hash();

        trace!(target: "engine::tree", block=?block_num_hash, parent=?parent_hash, "Fetching block state provider");
        let Some(provider_builder) =
            ensure_ok!(self.state_provider_builder(parent_hash, ctx.state()))
        else {
            // this is pre-validated in the tree
            return Err(InsertBlockError::new(
                self.convert_to_block(input)?.into_sealed_block(),
                ProviderError::HeaderNotFound(parent_hash.into()).into(),
            )
            .into())
        };

        let state_provider = ensure_ok!(provider_builder.build());

        // fetch parent block
        let Some(parent_block) = ensure_ok!(self.sealed_header_by_hash(parent_hash, ctx.state()))
        else {
            return Err(InsertBlockError::new(
                self.convert_to_block(input)?.into_sealed_block(),
                ProviderError::HeaderNotFound(parent_hash.into()).into(),
            )
            .into())
        };

        let evm_env = self.evm_env_for(&input);

        let env = ExecutionEnv { evm_env, hash: input.hash(), parent_hash: input.parent_hash() };

        // We only run the parallel state root if we are not currently persisting any blocks or
        // persisting blocks that are all ancestors of the one we are executing.
        //
        // If we're committing ancestor blocks, then: any trie updates being committed are a subset
        // of the in-memory trie updates collected before fetching reverts. So any diff in
        // reverts (pre vs post commit) is already covered by the in-memory trie updates we
        // collect in `compute_state_root_parallel`.
        //
        // See https://github.com/paradigmxyz/reth/issues/12688 for more details
        let persisting_kind = ctx.persisting_kind_for(input.block_with_parent());
        // don't run parallel if state root fallback is set
        let run_parallel_state_root =
            persisting_kind.can_run_parallel_state_root() && !self.config.state_root_fallback();

        // Use state root task only if:
        // 1. No persistence is in progress
        // 2. Config allows it
        // 3. No ancestors with missing trie updates. If any exist, it will mean that every state
        //    root task proof calculation will include a lot of unrelated paths in the prefix sets.
        //    It's cheaper to run a parallel state root that does one walk over trie tables while
        //    accounting for the prefix sets.
        let has_ancestors_with_missing_trie_updates =
            self.has_ancestors_with_missing_trie_updates(input.block_with_parent(), ctx.state());
        let mut use_state_root_task = run_parallel_state_root &&
            self.config.use_state_root_task() &&
            !has_ancestors_with_missing_trie_updates;

        debug!(
            target: "engine::tree",
            block=?block_num_hash,
            run_parallel_state_root,
            has_ancestors_with_missing_trie_updates,
            use_state_root_task,
            config_allows_state_root_task=self.config.use_state_root_task(),
            "Deciding which state root algorithm to run"
        );

        // use prewarming background task
        let txs = self.tx_iterator_for(&input)?;
        let mut handle = if use_state_root_task {
            // use background tasks for state root calc
            let consistent_view =
                ensure_ok!(ConsistentDbView::new_with_latest_tip(self.provider.clone()));

            // get allocated trie input if it exists
            let allocated_trie_input = self.payload_processor.take_trie_input();

            // Compute trie input
            let trie_input_start = Instant::now();
            let trie_input = ensure_ok!(self.compute_trie_input(
                persisting_kind,
                ensure_ok!(consistent_view.provider_ro()),
                parent_hash,
                ctx.state(),
                allocated_trie_input,
            ));

            self.metrics
                .block_validation
                .trie_input_duration
                .record(trie_input_start.elapsed().as_secs_f64());

            // Use state root task only if prefix sets are empty, otherwise proof generation is too
            // expensive because it requires walking over the paths in the prefix set in every
            // proof.
            if trie_input.prefix_sets.is_empty() {
                self.payload_processor.spawn(
                    env.clone(),
                    txs,
                    provider_builder,
                    consistent_view,
                    trie_input,
                    &self.config,
                )
            } else {
                debug!(target: "engine::tree", block=?block_num_hash, "Disabling state root task due to non-empty prefix sets");
                use_state_root_task = false;
                self.payload_processor.spawn_cache_exclusive(env.clone(), txs, provider_builder)
            }
        } else {
            self.payload_processor.spawn_cache_exclusive(env.clone(), txs, provider_builder)
        };

        // Use cached state provider before executing, used in execution after prewarming threads
        // complete
        let state_provider = CachedStateProvider::new_with_caches(
            state_provider,
            handle.caches(),
            handle.cache_metrics(),
        );

        let (output, execution_finish) = if self.config.state_provider_metrics() {
            let state_provider = InstrumentedStateProvider::from_state_provider(&state_provider);
            let (output, execution_finish) =
                ensure_ok!(self.execute_block(&state_provider, env, &input, &mut handle));
            state_provider.record_total_latency();
            (output, execution_finish)
        } else {
            ensure_ok!(self.execute_block(&state_provider, env, &input, &mut handle))
        };

        // after executing the block we can stop executing transactions
        handle.stop_prewarming_execution();

        let block = self.convert_to_block(input)?;

        // A helper macro that returns the block in case there was an error
        macro_rules! ensure_ok {
            ($expr:expr) => {
                match $expr {
                    Ok(val) => val,
                    Err(e) => return Err(InsertBlockError::new(block.into_sealed_block(), e.into()).into()),
                }
            };
        }

        trace!(target: "engine::tree", block=?block_num_hash, "Validating block consensus");
        // validate block consensus rules
        ensure_ok!(self.validate_block_inner(&block));

        // now validate against the parent
        if let Err(e) =
            self.consensus.validate_header_against_parent(block.sealed_header(), &parent_block)
        {
            warn!(target: "engine::tree", ?block, "Failed to validate header {} against parent: {e}", block.hash());
            return Err(InsertBlockError::new(block.into_sealed_block(), e.into()).into())
        }

        if let Err(err) = self.consensus.validate_block_post_execution(&block, &output) {
            // call post-block hook
            self.on_invalid_block(&parent_block, &block, &output, None, ctx.state_mut());
            return Err(InsertBlockError::new(block.into_sealed_block(), err.into()).into())
        }

        let hashed_state = self.provider.hashed_post_state(&output.state);

        if let Err(err) =
            self.validator.validate_block_post_execution_with_hashed_state(&hashed_state, &block)
        {
            // call post-block hook
            self.on_invalid_block(&parent_block, &block, &output, None, ctx.state_mut());
            return Err(InsertBlockError::new(block.into_sealed_block(), err.into()).into())
        }

        debug!(target: "engine::tree", block=?block_num_hash, "Calculating block state root");

        let root_time = Instant::now();

        let mut maybe_state_root = None;

        if run_parallel_state_root {
            // if we new payload extends the current canonical change we attempt to use the
            // background task or try to compute it in parallel
            if use_state_root_task {
                debug!(target: "engine::tree", block=?block_num_hash, "Using sparse trie state root algorithm");
                match handle.state_root() {
                    Ok(StateRootComputeOutcome { state_root, trie_updates }) => {
                        let elapsed = execution_finish.elapsed();
                        info!(target: "engine::tree", ?state_root, ?elapsed, "State root task finished");
                        // we double check the state root here for good measure
                        if state_root == block.header().state_root() {
                            maybe_state_root = Some((state_root, trie_updates, elapsed))
                        } else {
                            warn!(
                                target: "engine::tree",
                                ?state_root,
                                block_state_root = ?block.header().state_root(),
                                "State root task returned incorrect state root"
                            );
                        }
                    }
                    Err(error) => {
                        debug!(target: "engine::tree", %error, "Background parallel state root computation failed");
                    }
                }
            } else {
                debug!(target: "engine::tree", block=?block_num_hash, "Using parallel state root algorithm");
                match self.compute_state_root_parallel(
                    persisting_kind,
                    block.parent_hash(),
                    &hashed_state,
                    ctx.state(),
                ) {
                    Ok(result) => {
                        info!(
                            target: "engine::tree",
                            block = ?block_num_hash,
                            regular_state_root = ?result.0,
                            "Regular root task finished"
                        );
                        maybe_state_root = Some((result.0, result.1, root_time.elapsed()));
                    }
                    Err(ParallelStateRootError::Provider(ProviderError::ConsistentView(error))) => {
                        debug!(target: "engine::tree", %error, "Parallel state root computation failed consistency check, falling back");
                    }
                    Err(error) => {
                        return Err(InsertBlockError::new(
                            block.into_sealed_block(),
                            InsertBlockErrorKind::Other(Box::new(error)),
                        )
                        .into())
                    }
                }
            }
        }

        let (state_root, trie_output, root_elapsed) = if let Some(maybe_state_root) =
            maybe_state_root
        {
            maybe_state_root
        } else {
            // fallback is to compute the state root regularly in sync
            if self.config.state_root_fallback() {
                debug!(target: "engine::tree", block=?block_num_hash, "Using state root fallback for testing");
            } else {
                warn!(target: "engine::tree", block=?block_num_hash, ?persisting_kind, "Failed to compute state root in parallel");
                self.metrics.block_validation.state_root_parallel_fallback_total.increment(1);
            }

            let (root, updates) =
                ensure_ok!(state_provider.state_root_with_updates(hashed_state.clone()));
            (root, updates, root_time.elapsed())
        };

        self.metrics.block_validation.record_state_root(&trie_output, root_elapsed.as_secs_f64());
        debug!(target: "engine::tree", ?root_elapsed, block=?block_num_hash, "Calculated state root");

        // ensure state root matches
        if state_root != block.header().state_root() {
            // call post-block hook
            self.on_invalid_block(
                &parent_block,
                &block,
                &output,
                Some((&trie_output, state_root)),
                ctx.state_mut(),
            );
            let block_state_root = block.header().state_root();
            return Err(InsertBlockError::new(
                block.into_sealed_block(),
                ConsensusError::BodyStateRootDiff(
                    GotExpected { got: state_root, expected: block_state_root }.into(),
                )
                .into(),
            )
            .into())
        }

        // terminate prewarming task with good state output
        handle.terminate_caching(Some(output.state.clone()));

        // If the block is a fork, we don't save the trie updates, because they may be incorrect.
        // Instead, they will be recomputed on persistence.
        let trie_updates = if ctx.is_fork() {
            ExecutedTrieUpdates::Missing
        } else {
            ExecutedTrieUpdates::Present(Arc::new(trie_output))
        };

        Ok(ExecutedBlockWithTrieUpdates {
            block: ExecutedBlock {
                recovered_block: Arc::new(block),
                execution_output: Arc::new(ExecutionOutcome::from((output, block_num_hash.number))),
                hashed_state: Arc::new(hashed_state),
            },
            trie: trie_updates,
        })
    }

    /// Return sealed block from database or in-memory state by hash.
    fn sealed_header_by_hash(
        &self,
        hash: B256,
        state: &EngineApiTreeState<N>,
    ) -> ProviderResult<Option<SealedHeader<N::BlockHeader>>> {
        // check memory first
        let block =
            state.tree_state.block_by_hash(hash).map(|block| block.as_ref().clone_sealed_header());

        if block.is_some() {
            Ok(block)
        } else {
            self.provider.sealed_header_by_hash(hash)
        }
    }

    /// Validate if block is correct and satisfies all the consensus rules that concern the header
    /// and block body itself.
    fn validate_block_inner(&self, block: &RecoveredBlock<N::Block>) -> Result<(), ConsensusError> {
        if let Err(e) = self.consensus.validate_header(block.sealed_header()) {
            error!(target: "engine::tree", ?block, "Failed to validate header {}: {e}", block.hash());
            return Err(e)
        }

        if let Err(e) = self.consensus.validate_block_pre_execution(block.sealed_block()) {
            error!(target: "engine::tree", ?block, "Failed to validate block {}: {e}", block.hash());
            return Err(e)
        }

        Ok(())
    }

    /// Executes a block with the given state provider
    fn execute_block<S, Err, T>(
        &mut self,
        state_provider: S,
        env: ExecutionEnv<Evm>,
        input: &BlockOrPayload<T>,
        handle: &mut PayloadHandle<impl ExecutableTxFor<Evm>, Err>,
    ) -> Result<(BlockExecutionOutput<N::Receipt>, Instant), InsertBlockErrorKind>
    where
        S: StateProvider,
        Err: core::error::Error + Send + Sync + 'static,
        V: PayloadValidator<T, Block = N::Block>,
        T: PayloadTypes<BuiltPayload: BuiltPayload<Primitives = N>>,
        Evm: ConfigureEngineEvm<T::ExecutionData, Primitives = N>,
    {
        let num_hash = NumHash::new(env.evm_env.block_env.number.to(), env.hash);
        debug!(target: "engine::tree", block=?num_hash, "Executing block");
        let mut db = State::builder()
            .with_database(StateProviderDatabase::new(&state_provider))
            .with_bundle_update()
            .without_state_clear()
            .build();

        let evm = self.evm_config.evm_with_env(&mut db, env.evm_env.clone());
        let ctx = self.execution_ctx_for(input);
        let mut executor = self.evm_config.create_executor(evm, ctx);

        if !self.config.precompile_cache_disabled() {
            executor.evm_mut().precompiles_mut().map_precompiles(|address, precompile| {
                let metrics = self
                    .precompile_cache_metrics
                    .entry(*address)
                    .or_insert_with(|| CachedPrecompileMetrics::new_with_address(*address))
                    .clone();
                CachedPrecompile::wrap(
                    precompile,
                    self.precompile_cache_map.cache_for_address(*address),
                    *env.evm_env.spec_id(),
                    Some(metrics),
                )
            });
        }

        let execution_start = Instant::now();
        let state_hook = Box::new(handle.state_hook());
        let output = self.metrics.executor.execute_metered(
            executor,
            handle.iter_transactions().map(|res| res.map_err(BlockExecutionError::other)),
            state_hook,
        )?;
        let execution_finish = Instant::now();
        let execution_time = execution_finish.duration_since(execution_start);
        debug!(target: "engine::tree", elapsed = ?execution_time, number=?num_hash.number, "Executed block");
        Ok((output, execution_finish))
    }

    /// Compute state root for the given hashed post state in parallel.
    ///
    /// # Returns
    ///
    /// Returns `Ok(_)` if computed successfully.
    /// Returns `Err(_)` if error was encountered during computation.
    /// `Err(ProviderError::ConsistentView(_))` can be safely ignored and fallback computation
    /// should be used instead.
    fn compute_state_root_parallel(
        &self,
        persisting_kind: PersistingKind,
        parent_hash: B256,
        hashed_state: &HashedPostState,
        state: &EngineApiTreeState<N>,
    ) -> Result<(B256, TrieUpdates), ParallelStateRootError> {
        let consistent_view = ConsistentDbView::new_with_latest_tip(self.provider.clone())?;

        let mut input = self.compute_trie_input(
            persisting_kind,
            consistent_view.provider_ro()?,
            parent_hash,
            state,
            None,
        )?;
        // Extend with block we are validating root for.
        input.append_ref(hashed_state);

        ParallelStateRoot::new(consistent_view, input).incremental_root_with_updates()
    }

    /// Check if the given block has any ancestors with missing trie updates.
    fn has_ancestors_with_missing_trie_updates(
        &self,
        target_header: BlockWithParent,
        state: &EngineApiTreeState<N>,
    ) -> bool {
        // Walk back through the chain starting from the parent of the target block
        let mut current_hash = target_header.parent;
        while let Some(block) = state.tree_state.blocks_by_hash.get(&current_hash) {
            // Check if this block is missing trie updates
            if block.trie.is_missing() {
                return true;
            }

            // Move to the parent block
            current_hash = block.recovered_block().parent_hash();
        }

        false
    }

    /// Creates a `StateProviderBuilder` for the given parent hash.
    ///
    /// This method checks if the parent is in the tree state (in-memory) or persisted to disk,
    /// and creates the appropriate provider builder.
    fn state_provider_builder(
        &self,
        hash: B256,
        state: &EngineApiTreeState<N>,
    ) -> ProviderResult<Option<StateProviderBuilder<N, P>>> {
        if let Some((historical, blocks)) = state.tree_state.blocks_by_hash(hash) {
            debug!(target: "engine::tree", %hash, %historical, "found canonical state for block in memory, creating provider builder");
            // the block leads back to the canonical chain
            return Ok(Some(StateProviderBuilder::new(
                self.provider.clone(),
                historical,
                Some(blocks),
            )))
        }

        // Check if the block is persisted
        if let Some(header) = self.provider.header(&hash)? {
            debug!(target: "engine::tree", %hash, number = %header.number(), "found canonical state for block in database, creating provider builder");
            // For persisted blocks, we create a builder that will fetch state directly from the
            // database
            return Ok(Some(StateProviderBuilder::new(self.provider.clone(), hash, None)))
        }

        debug!(target: "engine::tree", %hash, "no canonical state found for block");
        Ok(None)
    }

    /// Called when an invalid block is encountered during validation.
    fn on_invalid_block(
        &self,
        parent_header: &SealedHeader<N::BlockHeader>,
        block: &RecoveredBlock<N::Block>,
        output: &BlockExecutionOutput<N::Receipt>,
        trie_updates: Option<(&TrieUpdates, B256)>,
        state: &mut EngineApiTreeState<N>,
    ) {
        if state.invalid_headers.get(&block.hash()).is_some() {
            // we already marked this block as invalid
            return;
        }
        self.invalid_block_hook.on_invalid_block(parent_header, block, output, trie_updates);
    }

    /// Computes the trie input at the provided parent hash.
    ///
    /// The goal of this function is to take in-memory blocks and generate a [`TrieInput`] that
    /// serves as an overlay to the database blocks.
    ///
    /// It works as follows:
    /// 1. Collect in-memory blocks that are descendants of the provided parent hash using
    ///    [`crate::tree::TreeState::blocks_by_hash`].
    /// 2. If the persistence is in progress, and the block that we're computing the trie input for
    ///    is a descendant of the currently persisting blocks, we need to be sure that in-memory
    ///    blocks are not overlapping with the database blocks that may have been already persisted.
    ///    To do that, we're filtering out in-memory blocks that are lower than the highest database
    ///    block.
    /// 3. Once in-memory blocks are collected and optionally filtered, we compute the
    ///    [`HashedPostState`] from them.
    fn compute_trie_input<TP: DBProvider + BlockNumReader>(
        &self,
        persisting_kind: PersistingKind,
        provider: TP,
        parent_hash: B256,
        state: &EngineApiTreeState<N>,
        allocated_trie_input: Option<TrieInput>,
    ) -> ProviderResult<TrieInput> {
        // get allocated trie input or use a default trie input
        let mut input = allocated_trie_input.unwrap_or_default();

        let best_block_number = provider.best_block_number()?;

        let (mut historical, mut blocks) = state
            .tree_state
            .blocks_by_hash(parent_hash)
            .map_or_else(|| (parent_hash.into(), vec![]), |(hash, blocks)| (hash.into(), blocks));

        // If the current block is a descendant of the currently persisting blocks, then we need to
        // filter in-memory blocks, so that none of them are already persisted in the database.
        if persisting_kind.is_descendant() {
            // Iterate over the blocks from oldest to newest.
            while let Some(block) = blocks.last() {
                let recovered_block = block.recovered_block();
                if recovered_block.number() <= best_block_number {
                    // Remove those blocks that lower than or equal to the highest database
                    // block.
                    blocks.pop();
                } else {
                    // If the block is higher than the best block number, stop filtering, as it's
                    // the first block that's not in the database.
                    break
                }
            }

            historical = if let Some(block) = blocks.last() {
                // If there are any in-memory blocks left after filtering, set the anchor to the
                // parent of the oldest block.
                (block.recovered_block().number() - 1).into()
            } else {
                // Otherwise, set the anchor to the original provided parent hash.
                parent_hash.into()
            };
        }

        if blocks.is_empty() {
            debug!(target: "engine::tree", %parent_hash, "Parent found on disk");
        } else {
            debug!(target: "engine::tree", %parent_hash, %historical, blocks = blocks.len(), "Parent found in memory");
        }

        // Convert the historical block to the block number.
        let block_number = provider
            .convert_hash_or_number(historical)?
            .ok_or_else(|| ProviderError::BlockHashNotFound(historical.as_hash().unwrap()))?;

        // Retrieve revert state for historical block.
        let revert_state = if block_number == best_block_number {
            // We do not check against the `last_block_number` here because
            // `HashedPostState::from_reverts` only uses the database tables, and not static files.
            debug!(target: "engine::tree", block_number, best_block_number, "Empty revert state");
            HashedPostState::default()
        } else {
            let revert_state = HashedPostState::from_reverts::<
                <P::StateCommitment as StateCommitment>::KeyHasher,
            >(provider.tx_ref(), block_number + 1)
            .map_err(ProviderError::from)?;
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

        // Extend with contents of parent in-memory blocks.
        input.extend_with_blocks(
            blocks.iter().rev().map(|block| (block.hashed_state(), block.trie_updates())),
        );

        Ok(input)
    }
}

/// Output of block or payload validation.
pub type ValidationOutcome<N, E = InsertPayloadError<BlockTy<N>>> =
    Result<ExecutedBlockWithTrieUpdates<N>, E>;

/// Type that validates the payloads processed by the engine.
///
/// This provides the necessary functions for validating/executing payloads/blocks.
pub trait EngineValidator<
    Types: PayloadTypes,
    N: NodePrimitives = <<Types as PayloadTypes>::BuiltPayload as BuiltPayload>::Primitives,
>: Send + Sync + 'static
{
    /// Validates the payload attributes with respect to the header.
    ///
    /// By default, this enforces that the payload attributes timestamp is greater than the
    /// timestamp according to:
    ///   > 7. Client software MUST ensure that payloadAttributes.timestamp is greater than
    ///   > timestamp
    ///   > of a block referenced by forkchoiceState.headBlockHash.
    ///
    /// See also: <https://github.com/ethereum/execution-apis/blob/647a677b7b97e09145b8d306c0eaf51c32dae256/src/engine/common.md#specification-1>
    fn validate_payload_attributes_against_header(
        &self,
        attr: &Types::PayloadAttributes,
        header: &N::BlockHeader,
    ) -> Result<(), InvalidPayloadAttributesError>;

    /// Ensures that the given payload does not violate any consensus rules that concern the block's
    /// layout.
    ///
    /// This function must convert the payload into the executable block and pre-validate its
    /// fields.
    ///
    /// Implementers should ensure that the checks are done in the order that conforms with the
    /// engine-API specification.
    fn ensure_well_formed_payload(
        &self,
        payload: Types::ExecutionData,
    ) -> Result<RecoveredBlock<N::Block>, NewPayloadError>;

    /// Validates a payload received from engine API.
    fn validate_payload(
        &mut self,
        payload: Types::ExecutionData,
        ctx: TreeCtx<'_, N>,
    ) -> ValidationOutcome<N>;

    /// Validates a block downloaded from the network.
    fn validate_block(
        &mut self,
        block: RecoveredBlock<N::Block>,
        ctx: TreeCtx<'_, N>,
    ) -> ValidationOutcome<N>;
}

impl<N, Types, P, Evm, V> EngineValidator<Types> for BasicEngineValidator<P, Evm, V>
where
    P: DatabaseProviderFactory<Provider: BlockReader>
        + BlockReader<Header = N::BlockHeader>
        + StateProviderFactory
        + StateReader
        + StateCommitmentProvider
        + HashedPostStateProvider
        + Clone
        + 'static,
    N: NodePrimitives,
    V: PayloadValidator<Types, Block = N::Block>,
    Evm: ConfigureEngineEvm<Types::ExecutionData, Primitives = N> + 'static,
    Types: PayloadTypes<BuiltPayload: BuiltPayload<Primitives = N>>,
{
    fn validate_payload_attributes_against_header(
        &self,
        attr: &Types::PayloadAttributes,
        header: &N::BlockHeader,
    ) -> Result<(), InvalidPayloadAttributesError> {
        self.validator.validate_payload_attributes_against_header(attr, header)
    }

    fn ensure_well_formed_payload(
        &self,
        payload: Types::ExecutionData,
    ) -> Result<RecoveredBlock<N::Block>, NewPayloadError> {
        let block = self.validator.ensure_well_formed_payload(payload)?;
        Ok(block)
    }

    fn validate_payload(
        &mut self,
        payload: Types::ExecutionData,
        ctx: TreeCtx<'_, N>,
    ) -> ValidationOutcome<N> {
        self.validate_block_with_state(BlockOrPayload::Payload(payload), ctx)
    }

    fn validate_block(
        &mut self,
        block: RecoveredBlock<N::Block>,
        ctx: TreeCtx<'_, N>,
    ) -> ValidationOutcome<N> {
        self.validate_block_with_state(BlockOrPayload::Block(block), ctx)
    }
}

/// Enum representing either block or payload being validated.
#[derive(Debug)]
pub enum BlockOrPayload<T: PayloadTypes> {
    /// Payload.
    Payload(T::ExecutionData),
    /// Block.
    Block(RecoveredBlock<BlockTy<<T::BuiltPayload as BuiltPayload>::Primitives>>),
}

impl<T: PayloadTypes> BlockOrPayload<T> {
    /// Returns the hash of the block.
    pub fn hash(&self) -> B256 {
        match self {
            Self::Payload(payload) => payload.block_hash(),
            Self::Block(block) => block.hash(),
        }
    }

    /// Returns the number and hash of the block.
    pub fn num_hash(&self) -> NumHash {
        match self {
            Self::Payload(payload) => payload.num_hash(),
            Self::Block(block) => block.num_hash(),
        }
    }

    /// Returns the parent hash of the block.
    pub fn parent_hash(&self) -> B256 {
        match self {
            Self::Payload(payload) => payload.parent_hash(),
            Self::Block(block) => block.parent_hash(),
        }
    }

    /// Returns [`BlockWithParent`] for the block.
    pub fn block_with_parent(&self) -> BlockWithParent {
        match self {
            Self::Payload(payload) => payload.block_with_parent(),
            Self::Block(block) => block.block_with_parent(),
        }
    }
}
