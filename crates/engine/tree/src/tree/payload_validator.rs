//! Types and traits for validating blocks and payloads.
//!
//! # Payload validation flow
//!
//! [`BasicEngineValidator::validate_block_with_state`] is the engine-side entry point for an
//! inserted block or an `engine_newPayload` payload. It overlaps payload conversion, transaction
//! preparation, cache prewarming, receipt-root computation, state-root computation, and deferred
//! trie input construction wherever those tasks do not depend on each other.
//!
//! ## Validation phases
//!
//! 1. Fetch the parent header and spawn `payload-convert`, which converts payloads into sealed
//!    blocks and runs header, parent-header, and pre-execution consensus validation.
//! 2. Build the parent state provider, EVM environment, transaction iterator, lazy ancestor
//!    overlay, and optional decoded EIP-7928 block access list (BAL).
//! 3. Prepare the per-block state-root job. The default strategy picks a skipped, synchronous, or
//!    sparse-trie job from [`TreeConfig`].
//! 4. Spawn the payload processor. This always prepares transaction conversion and prewarming; a
//!    streaming state-root job can provide a sink for prewarm and execution updates.
//! 5. Execute the block. BAL payloads use the parallel BAL execute path only when state caching and
//!    BAL parallel execution are enabled. Otherwise the regular executor still builds and validates
//!    the BAL before post-execution consensus uses the decoded BAL hash.
//! 6. Stop prewarming, terminate execution caching, spawn `hash-post-state`, await
//!    `payload-convert` and `receipt-root`, then run post-execution consensus validation.
//! 7. Resolve the state root by finishing the prepared job. The sparse-trie job falls back to
//!    serial computation when the state-root task fails to produce a usable root.
//! 8. Verify the header state root, spawn deferred trie input computation, and return the executed
//!    block without waiting for that deferred trie task on the hot path.
//!
//! ## Spawned background work
//!
//! | Work | Spawned when | Role | Completion point |
//! | --- | --- | --- | --- |
//! | `payload-convert` | parent is known | convert payloads, validate header and body roots | after execution, unless the gas sanity check awaits it earlier |
//! | `tx-iterator` | payload processor setup | convert transactions, using rayon for larger blocks | consumed by regular and BAL execution |
//! | `prewarm` | payload processor setup | warm execution caches; in BAL mode, stream BAL-derived trie targets | stopped after execution, then caching is terminated |
//! | proof workers | sparse-trie task setup | fetch trie proofs for sparse trie updates | consumed by the sparse trie task |
//! | `sparse-trie` | sparse-trie task setup | apply execution or BAL updates and compute the state root | awaited by the state-root job |
//! | `receipt-root` | execution start | compute receipt root and logs bloom incrementally | awaited before post-execution consensus |
//! | `hash-post-state` | after execution | hash changed accounts and storage from `BundleState` | awaited by post-execution validation and root computation |
//! | `serial-root` | sparse trie timeout fallback | race serial state-root computation against the sparse trie task | polled by the sparse-trie job |
//! | deferred trie task | after root verification | sort trie data | not awaited by the validation hot path |
//!
//! ```mermaid
//! sequenceDiagram
//!     autonumber
//!     participant Main as validate_block_with_state
//!     participant Convert as payload-convert
//!     participant Tx as tx-iterator
//!     participant Prewarm as prewarm
//!     participant Exec as EVM execution
//!     participant Receipt as receipt-root
//!     participant Trie as sparse trie and proofs
//!     participant Hash as hash-post-state
//!     participant Deferred as deferred trie task
//!
//!     Main->>Convert: spawn convert and pre-execution validation
//!     Main->>Main: parent provider, EVM env, optional BAL decode
//!     Main->>Tx: spawn transaction conversion
//!     alt sparse-trie job
//!         Main->>Trie: spawn proof workers and sparse trie
//!     end
//!     Main->>Prewarm: spawn transaction, BAL, or skipped prewarm
//!     Main->>Receipt: spawn receipt root task
//!     alt BAL path eligible
//!         Main->>Exec: execute_block_bal
//!         Prewarm->>Trie: BAL-derived sparse trie updates
//!     else regular execution
//!         Tx-->>Exec: recovered transactions in block order
//!         Main->>Exec: execute_block
//!         Exec->>Receipt: stream receipts
//!         Exec->>Trie: stream state hook updates
//!         Exec->>Exec: rebuild and validate BAL when present
//!     end
//!     Main->>Prewarm: stop prewarming and terminate cache
//!     Main->>Hash: spawn changed-state hashing
//!     Convert-->>Main: sealed block
//!     Receipt-->>Main: receipt root and logs bloom
//!     Main->>Main: post-execution consensus and BAL hash check
//!     Hash-->>Main: hashed post state
//!     alt sparse-trie job
//!         Trie-->>Main: state root and trie updates
//!     else synchronous or fallback
//!         Main->>Main: compute serial StateRoot
//!     end
//!     Main->>Main: verify header state root
//!     Main->>Deferred: spawn trie input sorting
//!     Main-->>Main: return ValidationOutput
//! ```
//!
//! ## Payload attributes validation
//!
//! During `engine_forkchoiceUpdated`,
//! [`PayloadValidator::validate_payload_attributes_against_header`] checks payload attributes
//! before a payload build job starts. On failure, the engine returns
//! `INVALID_PAYLOAD_ATTRIBUTES` without rolling back the forkchoice update.

use crate::tree::{
    error::{InsertBlockError, InsertBlockErrorKind, InsertPayloadError},
    instrumented_state::{InstrumentedStateProvider, StateProviderMetrics, StateProviderStats},
    multiproof::{PayloadStateRootHandle, StateRootStreams},
    payload_processor::PayloadProcessor,
    precompile_cache::{CachedPrecompile, CachedPrecompileMetrics, PrecompileCacheMap},
    types::{InsertPayloadResult, ValidationOutput},
    CacheWaitDurations, CachedStateProvider, EngineApiMetrics, EngineApiTreeState, ExecutionEnv,
    PayloadHandle, StateProviderBuilder, StateProviderDatabase, TreeConfig, WaitForCaches,
};
use alloy_consensus::transaction::{Either, TxHashRef};
use alloy_eip7928::{bal::DecodedBal, compute_block_access_list_hash, BlockAccessList};
use alloy_eips::{eip1898::BlockWithParent, eip4895::Withdrawal, NumHash};
use alloy_evm::Evm;
use alloy_primitives::{
    map::{AddressMap, B256Set},
    B256,
};
use reth_tasks::LazyHandle;

