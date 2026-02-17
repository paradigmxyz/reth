//! Types and traits for validating blocks and payloads.

use crate::tree::{
    cached_state::CachedStateProvider,
    error::{InsertBlockError, InsertBlockErrorKind, InsertPayloadError},
    instrumented_state::InstrumentedStateProvider,
    payload_processor::PayloadProcessor,
    precompile_cache::{CachedPrecompile, CachedPrecompileMetrics, PrecompileCacheMap},
    sparse_trie::StateRootComputeOutcome,
    CacheWaitDurations, EngineApiMetrics, EngineApiTreeState, ExecutionEnv, PayloadHandle,
    StateProviderBuilder, StateProviderDatabase, TreeConfig, WaitForCaches,
};
use alloy_consensus::transaction::{Either, TxHashRef};
use alloy_eip7928::BlockAccessList;
use alloy_eips::{eip1898::BlockWithParent, eip4895::Withdrawal, NumHash};
use alloy_evm::Evm;
use alloy_primitives::B256;
use alloy_rlp::Decodable;

use crate::tree::payload_processor::receipt_root_task::{IndexedReceipt, ReceiptRootTaskHandle};
use reth_chain_state::{CanonicalInMemoryState, DeferredTrieData, ExecutedBlock, LazyOverlay};
use reth_consensus::{ConsensusError, FullConsensus, ReceiptRootBloom};
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
    AlloyBlockHeader, BlockBody, BlockTy, FastInstant as Instant, GotExpected, NodePrimitives,
    RecoveredBlock, SealedBlock, SealedHeader, SignerRecoverable,
};
use reth_provider::{
    providers::OverlayStateProviderFactory, BlockExecutionOutput, BlockNumReader, BlockReader,
    ChangeSetReader, DatabaseProviderFactory, DatabaseProviderROFactory, HashedPostStateProvider,
    ProviderError, PruneCheckpointReader, StageCheckpointReader, StateProvider,
    StateProviderFactory, StateReader, StorageChangeSetReader, StorageSettingsCache,
};
use reth_revm::db::{states::bundle_state::BundleRetention, State};
use reth_trie::{updates::TrieUpdates, HashedPostState, StateRoot};
use reth_trie_db::ChangesetCache;
use reth_trie_parallel::root::{ParallelStateRoot, ParallelStateRootError};
use revm_primitives::Address;
use std::{
    collections::HashMap,
    panic::{self, AssertUnwindSafe},
    sync::{mpsc::RecvTimeoutError, Arc},
};
use tracing::{debug, debug_span, error, info, instrument, trace, warn};

/// Context providing access to tree state during validation.
///
/// This context is provided to the [`EngineValidator`] and includes the state of the tree's
/// internals
pub struct TreeCtx<'a, N: NodePrimitives> {
    /// The engine API tree state
    state: &'a mut EngineApiTreeState<N>,
    /// Reference to the canonical in-memory state
    canonical_in_memory_state: &'a CanonicalInMemoryState<N>,
}

impl<'a, N: NodePrimitives> std::fmt::Debug for TreeCtx<'a, N> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TreeCtx")
            .field("state", &"EngineApiTreeState")
            .field("canonical_in_memory_state", &self.canonical_in_memory_state)
            .finish()
    }
}

impl<'a, N: NodePrimitives> TreeCtx<'a, N> {
    /// Creates a new tree context
    pub const fn new(
        state: &'a mut EngineApiTreeState<N>,
        canonical_in_memory_state: &'a CanonicalInMemoryState<N>,
    ) -> Self {
        Self { state, canonical_in_memory_state }
    }

    /// Returns a reference to the engine tree state
    pub const fn state(&self) -> &EngineApiTreeState<N> {
        &*self.state
    }

    /// Returns a mutable reference to the engine tree state
    pub const fn state_mut(&mut self) -> &mut EngineApiTreeState<N> {
        self.state
    }

    /// Returns a reference to the canonical in-memory state
    pub const fn canonical_in_memory_state(&self) -> &'a CanonicalInMemoryState<N> {
        self.canonical_in_memory_state
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
    consensus: Arc<dyn FullConsensus<Evm::Primitives>>,
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
    /// Changeset cache for in-memory trie changesets
    changeset_cache: ChangesetCache,
    /// Task runtime for spawning parallel work.
    runtime: reth_tasks::Runtime,
}

impl<N, P, Evm, V> BasicEngineValidator<P, Evm, V>
where
    N: NodePrimitives,
    P: DatabaseProviderFactory<
            Provider: BlockReader
                          + StageCheckpointReader
                          + PruneCheckpointReader
                          + ChangeSetReader
                          + StorageChangeSetReader
                          + BlockNumReader
                          + StorageSettingsCache,
        > + BlockReader<Header = N::BlockHeader>
        + ChangeSetReader
        + BlockNumReader
        + StateProviderFactory
        + StateReader
        + HashedPostStateProvider
        + Clone
        + 'static,
    Evm: ConfigureEvm<Primitives = N> + 'static,
{
    /// Creates a new `TreePayloadValidator`.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        provider: P,
        consensus: Arc<dyn FullConsensus<N>>,
        evm_config: Evm,
        validator: V,
        config: TreeConfig,
        invalid_block_hook: Box<dyn InvalidBlockHook<N>>,
        changeset_cache: ChangesetCache,
        runtime: reth_tasks::Runtime,
    ) -> Self {
        let precompile_cache_map = PrecompileCacheMap::default();
        let payload_processor = PayloadProcessor::new(
            runtime.clone(),
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
            changeset_cache,
            runtime,
        }
    }

    /// Converts a [`BlockOrPayload`] to a recovered block.
    #[instrument(level = "debug", target = "engine::tree::payload_validator", skip_all)]
    pub fn convert_to_block<T: PayloadTypes<BuiltPayload: BuiltPayload<Primitives = N>>>(
        &self,
        input: BlockOrPayload<T>,
    ) -> Result<SealedBlock<N::Block>, NewPayloadError>
    where
        V: PayloadValidator<T, Block = N::Block>,
    {
        match input {
            BlockOrPayload::Payload(payload) => self.validator.convert_payload_to_block(payload),
            BlockOrPayload::Block(block) => Ok(block),
        }
    }

    /// Returns EVM environment for the given payload or block.
    pub fn evm_env_for<T: PayloadTypes<BuiltPayload: BuiltPayload<Primitives = N>>>(
        &self,
        input: &BlockOrPayload<T>,
    ) -> Result<EvmEnvFor<Evm>, Evm::Error>
    where
        V: PayloadValidator<T, Block = N::Block>,
        Evm: ConfigureEngineEvm<T::ExecutionData, Primitives = N>,
    {
        match input {
            BlockOrPayload::Payload(payload) => Ok(self.evm_config.evm_env_for_payload(payload)?),
            BlockOrPayload::Block(block) => Ok(self.evm_config.evm_env(block.header())?),
        }
    }

    /// Returns [`ExecutableTxIterator`] for the given payload or block.
    pub fn tx_iterator_for<'a, T: PayloadTypes<BuiltPayload: BuiltPayload<Primitives = N>>>(
        &'a self,
        input: &'a BlockOrPayload<T>,
    ) -> Result<impl ExecutableTxIterator<Evm>, NewPayloadError>
    where
        V: PayloadValidator<T, Block = N::Block>,
        Evm: ConfigureEngineEvm<T::ExecutionData, Primitives = N>,
    {
        Ok(match input {
            BlockOrPayload::Payload(payload) => {
                let iter = self
                    .evm_config
                    .tx_iterator_for_payload(payload)
                    .map_err(NewPayloadError::other)?;
                Either::Left(iter)
            }
            BlockOrPayload::Block(block) => {
                let txs = block.body().clone_transactions();
                let convert = |tx: N::SignedTx| tx.try_into_recovered();
                Either::Right((txs, convert))
            }
        })
    }

    /// Returns a [`ExecutionCtxFor`] for the given payload or block.
    pub fn execution_ctx_for<'a, T: PayloadTypes<BuiltPayload: BuiltPayload<Primitives = N>>>(
        &self,
        input: &'a BlockOrPayload<T>,
    ) -> Result<ExecutionCtxFor<'a, Evm>, Evm::Error>
    where
        V: PayloadValidator<T, Block = N::Block>,
        Evm: ConfigureEngineEvm<T::ExecutionData, Primitives = N>,
    {
        match input {
            BlockOrPayload::Payload(payload) => Ok(self.evm_config.context_for_payload(payload)?),
            BlockOrPayload::Block(block) => Ok(self.evm_config.context_for_block(block)?),
        }
    }

    /// Handles execution errors by checking if header validation errors should take precedence.
    ///
    /// When an execution error occurs, this function checks if there are any header validation
    /// errors that should be reported instead, as header validation errors have higher priority.
    fn handle_execution_error<T: PayloadTypes<BuiltPayload: BuiltPayload<Primitives = N>>>(
        &self,
        input: BlockOrPayload<T>,
        execution_err: InsertBlockErrorKind,
        parent_block: &SealedHeader<N::BlockHeader>,
    ) -> Result<ExecutedBlock<N>, InsertPayloadError<N::Block>>
    where
        V: PayloadValidator<T, Block = N::Block>,
    {
        debug!(
            target: "engine::tree::payload_validator",
            ?execution_err,
            block = ?input.num_hash(),
            "Block execution failed, checking for header validation errors"
        );

        // If execution failed, we should first check if there are any header validation
        // errors that take precedence over the execution error
        let block = self.convert_to_block(input)?;

        // Validate block consensus rules which includes header validation
        if let Err(consensus_err) = self.validate_block_inner(&block) {
            // Header validation error takes precedence over execution error
            return Err(InsertBlockError::new(block, consensus_err.into()).into())
        }

        // Also validate against the parent
        if let Err(consensus_err) =
            self.consensus.validate_header_against_parent(block.sealed_header(), parent_block)
        {
            // Parent validation error takes precedence over execution error
            return Err(InsertBlockError::new(block, consensus_err.into()).into())
        }

        // No header validation errors, return the original execution error
        Err(InsertBlockError::new(block, execution_err).into())
    }

    /// Validates a block that has already been converted from a payload.
    ///
    /// This method performs:
    /// - Consensus validation
    /// - Block execution
    /// - State root computation
    /// - Fork detection
    #[instrument(
        level = "debug",
        target = "engine::tree::payload_validator",
        skip_all,
        fields(
            parent = ?input.parent_hash(),
            type_name = ?input.type_name(),
        )
    )]
    pub fn validate_block_with_state<T: PayloadTypes<BuiltPayload: BuiltPayload<Primitives = N>>>(
        &mut self,
        input: BlockOrPayload<T>,
        mut ctx: TreeCtx<'_, N>,
    ) -> ValidationOutcome<N, InsertPayloadError<N::Block>>
    where
        V: PayloadValidator<T, Block = N::Block> + Clone,
        Evm: ConfigureEngineEvm<T::ExecutionData, Primitives = N>,
    {
        // Spawn block conversion on a background thread so it runs concurrently with the
        // rest of the function (setup + execution). For payloads this overlaps the cost of
        // RLP decoding + header hashing; for already-converted blocks this is a no-op.
        let convert_to_block = match &input {
            BlockOrPayload::Payload(_) => {
                let payload_clone = input.clone();
                let validator = self.validator.clone();
                let (tx, rx) = tokio::sync::oneshot::channel();
                self.payload_processor.executor().spawn_blocking(move || {
                    let BlockOrPayload::Payload(payload) = payload_clone else { unreachable!() };
                    let _ = tx.send(validator.convert_payload_to_block(payload));
                });
                Either::Left(rx)
            }
            BlockOrPayload::Block(_) => Either::Right(()),
        };

        // Returns the sealed block, either by awaiting the background conversion task (for
        // payloads) or by extracting the already-converted block directly.
        let convert_to_block =
            move |input: BlockOrPayload<T>| -> Result<SealedBlock<N::Block>, NewPayloadError> {
                match convert_to_block {
                    Either::Left(rx) => rx.blocking_recv().map_err(|_| {
                        NewPayloadError::Other("block conversion task panicked".into())
                    })?,
                    Either::Right(()) => {
                        let BlockOrPayload::Block(block) = input else { unreachable!() };
                        Ok(block)
                    }
                }
            };

        /// A helper macro that returns the block in case there was an error
        /// This macro is used for early returns before block conversion
        macro_rules! ensure_ok {
            ($expr:expr) => {
                match $expr {
                    Ok(val) => val,
                    Err(e) => {
                        let block = convert_to_block(input)?;
                        return Err(InsertBlockError::new(block, e.into()).into())
                    }
                }
            };
        }

        /// A helper macro for handling errors after the input has been converted to a block
        macro_rules! ensure_ok_post_block {
            ($expr:expr, $block:expr) => {
                match $expr {
                    Ok(val) => val,
                    Err(e) => {
                        return Err(
                            InsertBlockError::new($block.into_sealed_block(), e.into()).into()
                        )
                    }
                }
            };
        }

        let parent_hash = input.parent_hash();

        trace!(target: "engine::tree::payload_validator", "Fetching block state provider");
        let _enter =
            debug_span!(target: "engine::tree::payload_validator", "state provider").entered();
        let Some(provider_builder) =
            ensure_ok!(self.state_provider_builder(parent_hash, ctx.state()))
        else {
            // this is pre-validated in the tree
            return Err(InsertBlockError::new(
                convert_to_block(input)?,
                ProviderError::HeaderNotFound(parent_hash.into()).into(),
            )
            .into())
        };
        let mut state_provider = ensure_ok!(provider_builder.build());
        drop(_enter);

        // Fetch parent block. This goes to memory most of the time unless the parent block is
        // beyond the in-memory buffer.
        let Some(parent_block) = ensure_ok!(self.sealed_header_by_hash(parent_hash, ctx.state()))
        else {
            return Err(InsertBlockError::new(
                convert_to_block(input)?,
                ProviderError::HeaderNotFound(parent_hash.into()).into(),
            )
            .into())
        };

        let evm_env = debug_span!(target: "engine::tree::payload_validator", "evm env")
            .in_scope(|| self.evm_env_for(&input))
            .map_err(NewPayloadError::other)?;

        let env = ExecutionEnv {
            evm_env,
            hash: input.hash(),
            parent_hash: input.parent_hash(),
            parent_state_root: parent_block.state_root(),
            transaction_count: input.transaction_count(),
            gas_used: input.gas_used(),
            withdrawals: input.withdrawals().map(|w| w.to_vec()),
        };

        // Plan the strategy used for state root computation.
        let strategy = self.plan_state_root_computation();

        debug!(
            target: "engine::tree::payload_validator",
            ?strategy,
            "Decided which state root algorithm to run"
        );

        // Get an iterator over the transactions in the payload
        let txs = self.tx_iterator_for(&input)?;

        // Extract the BAL, if valid and available
        let block_access_list = ensure_ok!(input
            .block_access_list()
            .transpose()
            // Eventually gets converted to a `InsertBlockErrorKind::Other`
            .map_err(Box::<dyn std::error::Error + Send + Sync>::from))
        .map(Arc::new);

        // Create lazy overlay from ancestors - this doesn't block, allowing execution to start
        // before the trie data is ready. The overlay will be computed on first access.
        let (lazy_overlay, anchor_hash) = Self::get_parent_lazy_overlay(parent_hash, ctx.state());

        // Create overlay factory for payload processor (StateRootTask path needs it for
        // multiproofs)
        let overlay_factory =
            OverlayStateProviderFactory::new(self.provider.clone(), self.changeset_cache.clone())
                .with_block_hash(Some(anchor_hash))
                .with_lazy_overlay(lazy_overlay);

        // Spawn the appropriate processor based on strategy
        let mut handle = ensure_ok!(self.spawn_payload_processor(
            env.clone(),
            txs,
            provider_builder,
            overlay_factory.clone(),
            strategy,
            block_access_list,
        ));

        // Use cached state provider before executing, used in execution after prewarming threads
        // complete
        if let Some((caches, cache_metrics)) = handle.caches().zip(handle.cache_metrics()) {
            state_provider =
                Box::new(CachedStateProvider::new(state_provider, caches, cache_metrics));
        };

        if self.config.state_provider_metrics() {
            state_provider = Box::new(InstrumentedStateProvider::new(state_provider, "engine"));
        }

        // Execute the block and handle any execution errors.
        // The receipt root task is spawned before execution and receives receipts incrementally
        // as transactions complete, allowing parallel computation during execution.
        let (output, senders, receipt_root_rx) =
            match self.execute_block(state_provider, env, &input, &mut handle) {
                Ok(output) => output,
                Err(err) => return self.handle_execution_error(input, err, &parent_block),
            };

        // After executing the block we can stop prewarming transactions
        handle.stop_prewarming_execution();

        // Create ExecutionOutcome early so we can terminate caching before validation and state
        // root computation. Using Arc allows sharing with both the caching task and the deferred
        // trie task without cloning the expensive BundleState.
        let output = Arc::new(output);

        // Terminate caching task early since execution is complete and caching is no longer
        // needed. This frees up resources while state root computation continues.
        let valid_block_tx = handle.terminate_caching(Some(output.clone()));

        let block = convert_to_block(input)?.with_senders(senders);

        // Wait for the receipt root computation to complete.
        let receipt_root_bloom = receipt_root_rx
            .blocking_recv()
            .inspect_err(|_| {
                tracing::error!(
                    target: "engine::tree::payload_validator",
                    "Receipt root task dropped sender without result, receipt root calculation likely aborted"
                );
            })
            .ok();

        let hashed_state = ensure_ok_post_block!(
            self.validate_post_execution(
                &block,
                &parent_block,
                &output,
                &mut ctx,
                receipt_root_bloom
            ),
            block
        );

        let root_time = Instant::now();
        let mut maybe_state_root = None;
        let mut state_root_task_failed = false;

        match strategy {
            StateRootStrategy::StateRootTask => {
                debug!(target: "engine::tree::payload_validator", "Using sparse trie state root algorithm");

                let task_result = ensure_ok_post_block!(
                    self.await_state_root_with_timeout(
                        &mut handle,
                        overlay_factory.clone(),
                        &hashed_state,
                    ),
                    block
                );

                match task_result {
                    Ok(StateRootComputeOutcome { state_root, trie_updates }) => {
                        let elapsed = root_time.elapsed();
                        info!(target: "engine::tree::payload_validator", ?state_root, ?elapsed, "State root task finished");

                        // Compare trie updates with serial computation if configured
                        if self.config.always_compare_trie_updates() {
                            self.compare_trie_updates_with_serial(
                                overlay_factory.clone(),
                                &hashed_state,
                                trie_updates.clone(),
                            );
                        }

                        // we double check the state root here for good measure
                        if state_root == block.header().state_root() {
                            maybe_state_root = Some((state_root, trie_updates, elapsed))
                        } else {
                            warn!(
                                target: "engine::tree::payload_validator",
                                ?state_root,
                                block_state_root = ?block.header().state_root(),
                                "State root task returned incorrect state root"
                            );
                            state_root_task_failed = true;
                        }
                    }
                    Err(error) => {
                        debug!(target: "engine::tree::payload_validator", %error, "State root task failed");
                        state_root_task_failed = true;
                    }
                }
            }
            StateRootStrategy::Parallel => {
                debug!(target: "engine::tree::payload_validator", "Using parallel state root algorithm");
                match self.compute_state_root_parallel(overlay_factory.clone(), &hashed_state) {
                    Ok(result) => {
                        let elapsed = root_time.elapsed();
                        info!(
                            target: "engine::tree::payload_validator",
                            regular_state_root = ?result.0,
                            ?elapsed,
                            "Regular root task finished"
                        );
                        maybe_state_root = Some((result.0, result.1, elapsed));
                    }
                    Err(error) => {
                        debug!(target: "engine::tree::payload_validator", %error, "Parallel state root computation failed");
                    }
                }
            }
            StateRootStrategy::Synchronous => {}
        }

        // Determine the state root.
        // If the state root was computed in parallel, we use it.
        // Otherwise, we fall back to computing it synchronously.
        let (state_root, trie_output, root_elapsed) = if let Some(maybe_state_root) =
            maybe_state_root
        {
            maybe_state_root
        } else {
            // fallback is to compute the state root regularly in sync
            if self.config.state_root_fallback() {
                debug!(target: "engine::tree::payload_validator", "Using state root fallback for testing");
            } else {
                warn!(target: "engine::tree::payload_validator", "Failed to compute state root in parallel");
                self.metrics.block_validation.state_root_parallel_fallback_total.increment(1);
            }

            let (root, updates) = ensure_ok_post_block!(
                Self::compute_state_root_serial(overlay_factory.clone(), &hashed_state),
                block
            );

            if state_root_task_failed {
                self.metrics.block_validation.state_root_task_fallback_success_total.increment(1);
            }

            (root, updates, root_time.elapsed())
        };

        self.metrics.block_validation.record_state_root(&trie_output, root_elapsed.as_secs_f64());
        self.metrics
            .record_state_root_gas_bucket(block.header().gas_used(), root_elapsed.as_secs_f64());
        debug!(target: "engine::tree::payload_validator", ?root_elapsed, "Calculated state root");

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

        if let Some(valid_block_tx) = valid_block_tx {
            let _ = valid_block_tx.send(());
        }

        Ok(self.spawn_deferred_trie_task(
            block,
            output,
            &ctx,
            hashed_state,
            trie_output,
            overlay_factory,
        ))
    }

    /// Return sealed block header from database or in-memory state by hash.
    fn sealed_header_by_hash(
        &self,
        hash: B256,
        state: &EngineApiTreeState<N>,
    ) -> ProviderResult<Option<SealedHeader<N::BlockHeader>>> {
        // check memory first
        let header = state.tree_state.sealed_header_by_hash(&hash);

        if header.is_some() {
            Ok(header)
        } else {
            self.provider.sealed_header_by_hash(hash)
        }
    }

    /// Validate if block is correct and satisfies all the consensus rules that concern the header
    /// and block body itself.
    #[instrument(level = "debug", target = "engine::tree::payload_validator", skip_all)]
    fn validate_block_inner(&self, block: &SealedBlock<N::Block>) -> Result<(), ConsensusError> {
        if let Err(e) = self.consensus.validate_header(block.sealed_header()) {
            error!(target: "engine::tree::payload_validator", ?block, "Failed to validate header {}: {e}", block.hash());
            return Err(e)
        }

        if let Err(e) = self.consensus.validate_block_pre_execution(block) {
            error!(target: "engine::tree::payload_validator", ?block, "Failed to validate block {}: {e}", block.hash());
            return Err(e)
        }

        Ok(())
    }

    /// Executes a block with the given state provider.
    ///
    /// This method orchestrates block execution:
    /// 1. Sets up the EVM with state database and precompile caching
    /// 2. Spawns a background task for incremental receipt root computation
    /// 3. Executes transactions with metrics collection via state hooks
    /// 4. Merges state transitions and records execution metrics
    #[instrument(level = "debug", target = "engine::tree::payload_validator", skip_all)]
    #[expect(clippy::type_complexity)]
    fn execute_block<S, Err, T>(
        &mut self,
        state_provider: S,
        env: ExecutionEnv<Evm>,
        input: &BlockOrPayload<T>,
        handle: &mut PayloadHandle<impl ExecutableTxFor<Evm>, Err, N::Receipt>,
    ) -> Result<
        (
            BlockExecutionOutput<N::Receipt>,
            Vec<Address>,
            tokio::sync::oneshot::Receiver<(B256, alloy_primitives::Bloom)>,
        ),
        InsertBlockErrorKind,
    >
    where
        S: StateProvider + Send,
        Err: core::error::Error + Send + Sync + 'static,
        V: PayloadValidator<T, Block = N::Block>,
        T: PayloadTypes<
            BuiltPayload: BuiltPayload<Primitives = N>,
            ExecutionData: ExecutionPayload,
        >,
        Evm: ConfigureEngineEvm<T::ExecutionData, Primitives = N>,
    {
        debug!(target: "engine::tree::payload_validator", "Executing block");

        // Enable BAL builder if block access list is present in the payload
        let has_bal = input.block_access_list().is_some();
        let mut db = State::builder()
            .with_database(StateProviderDatabase::new(state_provider))
            .with_bundle_update()
            .without_state_clear()
            .with_bal_builder_if(has_bal)
            .build();

        let spec_id = *env.evm_env.spec_id();
        let evm = self.evm_config.evm_with_env(&mut db, env.evm_env);
        let ctx =
            self.execution_ctx_for(input).map_err(|e| InsertBlockErrorKind::Other(Box::new(e)))?;
        let mut executor = self.evm_config.create_executor(evm, ctx);

        if !self.config.precompile_cache_disabled() {
            // Only cache pure precompiles to avoid issues with stateful precompiles
            executor.evm_mut().precompiles_mut().map_pure_precompiles(|address, precompile| {
                let metrics = self
                    .precompile_cache_metrics
                    .entry(*address)
                    .or_insert_with(|| CachedPrecompileMetrics::new_with_address(*address))
                    .clone();
                CachedPrecompile::wrap(
                    precompile,
                    self.precompile_cache_map.cache_for_address(*address),
                    spec_id,
                    Some(metrics),
                )
            });
        }

        // Spawn background task to compute receipt root and logs bloom incrementally.
        // Unbounded channel is used since tx count bounds capacity anyway (max ~30k txs per block).
        let receipts_len = input.transaction_count();
        let (receipt_tx, receipt_rx) = crossbeam_channel::unbounded();
        let (result_tx, result_rx) = tokio::sync::oneshot::channel();
        let task_handle = ReceiptRootTaskHandle::new(receipt_rx, result_tx);
        self.payload_processor.executor().spawn_blocking(move || task_handle.run(receipts_len));

        let transaction_count = input.transaction_count();
        let executor = executor.with_state_hook(Some(Box::new(handle.state_hook())));

        let execution_start = Instant::now();

        // Execute all transactions and finalize
        let (executor, senders) = self.execute_transactions(
            executor,
            transaction_count,
            handle.iter_transactions(),
            &receipt_tx,
        )?;
        drop(receipt_tx);

        // Finish execution and get the result
        let post_exec_start = Instant::now();
        let (_evm, result) = debug_span!(target: "engine::tree", "finish")
            .in_scope(|| executor.finish())
            .map(|(evm, result)| (evm.into_db(), result))?;
        self.metrics.record_post_execution(post_exec_start.elapsed());

        // Merge transitions into bundle state
        debug_span!(target: "engine::tree", "merge transitions")
            .in_scope(|| db.merge_transitions(BundleRetention::Reverts));

        // Validate BAL hash if we executed with BAL tracking
        if has_bal {
            // Get the expected BAL from input and the built BAL from execution
            let expected_bal =
                input.block_access_list().transpose().map_err(BlockExecutionError::other)?;
            let built_bal = db.take_built_alloy_bal();

            // Compute hashes and compare
            let expected_hash = expected_bal
                .as_ref()
                .map(|bal| alloy_eips::eip7928::compute_block_access_list_hash(bal));
            let built_hash = built_bal
                .as_ref()
                .map(|bal| alloy_eips::eip7928::compute_block_access_list_hash(bal));

            if let (Some(expected), Some(got)) = (expected_hash, built_hash) &&
                expected != got
            {
                return Err(InsertBlockErrorKind::Consensus(
                    ConsensusError::BlockAccessListHashMismatch((got, expected).into()),
                ));
            }
        }

        let output = BlockExecutionOutput { result, state: db.take_bundle() };

        let execution_duration = execution_start.elapsed();
        self.metrics.record_block_execution(&output, execution_duration);
        self.metrics.record_block_execution_gas_bucket(output.result.gas_used, execution_duration);

        debug!(target: "engine::tree::payload_validator", elapsed = ?execution_duration, "Executed block");
        Ok((output, senders, result_rx))
    }

    /// Executes transactions and collects senders, streaming receipts to a background task.
    ///
    /// This method handles:
    /// - Applying pre-execution changes (e.g., beacon root updates)
    /// - Executing each transaction with timing metrics
    /// - Streaming receipts to the receipt root computation task
    /// - Collecting transaction senders for later use
    /// - Bumping BAL index after each transaction when BAL tracking is enabled
    ///
    /// Returns the executor (for finalization) and the collected senders.
    fn execute_transactions<'a, E, Tx, InnerTx, Err, DB>(
        &self,
        mut executor: E,
        transaction_count: usize,
        transactions: impl Iterator<Item = Result<Tx, Err>>,
        receipt_tx: &crossbeam_channel::Sender<IndexedReceipt<N::Receipt>>,
    ) -> Result<(E, Vec<Address>), BlockExecutionError>
    where
        E: BlockExecutor<Receipt = N::Receipt, Evm: alloy_evm::Evm<DB = &'a mut State<DB>>>,
        DB: revm::Database + 'a,
        Tx: alloy_evm::block::ExecutableTx<E> + alloy_evm::RecoveredTx<InnerTx>,
        InnerTx: TxHashRef,
        Err: core::error::Error + Send + Sync + 'static,
    {
        let mut senders = Vec::with_capacity(transaction_count);

        // Apply pre-execution changes (e.g., beacon root update)
        let pre_exec_start = Instant::now();
        debug_span!(target: "engine::tree", "pre execution")
            .in_scope(|| executor.apply_pre_execution_changes())?;
        self.metrics.record_pre_execution(pre_exec_start.elapsed());

        // Execute transactions
        let exec_span = debug_span!(target: "engine::tree", "execution").entered();
        let mut transactions = transactions.into_iter();
        // Some executors may execute transactions that do not append receipts during the
        // main loop (e.g., system transactions whose receipts are added during finalization).
        // In that case, invoking the callback on every transaction would resend the previous
        // receipt with the same index and can panic the ordered root builder.
        let mut last_sent_len = 0usize;
        loop {
            // Measure time spent waiting for next transaction from iterator
            // (e.g., parallel signature recovery)
            let wait_start = Instant::now();
            let Some(tx_result) = transactions.next() else { break };
            self.metrics.record_transaction_wait(wait_start.elapsed());

            let tx = tx_result.map_err(BlockExecutionError::other)?;
            let tx_signer = *<Tx as alloy_evm::RecoveredTx<InnerTx>>::signer(&tx);

            senders.push(tx_signer);

            let _enter = debug_span!(
                target: "engine::tree",
                "execute tx",
            )
            .entered();
            trace!(target: "engine::tree", "Executing transaction");

            let tx_start = Instant::now();
            executor.execute_transaction(tx)?;
            self.metrics.record_transaction_execution(tx_start.elapsed());

            let current_len = executor.receipts().len();
            if current_len > last_sent_len {
                last_sent_len = current_len;
                // Send the latest receipt to the background task for incremental root computation.
                if let Some(receipt) = executor.receipts().last() {
                    let tx_index = current_len - 1;
                    let _ = receipt_tx.send(IndexedReceipt::new(tx_index, receipt.clone()));
                }
            }
        }
        drop(exec_span);

        Ok((executor, senders))
    }

    /// Compute state root for the given hashed post state in parallel.
    ///
    /// Uses an overlay factory which provides the state of the parent block, along with the
    /// [`HashedPostState`] containing the changes of this block, to compute the state root and
    /// trie updates for this block.
    ///
    /// # Returns
    ///
    /// Returns `Ok(_)` if computed successfully.
    /// Returns `Err(_)` if error was encountered during computation.
    #[instrument(level = "debug", target = "engine::tree::payload_validator", skip_all)]
    fn compute_state_root_parallel(
        &self,
        overlay_factory: OverlayStateProviderFactory<P>,
        hashed_state: &HashedPostState,
    ) -> Result<(B256, TrieUpdates), ParallelStateRootError> {
        // The `hashed_state` argument will be taken into account as part of the overlay, but we
        // need to use the prefix sets which were generated from it to indicate to the
        // ParallelStateRoot which parts of the trie need to be recomputed.
        let prefix_sets = hashed_state.construct_prefix_sets().freeze();
        let overlay_factory =
            overlay_factory.with_extended_hashed_state_overlay(hashed_state.clone_into_sorted());
        ParallelStateRoot::new(overlay_factory, prefix_sets, self.runtime.clone())
            .incremental_root_with_updates()
    }

    /// Compute state root for the given hashed post state in serial.
    ///
    /// Uses an overlay factory which provides the state of the parent block, along with the
    /// [`HashedPostState`] containing the changes of this block, to compute the state root and
    /// trie updates for this block.
    fn compute_state_root_serial(
        overlay_factory: OverlayStateProviderFactory<P>,
        hashed_state: &HashedPostState,
    ) -> ProviderResult<(B256, TrieUpdates)> {
        // The `hashed_state` argument will be taken into account as part of the overlay, but we
        // need to use the prefix sets which were generated from it to indicate to the
        // StateRoot which parts of the trie need to be recomputed.
        let prefix_sets = hashed_state.construct_prefix_sets().freeze();
        let overlay_factory =
            overlay_factory.with_extended_hashed_state_overlay(hashed_state.clone_into_sorted());

        let provider = overlay_factory.database_provider_ro()?;

        Ok(StateRoot::new(&provider, &provider)
            .with_prefix_sets(prefix_sets)
            .root_with_updates()?)
    }

    /// Awaits the state root from the background task, with an optional timeout fallback.
    ///
    /// If a timeout is configured (`state_root_task_timeout`), this method first waits for the
    /// state root task up to the timeout duration. If the task doesn't complete in time, a
    /// sequential state root computation is spawned via `spawn_blocking`. Both computations
    /// then race: the main thread polls the task receiver and the sequential result channel
    /// in a loop, returning whichever finishes first.
    ///
    /// If no timeout is configured, this simply awaits the state root task without any fallback.
    ///
    /// Returns `ProviderResult<Result<...>>` where the outer `ProviderResult` captures
    /// unrecoverable errors from the sequential fallback (e.g. DB errors), while the inner
    /// `Result` captures parallel state root task errors that can still fall back to serial.
    #[instrument(
        level = "debug",
        target = "engine::tree::payload_validator",
        name = "await_state_root",
        skip_all
    )]
    fn await_state_root_with_timeout<Tx, Err, R: Send + Sync + 'static>(
        &self,
        handle: &mut PayloadHandle<Tx, Err, R>,
        overlay_factory: OverlayStateProviderFactory<P>,
        hashed_state: &HashedPostState,
    ) -> ProviderResult<Result<StateRootComputeOutcome, ParallelStateRootError>> {
        let Some(timeout) = self.config.state_root_task_timeout() else {
            return Ok(handle.state_root());
        };

        let task_rx = handle.take_state_root_rx();

        match task_rx.recv_timeout(timeout) {
            Ok(result) => Ok(result),
            Err(RecvTimeoutError::Disconnected) => {
                Ok(Err(ParallelStateRootError::Other("sparse trie task dropped".to_string())))
            }
            Err(RecvTimeoutError::Timeout) => {
                warn!(
                    target: "engine::tree::payload_validator",
                    ?timeout,
                    "State root task timed out, spawning sequential fallback"
                );
                self.metrics.block_validation.state_root_task_timeout_total.increment(1);

                let (seq_tx, seq_rx) =
                    std::sync::mpsc::channel::<ProviderResult<(B256, TrieUpdates)>>();

                let seq_overlay = overlay_factory;
                let seq_hashed_state = hashed_state.clone();
                self.payload_processor.executor().spawn_blocking(move || {
                    let result = Self::compute_state_root_serial(seq_overlay, &seq_hashed_state);
                    let _ = seq_tx.send(result);
                });

                const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(10);

                loop {
                    match task_rx.recv_timeout(POLL_INTERVAL) {
                        Ok(result) => {
                            debug!(
                                target: "engine::tree::payload_validator",
                                source = "task",
                                "State root timeout race won"
                            );
                            return Ok(result);
                        }
                        Err(RecvTimeoutError::Disconnected) => {
                            debug!(
                                target: "engine::tree::payload_validator",
                                "State root task dropped, waiting for sequential fallback"
                            );
                            let result = seq_rx.recv().map_err(|_| {
                                ProviderError::other(std::io::Error::other(
                                    "both state root computations failed",
                                ))
                            })?;
                            let (state_root, trie_updates) = result?;
                            return Ok(Ok(StateRootComputeOutcome { state_root, trie_updates }));
                        }
                        Err(RecvTimeoutError::Timeout) => {}
                    }

                    if let Ok(result) = seq_rx.try_recv() {
                        debug!(
                            target: "engine::tree::payload_validator",
                            source = "sequential",
                            "State root timeout race won"
                        );
                        let (state_root, trie_updates) = result?;
                        return Ok(Ok(StateRootComputeOutcome { state_root, trie_updates }));
                    }
                }
            }
        }
    }

    /// Compares trie updates from the state root task with serial state root computation.
    ///
    /// This is used for debugging and validating the correctness of the parallel state root
    /// task implementation. When enabled via `--engine.state-root-task-compare-updates`, this
    /// method runs a separate serial state root computation and compares the resulting trie
    /// updates.
    fn compare_trie_updates_with_serial(
        &self,
        overlay_factory: OverlayStateProviderFactory<P>,
        hashed_state: &HashedPostState,
        task_trie_updates: TrieUpdates,
    ) {
        debug!(target: "engine::tree::payload_validator", "Comparing trie updates with serial computation");

        match Self::compute_state_root_serial(overlay_factory.clone(), hashed_state) {
            Ok((serial_root, serial_trie_updates)) => {
                debug!(
                    target: "engine::tree::payload_validator",
                    ?serial_root,
                    "Serial state root computation finished for comparison"
                );

                // Get a database provider to use as trie cursor factory
                match overlay_factory.database_provider_ro() {
                    Ok(provider) => {
                        if let Err(err) = super::trie_updates::compare_trie_updates(
                            &provider,
                            task_trie_updates,
                            serial_trie_updates,
                        ) {
                            warn!(
                                target: "engine::tree::payload_validator",
                                %err,
                                "Error comparing trie updates"
                            );
                        }
                    }
                    Err(err) => {
                        warn!(
                            target: "engine::tree::payload_validator",
                            %err,
                            "Failed to get database provider for trie update comparison"
                        );
                    }
                }
            }
            Err(err) => {
                warn!(
                    target: "engine::tree::payload_validator",
                    %err,
                    "Failed to compute serial state root for comparison"
                );
            }
        }
    }

    /// Validates the block after execution.
    ///
    /// This performs:
    /// - parent header validation
    /// - post-execution consensus validation
    /// - state-root based post-execution validation
    ///
    /// If `receipt_root_bloom` is provided, it will be used instead of computing the receipt root
    /// and logs bloom from the receipts.
    #[instrument(level = "debug", target = "engine::tree::payload_validator", skip_all)]
    fn validate_post_execution<T: PayloadTypes<BuiltPayload: BuiltPayload<Primitives = N>>>(
        &self,
        block: &RecoveredBlock<N::Block>,
        parent_block: &SealedHeader<N::BlockHeader>,
        output: &BlockExecutionOutput<N::Receipt>,
        ctx: &mut TreeCtx<'_, N>,
        receipt_root_bloom: Option<ReceiptRootBloom>,
    ) -> Result<HashedPostState, InsertBlockErrorKind>
    where
        V: PayloadValidator<T, Block = N::Block>,
    {
        let start = Instant::now();

        trace!(target: "engine::tree::payload_validator", block=?block.num_hash(), "Validating block consensus");
        // validate block consensus rules
        if let Err(e) = self.validate_block_inner(block) {
            return Err(e.into())
        }

        // now validate against the parent
        let _enter = debug_span!(target: "engine::tree::payload_validator", "validate_header_against_parent").entered();
        if let Err(e) =
            self.consensus.validate_header_against_parent(block.sealed_header(), parent_block)
        {
            warn!(target: "engine::tree::payload_validator", ?block, "Failed to validate header {} against parent: {e}", block.hash());
            return Err(e.into())
        }
        drop(_enter);

        // Validate block post-execution rules
        let _enter =
            debug_span!(target: "engine::tree::payload_validator", "validate_block_post_execution")
                .entered();
        if let Err(err) =
            self.consensus.validate_block_post_execution(block, output, receipt_root_bloom)
        {
            // call post-block hook
            self.on_invalid_block(parent_block, block, output, None, ctx.state_mut());
            return Err(err.into())
        }
        drop(_enter);

        let _enter =
            debug_span!(target: "engine::tree::payload_validator", "hashed_post_state").entered();
        let hashed_state = self.provider.hashed_post_state(&output.state);
        drop(_enter);

        let _enter = debug_span!(target: "engine::tree::payload_validator", "validate_block_post_execution_with_hashed_state").entered();
        if let Err(err) =
            self.validator.validate_block_post_execution_with_hashed_state(&hashed_state, block)
        {
            // call post-block hook
            self.on_invalid_block(parent_block, block, output, None, ctx.state_mut());
            return Err(err.into())
        }

        // record post-execution validation duration
        self.metrics
            .block_validation
            .post_execution_validation_duration
            .record(start.elapsed().as_secs_f64());

        Ok(hashed_state)
    }

    /// Spawns a payload processor task based on the state root strategy.
    ///
    /// This method determines how to execute the block and compute its state root based on
    /// the selected strategy:
    /// - `StateRootTask`: Uses a dedicated task for state root computation with proof generation
    /// - `Parallel`: Computes state root in parallel with block execution
    /// - `Synchronous`: Falls back to sequential execution and state root computation
    ///
    /// The method handles strategy fallbacks if the preferred approach fails, ensuring
    /// block execution always completes with a valid state root.
    ///
    /// # Arguments
    ///
    /// * `overlay_factory` - Pre-computed overlay factory for multiproof generation
    ///   (`StateRootTask`)
    #[allow(clippy::too_many_arguments)]
    #[instrument(
        level = "debug",
        target = "engine::tree::payload_validator",
        skip_all,
        fields(?strategy)
    )]
    fn spawn_payload_processor<T: ExecutableTxIterator<Evm>>(
        &mut self,
        env: ExecutionEnv<Evm>,
        txs: T,
        provider_builder: StateProviderBuilder<N, P>,
        overlay_factory: OverlayStateProviderFactory<P>,
        strategy: StateRootStrategy,
        block_access_list: Option<Arc<BlockAccessList>>,
    ) -> Result<
        PayloadHandle<
            impl ExecutableTxFor<Evm> + use<N, P, Evm, V, T>,
            impl core::error::Error + Send + Sync + 'static + use<N, P, Evm, V, T>,
            N::Receipt,
        >,
        InsertBlockErrorKind,
    > {
        match strategy {
            StateRootStrategy::StateRootTask => {
                let spawn_start = Instant::now();

                // Use the pre-computed overlay factory for multiproofs
                let handle = self.payload_processor.spawn(
                    env,
                    txs,
                    provider_builder,
                    overlay_factory,
                    &self.config,
                    block_access_list,
                );

                // record prewarming initialization duration
                self.metrics
                    .block_validation
                    .spawn_payload_processor
                    .record(spawn_start.elapsed().as_secs_f64());

                Ok(handle)
            }
            StateRootStrategy::Parallel | StateRootStrategy::Synchronous => {
                let start = Instant::now();
                let handle = self.payload_processor.spawn_cache_exclusive(
                    env,
                    txs,
                    provider_builder,
                    block_access_list,
                );

                // Record prewarming initialization duration
                self.metrics
                    .block_validation
                    .spawn_payload_processor
                    .record(start.elapsed().as_secs_f64());

                Ok(handle)
            }
        }
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
            debug!(target: "engine::tree::payload_validator", %hash, %historical, "found canonical state for block in memory, creating provider builder");
            // the block leads back to the canonical chain
            return Ok(Some(StateProviderBuilder::new(
                self.provider.clone(),
                historical,
                Some(blocks),
            )))
        }

        // Check if the block is persisted
        if let Some(header) = self.provider.header(hash)? {
            debug!(target: "engine::tree::payload_validator", %hash, number = %header.number(), "found canonical state for block in database, creating provider builder");
            // For persisted blocks, we create a builder that will fetch state directly from the
            // database
            return Ok(Some(StateProviderBuilder::new(self.provider.clone(), hash, None)))
        }

        debug!(target: "engine::tree::payload_validator", %hash, "no canonical state found for block");
        Ok(None)
    }

    /// Determines the state root computation strategy based on configuration.
    ///
    /// Note: Use state root task only if prefix sets are empty, otherwise proof generation is
    /// too expensive because it requires walking all paths in every proof.
    const fn plan_state_root_computation(&self) -> StateRootStrategy {
        if self.config.state_root_fallback() {
            StateRootStrategy::Synchronous
        } else if self.config.use_state_root_task() {
            StateRootStrategy::StateRootTask
        } else {
            StateRootStrategy::Parallel
        }
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
            return
        }
        self.invalid_block_hook.on_invalid_block(parent_header, block, output, trie_updates);
    }

    /// Creates a [`LazyOverlay`] for the parent block without blocking.
    ///
    /// Returns a lazy overlay that will compute the trie input on first access, and the anchor
    /// block hash (the highest persisted ancestor). This allows execution to start immediately
    /// while the trie input computation is deferred until the overlay is actually needed.
    ///
    /// If parent is on disk (no in-memory blocks), returns `None` for the lazy overlay.
    ///
    /// Uses a cached overlay if available for the canonical head (the common case).
    fn get_parent_lazy_overlay(
        parent_hash: B256,
        state: &EngineApiTreeState<N>,
    ) -> (Option<LazyOverlay>, B256) {
        // Get blocks leading to the parent to determine the anchor
        let (anchor_hash, blocks) =
            state.tree_state.blocks_by_hash(parent_hash).unwrap_or_else(|| (parent_hash, vec![]));

        if blocks.is_empty() {
            debug!(target: "engine::tree::payload_validator", "Parent found on disk, no lazy overlay needed");
            return (None, anchor_hash);
        }

        // Try to use the cached overlay if it matches both parent hash and anchor
        if let Some(cached) = state.tree_state.get_cached_overlay(parent_hash, anchor_hash) {
            debug!(
                target: "engine::tree::payload_validator",
                %parent_hash,
                %anchor_hash,
                "Using cached canonical overlay"
            );
            return (Some(cached.overlay.clone()), cached.anchor_hash);
        }

        debug!(
            target: "engine::tree::payload_validator",
            %anchor_hash,
            num_blocks = blocks.len(),
            "Creating lazy overlay for in-memory blocks"
        );

        // Extract deferred trie data handles (non-blocking)
        let handles: Vec<DeferredTrieData> = blocks.iter().map(|b| b.trie_data_handle()).collect();

        (Some(LazyOverlay::new(anchor_hash, handles)), anchor_hash)
    }

    /// Spawns a background task to compute and sort trie data for the executed block.
    ///
    /// This function creates a [`DeferredTrieData`] handle with fallback inputs and spawns a
    /// blocking task that calls `wait_cloned()` to:
    /// 1. Sort the block's hashed state and trie updates
    /// 2. Merge ancestor overlays and extend with the sorted data
    /// 3. Create an [`AnchoredTrieInput`](reth_chain_state::AnchoredTrieInput) for efficient future
    ///    trie computations
    /// 4. Cache the result so subsequent calls return immediately
    ///
    /// If the background task hasn't completed when `trie_data()` is called, `wait_cloned()`
    /// computes from the stored inputs, eliminating deadlock risk and duplicate computation.
    ///
    /// The validation hot path can return immediately after state root verification,
    /// while consumers (DB writes, overlay providers, proofs) get trie data either
    /// from the completed task or via fallback computation.
    fn spawn_deferred_trie_task(
        &self,
        block: RecoveredBlock<N::Block>,
        execution_outcome: Arc<BlockExecutionOutput<N::Receipt>>,
        ctx: &TreeCtx<'_, N>,
        hashed_state: HashedPostState,
        trie_output: TrieUpdates,
        overlay_factory: OverlayStateProviderFactory<P>,
    ) -> ExecutedBlock<N> {
        // Capture parent hash and ancestor overlays for deferred trie input construction.
        let (anchor_hash, overlay_blocks) = ctx
            .state()
            .tree_state
            .blocks_by_hash(block.parent_hash())
            .unwrap_or_else(|| (block.parent_hash(), Vec::new()));

        // Collect lightweight ancestor trie data handles. We don't call trie_data() here;
        // the merge and any fallback sorting happens in the compute_trie_input_task.
        let ancestors: Vec<DeferredTrieData> =
            overlay_blocks.iter().rev().map(|b| b.trie_data_handle()).collect();

        // Create deferred handle with fallback inputs in case the background task hasn't completed.
        let deferred_trie_data = DeferredTrieData::pending(
            Arc::new(hashed_state),
            Arc::new(trie_output),
            anchor_hash,
            ancestors,
        );
        let deferred_handle_task = deferred_trie_data.clone();
        let block_validation_metrics = self.metrics.block_validation.clone();

        // Capture block info and cache handle for changeset computation
        let block_hash = block.hash();
        let block_number = block.number();
        let changeset_cache = self.changeset_cache.clone();

        // Spawn background task to compute trie data. Calling `wait_cloned` will compute from
        // the stored inputs and cache the result, so subsequent calls return immediately.
        let compute_trie_input_task = move || {
            let _span = debug_span!(
                target: "engine::tree::payload_validator",
                "compute_trie_input_task",
                block_number
            )
            .entered();

            let result = panic::catch_unwind(AssertUnwindSafe(|| {
                let compute_start = Instant::now();
                let computed = deferred_handle_task.wait_cloned();
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
                if let Some(anchored) = &computed.anchored_trie_input {
                    block_validation_metrics
                        .anchored_overlay_trie_updates_size
                        .record(anchored.trie_input.nodes.total_len() as f64);
                    block_validation_metrics
                        .anchored_overlay_hashed_state_size
                        .record(anchored.trie_input.state.total_len() as f64);
                }

                // Compute and cache changesets using the computed trie_updates
                let changeset_start = Instant::now();

                // Get a provider from the overlay factory for trie cursor access
                let changeset_result =
                    overlay_factory.database_provider_ro().and_then(|provider| {
                        reth_trie::changesets::compute_trie_changesets(
                            &provider,
                            &computed.trie_updates,
                        )
                        .map_err(ProviderError::Database)
                    });

                match changeset_result {
                    Ok(changesets) => {
                        debug!(
                            target: "engine::tree::changeset",
                            ?block_number,
                            elapsed = ?changeset_start.elapsed(),
                            "Computed and caching changesets"
                        );

                        changeset_cache.insert(block_hash, block_number, Arc::new(changesets));
                    }
                    Err(e) => {
                        warn!(
                            target: "engine::tree::changeset",
                            ?block_number,
                            ?e,
                            "Failed to compute changesets in deferred trie task"
                        );
                    }
                }
            }));

            if result.is_err() {
                error!(
                    target: "engine::tree::payload_validator",
                    "Deferred trie task panicked; fallback computation will be used when trie data is accessed"
                );
            }
        };

        // Spawn task that computes trie data asynchronously.
        self.payload_processor.executor().spawn_blocking(compute_trie_input_task);

        ExecutedBlock::with_deferred_trie_data(
            Arc::new(block),
            execution_outcome,
            deferred_trie_data,
        )
    }
}