use crate::tree::{
    payload_processor::receipt_root_task::{IndexedReceipt, ReceiptRootTaskHandle},
    state_root_strategy::{
        DefaultStateRootStrategy, PayloadStateRootJobContext, StateRootJobContext,
        StateRootStrategy,
    },
};
use alloy_consensus::constants::KECCAK_EMPTY;
use alloy_primitives::Address;
use reth_chain_state::{
    CanonicalInMemoryState, DeferredTrieData, ExecutedBlock, ExecutionTimingStats,
    StateTrieOverlayManager,
};
use reth_consensus::{ConsensusError, FullConsensus, ReceiptRootBloom};
use reth_engine_primitives::{
    ConfigureEngineEvm, ExecutableTxIterator, ExecutionPayload, InvalidBlockHook, PayloadValidator,
};
use reth_errors::{BlockExecutionError, ProviderResult};
use reth_evm::{
    block::BlockExecutor, execute::ExecutableTxFor, ConfigureEvm, EvmEnvFor, ExecutionCtxFor,
    OnStateHook, SpecFor,
};
use reth_execution_cache::{CacheFillMode, CacheStats, SavedCache};
use reth_payload_primitives::{
    BuiltPayload, BuiltPayloadExecutedBlock, InvalidPayloadAttributesError, NewPayloadError,
    PayloadTypes,
};
use reth_primitives_traits::{
    AlloyBlockHeader, BlockBody, BlockTy, FastInstant as Instant, GotExpected, NodePrimitives,
    RecoveredBlock, SealedBlock, SealedHeader, SignerRecoverable,
};
use reth_provider::{
    providers::{OverlayBuilder, OverlayStateProviderFactory},
    BlockExecutionOutput, BlockNumReader, BlockReader, ChangeSetReader, DatabaseProviderFactory,
    DatabaseProviderROFactory, HashedPostStateProvider, ProviderError, PruneCheckpointReader,
    StageCheckpointReader, StateProvider, StateProviderBox, StateProviderFactory, StateReader,
    StorageChangeSetReader, StorageSettingsCache,
};
use reth_revm::db::{states::bundle_state::BundleRetention, BundleAccount, State};
use reth_trie::{
    hashed_cursor::HashedCursorFactory, prefix_set::TriePrefixSetsMut,
    trie_cursor::TrieCursorFactory, updates::TrieUpdates, HashedPostState,
};
use reth_trie_db::ChangesetCache;
use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};
use tracing::{debug, debug_span, error, info, instrument, trace, warn, Level, Span};

pub use crate::tree::types::ValidationOutcome;

/// Handle to a [`HashedPostState`] computed on a background thread.
pub type LazyHashedPostState = reth_tasks::LazyHandle<Arc<HashedPostState>>;

/// Multiplier over the parent's gas limit beyond which a block's claimed gas usage cannot be
/// legitimate. Gas limit can change by at most 1/1024 per block, so anything over this is rejected
/// without entering execution.
const MAX_EXPECTED_GAS_USAGE_MULTIPLIER: u64 = 2;

/// Worker name for deferred trie data preparation.
const DEFERRED_TRIE_WORKER_NAME: &str = "deferred-trie";

type ReceiptRootSender<N> =
    crossbeam_channel::Sender<IndexedReceipt<<N as NodePrimitives>::Receipt>>;
type ReceiptRootReceiver = tokio::sync::oneshot::Receiver<(B256, alloy_primitives::Bloom)>;

/// Context providing access to tree state during validation.
///
/// This context is provided to the [`EngineValidator`] and includes the state of the tree's
/// internals
pub struct TreeCtx<'a, N: NodePrimitives> {
    /// The engine API tree state
    state: &'a mut EngineApiTreeState<N>,
    /// Reference to the canonical in-memory state
    canonical_in_memory_state: &'a CanonicalInMemoryState<N>,
    /// Pending sparse trie prune request to consume when spawning a sparse trie task.
    pending_sparse_trie_prune: &'a mut Option<TriePrefixSetsMut>,
}

impl<'a, N: NodePrimitives> std::fmt::Debug for TreeCtx<'a, N> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TreeCtx")
            .field("state", &"EngineApiTreeState")
            .field("canonical_in_memory_state", &self.canonical_in_memory_state)
            .field("pending_sparse_trie_prune", &self.pending_sparse_trie_prune.is_some())
            .finish()
    }
}

impl<'a, N: NodePrimitives> TreeCtx<'a, N> {
    /// Creates a new tree context
    pub const fn new(
        state: &'a mut EngineApiTreeState<N>,
        canonical_in_memory_state: &'a CanonicalInMemoryState<N>,
        pending_sparse_trie_prune: &'a mut Option<TriePrefixSetsMut>,
    ) -> Self {
        Self { state, canonical_in_memory_state, pending_sparse_trie_prune }
    }
}

impl<'a, N: NodePrimitives> TreeCtx<'a, N> {
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

    /// Takes the pending sparse trie prune request, if any.
    pub const fn take_sparse_trie_prune(&mut self) -> Option<TriePrefixSetsMut> {
        self.pending_sparse_trie_prune.take()
    }
}

/// Pauses JIT helper execution while validating imported payloads.
///
/// Validation still queues JIT work and can use resident compiled code, but helper execution is
/// paused during validation to minimize latency. Queued work resumes when validation exits, so JIT
/// compilation is biased toward idle periods instead of competing with payload validation.
struct JitPauseGuard<Evm: ConfigureEvm>(Evm);

impl<Evm: ConfigureEvm> JitPauseGuard<Evm> {
    fn new(evm_config: &Evm) -> Self {
        if let Some(jit_backend) = evm_config.jit_backend() {
            jit_backend.pause();
        }
        Self(evm_config.clone())
    }
}

impl<Evm: ConfigureEvm> Drop for JitPauseGuard<Evm> {
    fn drop(&mut self) {
        if let Some(jit_backend) = self.0.jit_backend() {
            jit_backend.resume();
        }
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
    precompile_cache_metrics: AddressMap<CachedPrecompileMetrics>,
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
    /// State-root strategy used to prepare per-block commitment tasks.
    #[debug(skip)]
    state_root_strategy: Arc<dyn StateRootStrategy<Evm::Primitives, P, Evm>>,
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
    OverlayStateProviderFactory<P, N>: DatabaseProviderROFactory<Provider: TrieCursorFactory + HashedCursorFactory>
        + Clone
        + Send
        + Sync
        + 'static,
    Evm: ConfigureEvm<Primitives = N> + 'static,
{
    /// Creates a new `TreePayloadValidator`.
    #[expect(clippy::too_many_arguments)]
    pub fn new(
        provider: P,
        consensus: Arc<dyn FullConsensus<N>>,
        evm_config: Evm,
        validator: V,
        config: TreeConfig,
        invalid_block_hook: Box<dyn InvalidBlockHook<N>>,
        changeset_cache: ChangesetCache,
        state_trie_overlays: StateTrieOverlayManager<N>,
        runtime: reth_tasks::Runtime,
    ) -> Self {
        let precompile_cache_map = PrecompileCacheMap::default();
        let payload_processor = PayloadProcessor::new(
            runtime.clone(),
            evm_config.clone(),
            &config,
            precompile_cache_map.clone(),
            state_trie_overlays,
        );
        Self {
            provider,
            consensus,
            evm_config,
            payload_processor,
            precompile_cache_map,
            precompile_cache_metrics: AddressMap::default(),
            config,
            invalid_block_hook,
            metrics: EngineApiMetrics::default(),
            validator,
            changeset_cache,
            runtime,
            state_root_strategy: Arc::new(DefaultStateRootStrategy),
        }
    }

    /// Sets the state-root strategy used by payload validation.
    pub fn with_state_root_strategy(
        mut self,
        state_root_strategy: Arc<dyn StateRootStrategy<N, P, Evm>>,
    ) -> Self {
        self.state_root_strategy = state_root_strategy;
        self
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
    ) -> InsertPayloadResult<N>
    where
        V: PayloadValidator<T, Block = N::Block> + Clone,
        Evm: ConfigureEngineEvm<T::ExecutionData, Primitives = N>,
    {
        let parent_hash = input.parent_hash();
        let _jit_pause = JitPauseGuard::new(&self.evm_config);

        // Fetch parent block. This goes to memory most of the time unless the parent block is
        // beyond the in-memory buffer.
        let parent_block = match self.sealed_header_by_hash(parent_hash, ctx.state()) {
            Ok(Some(parent_block)) => parent_block,
            Ok(None) => {
                return Err(InsertBlockError::new(
                    self.convert_to_block(input)?,
                    ProviderError::HeaderNotFound(parent_hash.into()).into(),
                )
                .into())
            }
            Err(e) => {
                return Err(InsertBlockError::new(self.convert_to_block(input)?, e.into()).into())
            }
        };

        // Spawn payload conversion and basic validation on a background thread so it runs
        // concurrently with the rest of the function (setup + execution). For payloads this
        // overlaps the cost of RLP decoding + header hashing.
        let validated_block = self.spawn_convert_and_validate(&input, parent_block.clone());

        /// A helper macro that returns the block in case there was an error
        /// This macro is used for early returns before block conversion
        macro_rules! ensure_ok {
            ($expr:expr) => {
                match $expr {
                    Ok(val) => val,
                    Err(e) => {
                        let block = validated_block.try_into_inner().expect("sole handle")?;
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

        // If the gas usage is suspiciously high (multiple times higher than parent's gas limit), be
        // cautious and block on pre-execution checks of the block.
        if input.gas_used() > parent_block.gas_limit() * MAX_EXPECTED_GAS_USAGE_MULTIPLIER {
            // Call `.get()` to await the pre-execution checks and exit early if they fail.
            if validated_block.get().is_err() {
                return Err(validated_block
                    .try_into_inner()
                    .expect("sole handle")
                    .expect_err("Err result checked"))
            }
        }

        trace!(target: "engine::tree::payload_validator", "Fetching block state provider");
        let _enter =
            debug_span!(target: "engine::tree::payload_validator", "state_provider").entered();
        let Some(provider_builder) =
            ensure_ok!(self.state_provider_builder(parent_hash, ctx.state()))
        else {
            // this is pre-validated in the tree
            return Err(InsertBlockError::new(
                validated_block.try_into_inner().expect("sole handle")?,
                ProviderError::HeaderNotFound(parent_hash.into()).into(),
            )
            .into())
        };
        drop(_enter);

        let evm_env = debug_span!(target: "engine::tree::payload_validator", "evm_env")
            .in_scope(|| self.evm_env_for(&input))
            .map_err(NewPayloadError::other)?;

        // Extract the decoded BAL, if valid and available.
        let decoded_bal = ensure_ok!(input
            .try_decoded_access_list()
            .map_err(|err| ConsensusError::BlockAccessListInvalid(err.to_string())))
        .map(Arc::new);

        if let Some(decoded_bal) = decoded_bal.as_deref() {
            // Reject oversized BAL sidecars before executing the block.
            ensure_ok!(decoded_bal
                .as_bal()
                .validate_gas_limit(input.gas_limit())
                .map_err(ConsensusError::from));
        }

        let env = ExecutionEnv {
            evm_env,
            hash: input.hash(),
            parent_hash: input.parent_hash(),
            parent_state_root: parent_block.state_root(),
            transaction_count: input.transaction_count(),
            gas_used: input.gas_used(),
            withdrawals: input.withdrawals().map(|w| w.to_vec()),
            decoded_bal: decoded_bal.as_ref().map(Arc::clone),
        };

        // Get an iterator over the transactions in the payload
        let txs = self.tx_iterator_for(&input)?;

        // Create overlay factory for state-root tasks that need multiproofs.
        let provider_factory = self.provider.clone();
        let overlay_builder = Self::overlay_builder_for_parent(
            parent_hash,
            ctx.state(),
            self.changeset_cache.clone(),
        );
        let proof_worker_overlay_builder =
            overlay_builder.with_parent_state_root(parent_block.state_root());
        let overlay_factory =
            OverlayStateProviderFactory::new(provider_factory, proof_worker_overlay_builder);

        let parallel_bal_execution = ensure_ok!(self.bal_path_eligible(env.decoded_bal.as_deref()));

        // Prepare the state-root job before execution so it can provide streaming hooks.
        let pending_sparse_trie_prune = (!self.config.skip_state_root() &&
            !self.config.state_root_fallback() &&
            self.config.use_state_root_task())
        .then(|| ctx.take_sparse_trie_prune())
        .flatten();
        let mut state_root_job =
            ensure_ok!(self.state_root_strategy.prepare(StateRootJobContext::new(
                &self.payload_processor,
                &env,
                provider_builder.clone(),
                overlay_factory,
                &self.config,
                parallel_bal_execution,
                pending_sparse_trie_prune,
            )));
        let state_root_job_name = state_root_job.name();

        debug!(
            target: "engine::tree::payload_validator",
            strategy = state_root_job_name,
            "Prepared state root job"
        );

        let state_root_streams = state_root_job.streams();
        // Only take the hook on the serial path: on the parallel BAL path it would be dropped
        // unused, and the drop would fire a spurious end-of-updates signal into the job.
        let execution_state_hook =
            (!parallel_bal_execution).then(|| state_root_job.take_execution_hook()).flatten();

        // Spawn transaction conversion and prewarming.
        let mut handle = ensure_ok!(self.spawn_payload_processor(
            env.clone(),
            txs,
            provider_builder.clone(),
            state_root_streams,
            parallel_bal_execution,
        ));

        // Create optional cache stats for detailed block logging
        let slow_block_enabled = self.config.slow_block_threshold().is_some();
        let cache_stats = slow_block_enabled.then(|| Arc::new(CacheStats::default()));
        let instrument_state_provider = slow_block_enabled || self.config.state_provider_metrics();
        let state_provider_metrics =
            instrument_state_provider.then(|| StateProviderMetrics::with_source("engine"));
        let state_provider_stats =
            instrument_state_provider.then(|| Arc::new(StateProviderStats::default()));
        let execution_cache = handle.caches().map(|caches| (caches, handle.cache_metrics()));

        // This state provider factory is parametrized by:
        //
        // 1. fill_on_miss?
        // 2. instrument_state_provider?
        //
        // `fill_on_miss` controls whether the loaded value after a cache miss will be inserted
        // back into the cache. On a glance it seems to be always useful to do this. However,
        // in practice, for the serial/non-BAL execution, it's not needed and is net negative:
        //
        // - It's not necessary because the revm machinery provides layer of caching itself. That
        //   means a value for a miss will be recorded in revm's cache.
        // - Inserting back into the cache is not free.
        // - After execution, the execution post-state will be dumped into the execution cache as
        //   whole anyway.
        //
        // Therefore, there `fill_on_miss` is going to be false for those paths.
        //
        // The second parameter `instrument_state_provider` controls whether we should
        // instrument the state provider with metrics.
        let make_state_provider = |fill_on_miss: bool| -> ProviderResult<StateProviderBox> {
            let provider = provider_builder.build()?;
            let mut provider = if let Some((caches, cache_metrics)) = &execution_cache {
                let fill_mode = if fill_on_miss {
                    CacheFillMode::FillOnMiss
                } else {
                    CacheFillMode::LookupOnly
                };
                Box::new(CachedStateProvider::new_with_mode(
                    provider,
                    caches.clone(),
                    fill_mode,
                    cache_metrics.clone(),
                    cache_stats.clone(),
                )) as StateProviderBox
            } else {
                provider
            };

            if instrument_state_provider {
                let stats = state_provider_stats
                    .as_ref()
                    .expect("instrumented state provider requires shared stats");
                let metrics = state_provider_metrics
                    .as_ref()
                    .expect("instrumented state provider requires metrics");
                provider = Box::new(InstrumentedStateProvider::with_stats(
                    provider,
                    metrics.clone(),
                    Arc::clone(stats),
                ));
            }

            Ok(provider)
        };

        // Execute the block and handle any execution errors.
        // The receipt root task is spawned before execution and receives receipts incrementally
        // as transactions complete, allowing parallel computation during execution.
        let execute_block_start = Instant::now();
        let execution_result = if parallel_bal_execution {
            self.execute_block_bal(env, &input, &handle, &make_state_provider)
        } else {
            let state_provider = make_state_provider(false);
            match state_provider {
                Ok(state_provider) => self.execute_block(
                    state_provider,
                    env,
                    &input,
                    &mut handle,
                    execution_state_hook,
                ),
                Err(err) => Err(err.into()),
            }
        };
        let execution_duration = execute_block_start.elapsed();
        if let (Some(metrics), Some(stats)) = (&state_provider_metrics, &state_provider_stats) {
            metrics.record_totals(stats);
        }
        let (output, senders, receipt_root_rx, built_bal) = ensure_ok!(execution_result);

        // After executing the block we can stop prewarming transactions
        handle.stop_prewarming_execution();

        // Create ExecutionOutcome early so we can terminate caching before validation and state
        // root computation. Using Arc allows sharing with both the caching task and the deferred
        // trie task without cloning the expensive BundleState.
        let output = Arc::new(output);

        // Terminate caching task early since execution is complete and caching is no longer
        // needed. This frees up resources while state root computation continues.
        let valid_block_tx = handle.terminate_caching(Some(output.clone()));

        // Spawn hashed post state computation in background so it runs concurrently with
        // block conversion and receipt root computation. This is a pure CPU-bound task
        // (keccak256 hashing of all changed addresses and storage slots).
        let hashed_state_output = output.clone();
        let hashed_state_provider = self.provider.clone();
        let mut hashed_state_rx = state_root_job.take_hashed_state_rx();
        let mut hashed_state: LazyHashedPostState =
            self.payload_processor.executor().spawn_blocking_named("hash-post-state", move || {
                let _span = debug_span!(
                    target: "engine::tree::payload_validator",
                    "hashed_post_state",
                )
                .entered();
                let state = if let Some(Ok(state)) = hashed_state_rx.as_mut().map(|rx| rx.recv()) {
                    state
                } else {
                    hashed_state_provider.hashed_post_state(&hashed_state_output.state)
                };
                Arc::new(state)
            });

        let block = validated_block.try_into_inner().expect("sole handle")?;
        let block = block.with_senders(senders);

        // Wait for the receipt root computation to complete.
        let receipt_root_bloom = {
            let _enter = debug_span!(
                target: "engine::tree::payload_validator",
                "wait_receipt_root",
            )
            .entered();

            receipt_root_rx
                .blocking_recv()
                .inspect_err(|_| {
                    tracing::error!(
                        target: "engine::tree::payload_validator",
                        "Receipt root task dropped sender without result, receipt root calculation likely aborted"
                    );
                })
                .ok()
        };

        ensure_ok_post_block!(
            self.validate_post_execution(
                &block,
                &parent_block,
                &output,
                &mut ctx,
                receipt_root_bloom,
                built_bal
            ),
            block
        );

        let mut hashed_state_validate_result = debug_span!(
            target: "engine::tree::payload_validator",
            "validate_block_post_execution_with_hashed_state"
        )
        .in_scope(|| {
            self.validator
                .validate_block_post_execution_with_hashed_state(&|| hashed_state.get(), &block)
        });

        let root_start = Instant::now();
        let root_outcome = ensure_ok_post_block!(
            state_root_job.finish(&block, output.clone(), &hashed_state),
            block
        );
        let root_elapsed = root_start.elapsed();

        info!(
            target: "engine::tree::payload_validator",
            strategy = state_root_job_name,
            state_root = ?root_outcome.state_root,
            elapsed = ?root_elapsed,
            "State root job finished"
        );

        let state_root = root_outcome.state_root;
        let trie_output = root_outcome.trie_updates;
        let changed_paths = root_outcome.changed_paths;

        // A fallback path recomputed the hashed post state. Replace the streaming-derived one
        // and re-run hashed-state validation against it, since a failed state-root task may
        // have produced an inconsistent byproduct.
        if let Some(refreshed) = root_outcome.hashed_state {
            hashed_state = LazyHandle::ready(refreshed);
            hashed_state_validate_result = debug_span!(
                target: "engine::tree::payload_validator",
                "validate_block_post_execution_with_hashed_state"
            )
            .in_scope(|| {
                self.validator
                    .validate_block_post_execution_with_hashed_state(&|| hashed_state.get(), &block)
            });
        }

        if let Err(err) = hashed_state_validate_result {
            // call post-block hook
            self.on_invalid_block(&parent_block, &block, &output, None, ctx.state_mut());
            return Err(InsertBlockError::new(block.into_sealed_block(), err.into()).into())
        }

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

        let timing_stats = state_provider_stats.filter(|_| slow_block_enabled).map(|stats| {
            self.calculate_timing_stats(
                &block,
                stats,
                cache_stats,
                &output,
                execution_duration,
                root_elapsed,
            )
        });

        if let Some(valid_block_tx) = valid_block_tx {
            let _ = valid_block_tx.send(());
        }

        let executed_block = self.spawn_deferred_trie_task(
            Arc::new(block),
            output,
            hashed_state,
            trie_output,
            changed_paths,
        );
        let raw_bal = decoded_bal.map(|decoded_bal| decoded_bal.as_raw_bal().clone());
        Ok(ValidationOutput::new(executed_block, timing_stats).with_raw_bal(raw_bal))
    }

    /// Spawns a background task to convert a [`BlockOrPayload`] into a [`SealedBlock`] and perform
    /// basic consensus validations on it.
    #[expect(clippy::type_complexity)]
    pub fn spawn_convert_and_validate<T>(
        &self,
        input: &BlockOrPayload<T>,
        parent: SealedHeader<N::BlockHeader>,
    ) -> LazyHandle<Result<SealedBlock<N::Block>, InsertPayloadError<N::Block>>>
    where
        T: PayloadTypes<BuiltPayload: BuiltPayload<Primitives = N>>,
        V: PayloadValidator<T, Block = N::Block> + Clone,
    {
        let input = input.clone();
        let validator = self.validator.clone();
        let consensus = self.consensus.clone();
        let parent_span = Span::current();
        self.payload_processor.executor().spawn_blocking_named("payload-convert", move || {
            let _span = debug_span!(
                target: "engine::tree::payload_validator",
                parent: parent_span,
                "convert_and_validate",
            )
            .entered();
            let block = match input {
                BlockOrPayload::Block(block) => block,
                BlockOrPayload::Payload(payload) => {
                    validator.convert_payload_to_block(payload)?
                }
            };

            if let Err(e) = consensus.validate_header(block.sealed_header()) {
                error!(target: "engine::tree::payload_validator", ?block, "Failed to validate header {}: {e}", block.hash());
                return Err(InsertBlockError::consensus_error(e, block).into())
            }

            // now validate against the parent
            let _enter = debug_span!(target: "engine::tree::payload_validator", "validate_header_against_parent").entered();
            if let Err(e) = consensus.validate_header_against_parent(block.sealed_header(), &parent)
            {
                warn!(target: "engine::tree::payload_validator", ?block, "Failed to validate header {} against parent: {e}", block.hash());
                return Err(InsertBlockError::consensus_error(e, block).into())
            }
            drop(_enter);

            if let Err(e) =
                consensus.validate_block_pre_execution_with_tx_root(&block, None)
            {
                error!(target: "engine::tree::payload_validator", ?block, "Failed to validate block {}: {e}", block.hash());
                return Err(InsertBlockError::consensus_error(e, block).into())
            }

            Ok(block)
        })
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
        state_hook: Option<Box<dyn OnStateHook + 'static>>,
    ) -> Result<
        (
            BlockExecutionOutput<N::Receipt>,
            Vec<Address>,
            ReceiptRootReceiver,
            Option<BlockAccessList>,
        ),
        InsertBlockErrorKind,
    >
    where
        S: StateProvider + Send,
        Err: core::error::Error + Send + Sync + 'static,
        V: PayloadValidator<T, Block = N::Block>,
        T: PayloadTypes<BuiltPayload: BuiltPayload<Primitives = N>>,
        Evm: ConfigureEngineEvm<T::ExecutionData, Primitives = N>,
    {
        debug!(target: "engine::tree::payload_validator", "Executing block");

        let has_bal = env.decoded_bal.is_some();
        let mut db = debug_span!(target: "engine::tree", "build_state_db").in_scope(|| {
            State::builder()
                .with_database(StateProviderDatabase::new(state_provider))
                .with_bundle_update()
                .with_bal_builder_if(has_bal)
                .build()
        });

        let (spec_id, mut executor) = {
            let _span = debug_span!(target: "engine::tree", "create_evm").entered();
            let spec_id = *env.evm_env.spec_id();
            let evm_config = self.evm_config.clone().with_jit_support();
            let evm = evm_config.evm_with_env(&mut db, env.evm_env);
            let ctx = self
                .execution_ctx_for(input)
                .map_err(|e| InsertBlockErrorKind::Other(Box::new(e)))?;
            let executor = self.evm_config.create_executor(evm, ctx);
            (spec_id, executor)
        };

        if !self.config.precompile_cache_disabled() {
            let _span = debug_span!(target: "engine::tree", "setup_precompile_cache").entered();
            executor.evm_mut().precompiles_mut().map_cacheable_precompiles(
                |address, precompile| {
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
                },
            );
        }

        let transaction_count = input.transaction_count();
        let (receipt_tx, result_rx) = self.spawn_receipt_root_task(transaction_count);
        let executed_tx_index = Arc::clone(handle.executed_tx_index());
        executor.evm_mut().db_mut().set_state_hook(state_hook);

        let execution_start = Instant::now();

        // Execute all transactions and finalize
        let (executor, senders) = self.execute_transactions(
            executor,
            transaction_count,
            handle.iter_transactions(),
            &receipt_tx,
            &executed_tx_index,
            has_bal,
        )?;
        drop(receipt_tx);

        // Finish execution and get the result
        let post_exec_start = Instant::now();
        let (_evm, result) = debug_span!(target: "engine::tree", "BlockExecutor::finish")
            .in_scope(|| executor.finish())
            .map(|(evm, result)| (evm.into_db(), result))?;
        self.metrics.record_post_execution(post_exec_start.elapsed());

        // Merge transitions into bundle state
        debug_span!(target: "engine::tree", "merge_transitions")
            .in_scope(|| db.merge_transitions(BundleRetention::Reverts));

        let built_bal = if has_bal { db.take_built_alloy_bal() } else { None };
        let output = BlockExecutionOutput { result, state: db.take_bundle() };

        let execution_duration = execution_start.elapsed();
        self.metrics.record_block_execution(&output, execution_duration);
        self.metrics.record_block_execution_gas_bucket(output.result.gas_used, execution_duration);
        debug!(target: "engine::tree::payload_validator", elapsed = ?execution_duration, "Executed block");

        Ok((output, senders, result_rx, built_bal))
    }

    /// Returns true when the BAL execute path should be used for this block.
    // TODO: extend with stronger gating before enabling on mainnet:
    //   - Fork check: `Amsterdam.active_at_timestamp(env.evm_env.timestamp)`. Today a BAL only
    //     exists post-Amsterdam, so the BAL-presence check is a sufficient proxy. It is a proxy,
    //     not a guarantee.
    //   - Tx-count threshold (`bal_execute_path_min_tx_count`): below the parallelism break-even
    //     point, provider setup and worker scheduling overhead can exceed the gain. Tune
    //     empirically once workers are parallel; meaningless while the commit loop is sequential.
    fn bal_path_eligible(&self, bal: Option<&DecodedBal>) -> Result<bool, InsertBlockErrorKind> {
        let has_bal = bal.is_some();
        let parallel_execution = has_bal && !self.config.disable_bal_parallel_execution();
        if parallel_execution && self.config.disable_bal_parallel_state_root() {
            return Err(InsertBlockErrorKind::Other(
                "disabling parallel state root is impossible when parallel execution is enabled"
                    .into(),
            ));
        }

        Ok(parallel_execution)
    }

    /// Executes the block on the BAL path. Mirrors the return shape of [`Self::execute_block`]
    /// so the dispatch site stays uniform.
    ///
    /// Inside, this:
    /// 1. Creates a shared parent-state cache handle for provider-backed workers.
    /// 2. Relies on BAL prewarm to stream state-root updates and optional state prefetches.
    /// 3. Spawns the receipt-root task.
    /// 4. Calls [`crate::tree::payload_processor::bal::execute_block`].
    /// 5. Returns the rebuilt BAL for post-execution consensus validation.
    #[instrument(level = "debug", target = "engine::tree::payload_validator", skip_all)]
    #[expect(clippy::type_complexity)]
    fn execute_block_bal<Tx, Err, MakeStateProvider, T>(
        &self,
        env: ExecutionEnv<Evm>,
        input: &BlockOrPayload<T>,
        handle: &PayloadHandle<Tx, Err, N::Receipt>,
        make_state_provider: &MakeStateProvider,
    ) -> Result<
        (
            BlockExecutionOutput<N::Receipt>,
            Vec<Address>,
            ReceiptRootReceiver,
            Option<BlockAccessList>,
        ),
        InsertBlockErrorKind,
    >
    where
        Tx: ExecutableTxFor<Evm> + Send,
        Err: core::error::Error + Send + Sync + 'static,
        MakeStateProvider: Fn(bool) -> ProviderResult<StateProviderBox> + Sync,
        Evm: ConfigureEngineEvm<T::ExecutionData, Primitives = N>,
        T: PayloadTypes<BuiltPayload: BuiltPayload<Primitives = N>>,
        V: PayloadValidator<T, Block = N::Block>,
    {
        debug!(target: "engine::tree::payload_validator", "Executing block via BAL path");

        let (receipt_tx, result_rx) = self.spawn_receipt_root_task(env.transaction_count);
        let input_bal = env.decoded_bal.ok_or_else(|| {
            InsertBlockErrorKind::Other("BAL execute path: no decoded BAL available".into())
        })?;

        let make_db = |fill_on_miss| {
            let provider = make_state_provider(fill_on_miss)
                .map_err(crate::tree::payload_processor::bal::BalExecutionError::Provider)?;
            Ok(StateProviderDatabase::new(provider))
        };
        let execution_start = Instant::now();
        let ctx =
            self.execution_ctx_for(input).map_err(|e| InsertBlockErrorKind::Other(Box::new(e)))?;
        let (output, senders, built_bal) = crate::tree::payload_processor::bal::execute_block(
            &self.runtime,
            &self.evm_config,
            &make_db,
            input_bal,
            env.evm_env,
            ctx,
            env.transaction_count,
            handle.clone_transaction_receiver(),
            receipt_tx,
        )?;
        let execution_duration = execution_start.elapsed();

        self.metrics.record_block_execution(&output, execution_duration);
        self.metrics.record_block_execution_gas_bucket(output.result.gas_used, execution_duration);
        debug!(
            target: "engine::tree::payload_validator",
            elapsed = ?execution_duration,
            "Executed block via BAL path",
        );

        Ok((output, senders, result_rx, Some(built_bal)))
    }

    fn spawn_receipt_root_task(
        &self,
        receipts_len: usize,
    ) -> (ReceiptRootSender<N>, ReceiptRootReceiver) {
        // Unbounded channel is used since tx count bounds capacity anyway.
        let (receipt_tx, receipt_rx) = crossbeam_channel::unbounded();
        let (result_tx, result_rx) = tokio::sync::oneshot::channel();
        let task_handle = ReceiptRootTaskHandle::new(receipt_rx, result_tx);
        self.payload_processor
            .executor()
            .spawn_blocking_named("receipt-root", move || task_handle.run(receipts_len));

        (receipt_tx, result_rx)
    }

    /// Executes transactions and collects senders, streaming receipts to a background task.
    ///
    /// This method handles:
    /// - Applying pre-execution changes (e.g., beacon root updates)
    /// - Executing each transaction with timing metrics
    /// - Streaming receipts to the receipt root computation task
    /// - Collecting transaction senders for later use
    ///
    /// Returns the executor (for finalization) and the collected senders.
    fn execute_transactions<'a, E, Tx, InnerTx, Err, DB>(
        &self,
        mut executor: E,
        transaction_count: usize,
        transactions: impl Iterator<Item = Result<Tx, Err>>,
        receipt_tx: &crossbeam_channel::Sender<IndexedReceipt<N::Receipt>>,
        executed_tx_index: &AtomicUsize,
        has_bal: bool,
    ) -> Result<(E, Vec<Address>), BlockExecutionError>
    where
        E: BlockExecutor<Receipt = N::Receipt, Evm: alloy_evm::Evm<DB = &'a mut State<DB>>>,
        Tx: alloy_evm::block::ExecutableTx<E> + alloy_evm::RecoveredTx<InnerTx>,
        InnerTx: TxHashRef,
        DB: revm::Database + 'a,
        Err: core::error::Error + Send + Sync + 'static,
    {
        let mut senders = Vec::with_capacity(transaction_count);

        // Apply pre-execution changes (e.g., beacon root update)
        let pre_exec_start = Instant::now();
        debug_span!(target: "engine::tree", "pre_execution")
            .in_scope(|| executor.apply_pre_execution_changes())?;
        self.metrics.record_pre_execution(pre_exec_start.elapsed());

        // Bump BAL index after pre-execution changes (EIP-7928: index 0 is pre-execution)
        if has_bal {
            executor.evm_mut().db_mut().bump_bal_index();
        }

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

            let _enter = tracing::enabled!(target: "engine::tree", Level::TRACE).then(|| {
                tracing::trace_span!(
                    target: "engine::tree",
                    "execute tx",
                    tx_index = senders.len() - 1,
                )
                .entered()
            });
            if tracing::enabled!(target: "engine::tree", Level::TRACE) {
                trace!(target: "engine::tree", "Executing transaction");
            }

            let tx_start = Instant::now();
            executor.execute_transaction(tx)?;
            self.metrics.record_transaction_execution(tx_start.elapsed());

            // advance the shared counter so prewarm workers skip already-executed txs
            executed_tx_index.store(senders.len(), Ordering::Relaxed);

            let current_len = executor.receipts().len();
            if current_len > last_sent_len {
                last_sent_len = current_len;
                // Send the latest receipt to the background task for incremental root computation.
                if let Some(receipt) = executor.receipts().last() {
                    let tx_index = current_len - 1;
                    let _ = receipt_tx.send(IndexedReceipt::new(tx_index, receipt.clone()));
                }
            }
            // Bump BAL index after each transaction (EIP-7928)
            if has_bal {
                executor.evm_mut().db_mut().bump_bal_index();
            }
        }

        drop(exec_span);

        Ok((executor, senders))
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
    ///
    /// The `hashed_state` handle wraps the background hashed post state computation.
    #[instrument(level = "debug", target = "engine::tree::payload_validator", skip_all)]
    fn validate_post_execution<T: PayloadTypes<BuiltPayload: BuiltPayload<Primitives = N>>>(
        &self,
        block: &RecoveredBlock<N::Block>,
        parent_block: &SealedHeader<N::BlockHeader>,
        output: &BlockExecutionOutput<N::Receipt>,
        ctx: &mut TreeCtx<'_, N>,
        receipt_root_bloom: Option<ReceiptRootBloom>,
        built_bal: Option<BlockAccessList>,
    ) -> Result<(), InsertBlockErrorKind>
    where
        V: PayloadValidator<T, Block = N::Block>,
    {
        let start = Instant::now();

        trace!(target: "engine::tree::payload_validator", block=?block.num_hash(), "Validating block consensus");

        // Validate block post-execution rules
        let _enter =
            debug_span!(target: "engine::tree::payload_validator", "validate_block_post_execution")
                .entered();
        let block_access_list_hash =
            built_bal.as_ref().map(|bal| compute_block_access_list_hash(bal));

        if let Err(err) = self.consensus.validate_block_post_execution(
            block,
            output,
            receipt_root_bloom,
            block_access_list_hash,
        ) {
            // call post-block hook
            self.on_invalid_block(parent_block, block, output, None, ctx.state_mut());
            return Err(err.into())
        }
        drop(_enter);

        // record post-execution validation duration
        self.metrics
            .block_validation
            .post_execution_validation_duration
            .record(start.elapsed().as_secs_f64());

        Ok(())
    }

    /// Spawns transaction conversion and cache prewarming for payload validation.
    ///
    /// State-root tasks are prepared before this method and can provide streams that prewarm uses
    /// for BAL-derived authoritative updates or transaction-derived hints.
    #[instrument(
        level = "debug",
        target = "engine::tree::payload_validator",
        skip_all,
        fields(has_state_root_streams = !state_root_streams.is_empty(), parallel_bal_execution)
    )]
    fn spawn_payload_processor<T: ExecutableTxIterator<Evm>>(
        &self,
        env: ExecutionEnv<Evm>,
        txs: T,
        provider_builder: StateProviderBuilder<N, P>,
        state_root_streams: StateRootStreams,
        parallel_bal_execution: bool,
    ) -> Result<
        PayloadHandle<
            impl ExecutableTxFor<Evm> + use<N, P, Evm, V, T>,
            impl core::error::Error + Send + Sync + 'static + use<N, P, Evm, V, T>,
            N::Receipt,
        >,
        InsertBlockErrorKind,
    > {
        let start = Instant::now();
        let handle = self.payload_processor.spawn_with_state_root_streams(
            env,
            txs,
            provider_builder,
            state_root_streams,
            parallel_bal_execution,
        );

        self.metrics.block_validation.spawn_payload_processor.record(start.elapsed().as_secs_f64());

        Ok(handle)
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

    /// Returns an overlay builder configured for a payload parent.
    fn overlay_builder_for_parent(
        parent_hash: B256,
        state: &EngineApiTreeState<N>,
        changeset_cache: ChangesetCache,
    ) -> OverlayBuilder<N> {
        OverlayBuilder::new(parent_hash, changeset_cache)
            .with_state_trie_overlay_manager(state.tree_state.state_trie_overlays.clone())
    }

    /// Spawns a background task to compute and sort trie data for the executed block.
    ///
    /// This function creates a [`DeferredTrieData`] handle and spawns a blocking task that:
    /// 1. Sort the block's hashed state and trie updates
    /// 2. Publishes the result so subsequent calls return immediately
    ///
    /// If the background task hasn't completed when `trie_data()` is called, callers wait for the
    /// publishing task instead of computing synchronously.
    ///
    /// The validation hot path can return immediately after state root verification,
    /// while consumers (DB writes, overlay providers, proofs) get trie data from the completed
    /// task.
    fn spawn_deferred_trie_task(
        &self,
        block: Arc<RecoveredBlock<N::Block>>,
        execution_outcome: Arc<BlockExecutionOutput<N::Receipt>>,
        hashed_state: LazyHashedPostState,
        trie_output: Arc<TrieUpdates>,
        changed_paths: Option<Arc<TriePrefixSetsMut>>,
    ) -> ExecutedBlock<N> {
        // Create deferred handle and task that owns the unsorted inputs.
        // Resolve the lazy handle into Arc<HashedPostState>. By this point the hashed state has
        // already been computed and used for state root verification, so .get() returns instantly.
        let hashed_state = match hashed_state.try_into_inner() {
            Ok(state) => state,
            Err(handle) => handle.get().clone(),
        };
        let (deferred_trie_data, deferred_trie_task) =
            DeferredTrieData::pending(hashed_state, trie_output, changed_paths);
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
        self.payload_processor
            .executor()
            .spawn_blocking_named(DEFERRED_TRIE_WORKER_NAME, compute_trie_input_task);

        ExecutedBlock::with_deferred_trie_data(block, execution_outcome, deferred_trie_data)
    }

    fn calculate_timing_stats(
        &self,
        block: &RecoveredBlock<N::Block>,
        provider_stats: Arc<StateProviderStats>,
        cache_stats: Option<Arc<CacheStats>>,
        output: &BlockExecutionOutput<N::Receipt>,
        execution_duration: Duration,
        state_hash_duration: Duration,
    ) -> Box<ExecutionTimingStats> {
        let accounts_read = provider_stats.total_account_fetches();
        let storage_read = provider_stats.total_storage_fetches();
        let code_read = provider_stats.total_code_fetches();
        let code_bytes_read = provider_stats.total_code_fetched_bytes();

        // Write stats from BundleState (final state changes)
        let accounts_changed = output.state.state.len();
        let accounts_deleted =
            output.state.state.values().filter(|acc| acc.was_destroyed()).count();
        let storage_slots_changed =
            output.state.state.values().map(|account| account.storage.len()).sum::<usize>();
        let storage_slots_deleted = output
            .state
            .state
            .values()
            .flat_map(|account| account.storage.values())
            .filter(|slot| {
                slot.present_value.is_zero() && !slot.previous_or_original_value.is_zero()
            })
            .count();

        // Helper: check if account represents a new contract deployment
        let is_new_deployment = |acc: &BundleAccount| -> bool {
            let has_code_now = acc.info.as_ref().is_some_and(|info| info.code_hash != KECCAK_EMPTY);
            let had_no_code_before = acc
                .original_info
                .as_ref()
                .map(|info| info.code_hash == KECCAK_EMPTY)
                .unwrap_or(true);
            has_code_now && had_no_code_before
        };

        let bytecodes_changed =
            output.state.state.values().filter(|acc| is_new_deployment(acc)).count();

        // Unique new code hashes to count actual bytes persisted (deduplicated)
        let unique_new_code_hashes: B256Set = output
            .state
            .state
            .values()
            .filter(|acc| is_new_deployment(acc))
            .filter_map(|acc| acc.info.as_ref().map(|info| info.code_hash))
            .collect();
        let code_bytes_written: usize = unique_new_code_hashes
            .iter()
            .filter_map(|hash| {
                output.state.contracts.get(hash).map(|bytecode| bytecode.original_bytes().len())
            })
            .sum();

        // Total time spent fetching state during execution
        let state_read_duration = provider_stats.total_account_fetch_latency() +
            provider_stats.total_storage_fetch_latency() +
            provider_stats.total_code_fetch_latency();

        // EIP-7702 delegation tracking from bytecode changes
        // Count new EIP-7702 bytecodes as delegations set
        let eip7702_delegations_set =
            output.state.contracts.values().filter(|bytecode| bytecode.is_eip7702()).count();
        // Delegations cleared: accounts where bytecode changed FROM EIP-7702 TO empty
        // This detects when an EIP-7702 delegation is removed by setting code to empty
        // Note: Clearing a delegation does NOT destroy the account - it just empties the
        // bytecode
        let eip7702_delegations_cleared = output
            .state
            .state
            .values()
            .filter(|acc| {
                // Check if original bytecode was EIP-7702
                let original_was_eip7702 = acc
                    .original_info
                    .as_ref()
                    .and_then(|info| info.code.as_ref())
                    .map(|bytecode| bytecode.is_eip7702())
                    .unwrap_or(false);

                // Check if current code is empty (delegation cleared)
                let code_now_empty =
                    acc.info.as_ref().map(|info| info.code_hash == KECCAK_EMPTY).unwrap_or(false);

                original_was_eip7702 && code_now_empty
            })
            .count();

        // Get cache statistics for detailed block logging
        let (account_cache_hits, account_cache_misses) = cache_stats
            .as_ref()
            .map(|s| (s.account_hits(), s.account_misses()))
            .unwrap_or_default();
        let (storage_cache_hits, storage_cache_misses) = cache_stats
            .as_ref()
            .map(|s| (s.storage_hits(), s.storage_misses()))
            .unwrap_or_default();
        let (code_cache_hits, code_cache_misses) =
            cache_stats.as_ref().map(|s| (s.code_hits(), s.code_misses())).unwrap_or_default();

        // Build execution timing stats for detailed block logging
        Box::new(ExecutionTimingStats {
            block_number: block.number(),
            block_hash: block.hash(),
            gas_used: output.result.gas_used,
            tx_count: block.transaction_count(),
            execution_duration,
            state_read_duration,
            state_hash_duration,
            accounts_read,
            storage_read,
            code_read,
            code_bytes_read,
            accounts_changed,
            accounts_deleted,
            storage_slots_changed,
            storage_slots_deleted,
            bytecodes_changed,
            code_bytes_written,
            eip7702_delegations_set,
            eip7702_delegations_cleared,
            account_cache_hits,
            account_cache_misses,
            storage_cache_hits,
            storage_cache_misses,
            code_cache_hits,
            code_cache_misses,
        })
    }
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
    fn on_inserted_executed_block(
        &self,
        block: BuiltPayloadExecutedBlock<N>,
    ) -> ProviderResult<ExecutedBlock<N>>;

    /// Returns [`SavedCache`] for the given block hash.
    fn cache_for(&self, _block_hash: B256) -> Option<SavedCache>;

    /// Prepares the optional payload-builder state-root handle through the installed
    /// [`StateRootStrategy`].
    ///
    /// Returns `None` when the strategy declines, in which case the payload builder computes
    /// the state root itself.
    ///
    /// `timestamp` is the timestamp of the payload being built, taken from the payload
    /// attributes.
    fn payload_state_root_handle_for(
        &self,
        parent_hash: B256,
        parent_header: &N::BlockHeader,
        timestamp: u64,
        state: &EngineApiTreeState<N>,
    ) -> Option<PayloadStateRootHandle>;
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
    OverlayStateProviderFactory<P, N>: DatabaseProviderROFactory<Provider: TrieCursorFactory + HashedCursorFactory>
        + Clone
        + Send
        + Sync
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

    fn on_inserted_executed_block(
        &self,
        block: BuiltPayloadExecutedBlock<N>,
    ) -> ProviderResult<ExecutedBlock<N>> {
        self.payload_processor.on_inserted_executed_block(
            block.recovered_block.block_with_parent(),
            &block.execution_output.state,
        );

        Ok(self.spawn_deferred_trie_task(
            block.recovered_block,
            block.execution_output,
            LazyHashedPostState::ready(block.hashed_state),
            block.trie_updates,
            block.changed_paths,
        ))
    }

    fn cache_for(&self, block_hash: B256) -> Option<SavedCache> {
        Some(self.payload_processor.cache_for(block_hash))
    }

    fn payload_state_root_handle_for(
        &self,
        parent_hash: B256,
        parent_header: &N::BlockHeader,
        timestamp: u64,
        state: &EngineApiTreeState<N>,
    ) -> Option<PayloadStateRootHandle> {
        let provider_builder = match self.state_provider_builder(parent_hash, state) {
            Ok(Some(provider_builder)) => provider_builder,
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
        let overlay_factory = OverlayStateProviderFactory::new(
            self.provider.clone(),
            Self::overlay_builder_for_parent(parent_hash, state, self.changeset_cache.clone())
                .with_parent_state_root(parent_header.state_root()),
        );

        match self.state_root_strategy.prepare_payload_builder(PayloadStateRootJobContext::new(
            &self.payload_processor,
            parent_hash,
            parent_header,
            timestamp,
            provider_builder,
            overlay_factory,
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

    /// Returns true if this is a payload.
    pub const fn is_payload(&self) -> bool {
        matches!(self, Self::Payload(_))
    }

    /// Returns true if this is a block.
    pub const fn is_block(&self) -> bool {
        matches!(self, Self::Block(_))
    }

    /// Returns the decoded block access list, if present and successfully decoded.
    pub fn try_decoded_access_list(&self) -> Result<Option<DecodedBal>, alloy_rlp::Error> {
        match self {
            Self::Payload(payload) => payload
                .block_access_list()
                .map(|block_access_list| DecodedBal::from_rlp_bytes(block_access_list.clone()))
                .transpose(),
            Self::Block(_) => Ok(None),
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

    /// Returns the gas limit used by the block.
    pub fn gas_limit(&self) -> u64
    where
        T::ExecutionData: ExecutionPayload,
    {
        match self {
            Self::Payload(payload) => payload.gas_limit(),
            Self::Block(block) => block.gas_limit(),
        }
    }
}