/// Output of block or payload validation.
pub type ValidationOutcome<N, E = InsertPayloadError<BlockTy<N>>> = Result<ExecutedBlock<N>, E>;

/// Strategy describing how to compute the state root.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StateRootStrategy {
    /// Use the state root task (background sparse trie computation).
    StateRootTask,
    /// Run the parallel state root computation on the calling thread.
    Parallel,
    /// Fall back to synchronous computation via the state provider.
    Synchronous,
}

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
    /// See also: <https://github.com/ethereum/execution-apis/blob/main/src/engine/common.md#specification-1>
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
    fn convert_payload_to_block(
        &self,
        payload: Types::ExecutionData,
    ) -> Result<SealedBlock<N::Block>, NewPayloadError>;

    /// Validates a payload received from engine API.
    fn validate_payload(
        &mut self,
        payload: Types::ExecutionData,
        ctx: TreeCtx<'_, N>,
    ) -> ValidationOutcome<N>;

    /// Validates a block downloaded from the network.
    fn validate_block(
        &mut self,
        block: SealedBlock<N::Block>,
        ctx: TreeCtx<'_, N>,
    ) -> ValidationOutcome<N>;

    /// Hook called after an executed block is inserted directly into the tree.
    ///
    /// This is invoked when blocks are inserted via `InsertExecutedBlock` (e.g., locally built
    /// blocks by sequencers) to allow implementations to update internal state such as caches.
    fn on_inserted_executed_block(&self, block: ExecutedBlock<N>);
}

impl<N, Types, P, Evm, V> EngineValidator<Types> for BasicEngineValidator<P, Evm, V>
where
    P: DatabaseProviderFactory<
            Provider: BlockReader
                          + StageCheckpointReader
                          + PruneCheckpointReader
                          + ChangeSetReader
                          + StorageChangeSetReader
                          + BlockNumReader
                          + StorageSettingsCache,
        > + BlockReader<Header = N::BlockHeader>
        + StateProviderFactory
        + StateReader
        + ChangeSetReader
        + BlockNumReader
        + HashedPostStateProvider
        + Clone
        + 'static,
    N: NodePrimitives,
    V: PayloadValidator<Types, Block = N::Block> + Clone,
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

    fn convert_payload_to_block(
        &self,
        payload: Types::ExecutionData,
    ) -> Result<SealedBlock<N::Block>, NewPayloadError> {
        let block = self.validator.convert_payload_to_block(payload)?;
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
        block: SealedBlock<N::Block>,
        ctx: TreeCtx<'_, N>,
    ) -> ValidationOutcome<N> {
        self.validate_block_with_state(BlockOrPayload::Block(block), ctx)
    }

    fn on_inserted_executed_block(&self, block: ExecutedBlock<N>) {
        self.payload_processor.on_inserted_executed_block(
            block.recovered_block.block_with_parent(),
            &block.execution_output.state,
        );
    }
}

impl<P, Evm, V> WaitForCaches for BasicEngineValidator<P, Evm, V>
where
    Evm: ConfigureEvm,
{
    fn wait_for_caches(&self) -> CacheWaitDurations {
        self.payload_processor.wait_for_caches()
    }
}

/// Enum representing either block or payload being validated.
#[derive(Debug, Clone)]
pub enum BlockOrPayload<T: PayloadTypes> {
    /// Payload.
    Payload(T::ExecutionData),
    /// Block.
    Block(SealedBlock<BlockTy<<T::BuiltPayload as BuiltPayload>::Primitives>>),
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

    /// Returns a string showing whether or not this is a block or payload.
    pub const fn type_name(&self) -> &'static str {
        match self {
            Self::Payload(_) => "payload",
            Self::Block(_) => "block",
        }
    }

    /// Returns the block access list if available.
    pub fn block_access_list(&self) -> Option<Result<BlockAccessList, alloy_rlp::Error>>
    where
        T::ExecutionData: ExecutionPayload,
    {
        match self {
            Self::Payload(payload) => payload
                .block_access_list()
                .map(|bytes| BlockAccessList::decode(&mut bytes.as_ref())),
            Self::Block(_) => None,
        }
    }

    /// Returns the number of transactions in the payload or block.
    pub fn transaction_count(&self) -> usize
    where
        T::ExecutionData: ExecutionPayload,
    {
        match self {
            Self::Payload(payload) => payload.transaction_count(),
            Self::Block(block) => block.transaction_count(),
        }
    }

    /// Returns the withdrawals from the payload or block.
    pub fn withdrawals(&self) -> Option<&[Withdrawal]>
    where
        T::ExecutionData: ExecutionPayload,
    {
        match self {
            Self::Payload(payload) => payload.withdrawals().map(|w| w.as_slice()),
            Self::Block(block) => block.body().withdrawals().map(|w| w.as_slice()),
        }
    }

    /// Returns the total gas used by the block.
    pub fn gas_used(&self) -> u64
    where
        T::ExecutionData: ExecutionPayload,
    {
        match self {
            Self::Payload(payload) => payload.gas_used(),
            Self::Block(block) => block.gas_used(),
        }
    }
}
