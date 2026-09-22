//! State-root strategies for engine-tree block validation.
//!
//! A [`StateRootStrategy`] is installed once per node, via
//! `BasicEngineValidator::with_state_root_strategy`, and consulted for every block that engine
//! validation executes. For each block the strategy prepares a [`StateRootJob`] before execution
//! starts, and validation finishes the job after execution to obtain the state root that is
//! checked against the block header. On every FCU that carries payload attributes, the strategy
//! is also asked through [`StateRootStrategy::prepare_payload_builder`] for an optional
//! [`PayloadStateRootHandle`] that the payload builder uses while building a block.
//!
//! # Job lifecycle
//!
//! 1. [`StateRootStrategy::prepare`] runs before block execution. The job can spawn background work
//!    here and can expose hooks that observe execution.
//! 2. Execution runs. Jobs that observe execution receive updates through their hooks.
//! 3. [`StateRootJob::finish`] runs after execution and returns the [`StateRootJobOutcome`]. It
//!    must produce a result even if no execution updates were observed, since the full
//!    [`BlockExecutionOutput`] is passed to it.
//!
//! Dropping a prepared job without calling `finish` aborts it. Implementations must treat
//! channel disconnects from dropped hooks as cancellation and must not leak background work.
//!
//! # Stream delivery contract
//!
//! A prepared job exposes update-stream capabilities over its sink. `prepare` installs exactly
//! one authoritative capability per block, matching the execution mode:
//!
//! - On the parallel BAL execution path, prewarm converts the block access list and delivers
//!   pre-hashed updates through the hashed update stream, terminated by
//!   [`StateRootUpdateStream::finish`].
//! - On the serial execution path, per-transaction `EvmState` updates arrive through the execution
//!   hook, terminated when the hook is dropped after execution.
//!
//! Which path runs depends on runtime conditions (BAL present, caching and prewarming enabled),
//! so a sink must handle both. Access hints from prewarming are best-effort: they may be
//! missing, duplicated, or stale, and must not be treated as state updates.
//!
//! # Custom strategies
//!
//! Custom implementations can hold a [`DefaultStateRootStrategy`] and forward calls to it for
//! blocks where the default behavior is wanted, for example before a fork activates. See
//! `examples/custom-state-root` for the wiring.
//!
//! # Empty accounts
//!
//! The sparse-trie path treats an account with zero nonce, zero balance, empty bytecode, and an
//! empty storage root as absent from the account trie. This relies on the post-Merge invariant
//! established by EIP-7523 (<https://eips.ethereum.org/EIPS/eip-7523>): post-Merge state cannot
//! contain empty accounts. A custom strategy that replays pre-Merge blocks through the Engine API
//! must route those blocks to a state-root implementation that supports historical empty accounts.
//!
//! Returning empty trie updates in the outcome means the trie tables are no longer maintained:
//! `eth_getProof` and anything else that reads the stored trie will not work for new blocks.
//! Sparse-trie cache pruning uses node epochs to retain the in-memory block range.

mod sparse_trie;

use self::sparse_trie::{SparseTrieCacheTask, SparseTrieTaskMetrics};
use crate::tree::{metrics::BlockValidationMetrics, EngineApiTreeState, ExecutionEnv, TreeConfig};
use alloy_primitives::B256;
use crossbeam_channel::{Receiver as CrossbeamReceiver, Sender as CrossbeamSender};
use parking_lot::Mutex;
use reth_chain_state::{ExecutedBlock, PreservedSparseTrie};
use reth_errors::ProviderResult;
use reth_evm::{ConfigureEvm, OnStateHook};
use reth_primitives_traits::{
    AlloyBlockHeader, FastInstant as Instant, NodePrimitives, RecoveredBlock, SealedHeader,
};
use reth_provider::{
    BlockExecutionOutput, BlockNumReader, ChangeSetReader, DatabaseProviderFactory,
    DatabaseProviderROFactory, HashedPostStateProvider, ProviderError, PruneCheckpointReader,
    StageCheckpointReader, StateRootProvider, StorageChangeSetReader, StorageSettingsCache,
};
use reth_storage_overlay::{OverlayManager, OverlayStateProviderFactory};
use reth_tasks::utils::increase_thread_priority;
use reth_trie::{
    hashed_cursor::HashedCursorFactory, trie_cursor::TrieCursorFactory, updates::TrieUpdates,
    HashedPostState,
};
use reth_trie_parallel::proof_task::{ProofResultMessage, ProofTaskCtx, ProofWorkerHandle};
pub use reth_trie_parallel::{
    error::StateRootTaskError,
    state_root_task::{
        evm_state_to_hashed_post_state, PayloadStateRootHandle, StateAccessHint,
        StateRootComputeOutcome, StateRootHandle, StateRootHintStream, StateRootMessage,
        StateRootSink, StateRootTaskCancelGuard, StateRootUpdateHook, StateRootUpdateStream,
    },
};
use reth_trie_sparse::{
    ArenaParallelSparseTrie, RevealableSparseTrie, SparseStateTrie, TrieNodeEpoch,
};
use std::{
    fmt,
    sync::{
        mpsc::{self, RecvTimeoutError},
        Arc,
    },
    time::Duration,
};
use tracing::{debug, debug_span, instrument, warn, Span};

/// Handle to a [`HashedPostState`] computed on a background thread.
pub type LazyHashedPostState = reth_tasks::LazyHandle<Arc<HashedPostState>>;

/// Strategy used by engine-tree validation to prepare per-block state-root work.
pub trait StateRootStrategy<N, P, Evm>: Send + Sync
where
    N: NodePrimitives,
    Evm: ConfigureEvm<Primitives = N>,
{
    /// Prepares a per-block state-root job before execution starts.
    ///
    /// A custom strategy that maintains a reusable sparse trie is responsible for consuming the
    /// pending prune request from the context when it starts the corresponding job.
    fn prepare(
        &self,
        ctx: StateRootJobContext<'_, N, P, Evm>,
    ) -> ProviderResult<PreparedStateRootJob<N>>;

    /// Prepares the optional payload-builder state-root handle used for FCU-triggered block
    /// building.
    ///
    /// This is consulted on every FCU that carries payload attributes. Returning `None` means the
    /// payload builder computes the state root itself; the stock builders fall back to a
    /// synchronous MPT state root. The default implementation returns `None`.
    fn prepare_payload_builder(
        &self,
        _ctx: PayloadStateRootJobContext<'_, N, P>,
    ) -> ProviderResult<Option<PayloadStateRootHandle>> {
        Ok(None)
    }
}

/// Data available while preparing one payload-builder state-root handle.
pub struct PayloadStateRootJobContext<'a, N, P>
where
    N: NodePrimitives,
{
    executor: &'a reth_tasks::Runtime,
    overlay_manager: &'a OverlayManager<N>,
    parent_hash: B256,
    parent_header: &'a N::BlockHeader,
    timestamp: u64,
    state: &'a mut EngineApiTreeState<N>,
    state_provider_factory: OverlayStateProviderFactory<P, N>,
    config: &'a TreeConfig,
}

impl<N, P> fmt::Debug for PayloadStateRootJobContext<'_, N, P>
where
    N: NodePrimitives,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PayloadStateRootJobContext")
            .field("parent_hash", &self.parent_hash)
            .field("parent_state_root", &self.parent_state_root())
            .field("timestamp", &self.timestamp)
            .field("pending_sparse_trie_prune", &self.state.pending_sparse_trie_prune())
            .finish_non_exhaustive()
    }
}

impl<'a, N, P> PayloadStateRootJobContext<'a, N, P>
where
    N: NodePrimitives,
{
    /// Creates a payload-builder state-root job context.
    #[expect(clippy::too_many_arguments)]
    pub(crate) const fn new(
        executor: &'a reth_tasks::Runtime,
        overlay_manager: &'a OverlayManager<N>,
        parent_hash: B256,
        parent_header: &'a N::BlockHeader,
        timestamp: u64,
        state: &'a mut EngineApiTreeState<N>,
        state_provider_factory: OverlayStateProviderFactory<P, N>,
        config: &'a TreeConfig,
    ) -> Self {
        Self {
            executor,
            overlay_manager,
            parent_hash,
            parent_header,
            timestamp,
            state,
            state_provider_factory,
            config,
        }
    }

    /// Returns the parent block hash for the payload being built.
    pub const fn parent_hash(&self) -> B256 {
        self.parent_hash
    }

    /// Returns the parent block header for the payload being built.
    ///
    /// This is the chain's concrete header type, so chain-specific strategies can read
    /// chain-specific fields, and number-activated forks can dispatch on the parent number.
    pub const fn parent_header(&self) -> &N::BlockHeader {
        self.parent_header
    }

    /// Returns the parent state root for the payload being built.
    pub fn parent_state_root(&self) -> B256 {
        self.parent_header.state_root()
    }

    /// Returns the timestamp of the payload being built, taken from the payload attributes.
    ///
    /// Strategies that switch behavior at a fork activation can dispatch on this value.
    pub const fn timestamp(&self) -> u64 {
        self.timestamp
    }

    /// Returns the task runtime used by state-root work.
    pub const fn executor(&self) -> &reth_tasks::Runtime {
        self.executor
    }

    /// Consumes the pending sparse trie prune request as in-memory parent-chain blocks, if any.
    ///
    /// Custom strategies that maintain a reusable sparse trie should call this when starting the
    /// corresponding job. Strategies that do not use the request should leave it pending.
    pub fn take_sparse_trie_prune_blocks(&mut self) -> Option<Vec<ExecutedBlock<N>>> {
        self.state.take_sparse_trie_prune_blocks(self.parent_hash)
    }
}

/// Data available while preparing one state-root job.
pub struct StateRootJobContext<'a, N, P, Evm>
where
    N: NodePrimitives,
    Evm: ConfigureEvm<Primitives = N>,
{
    executor: &'a reth_tasks::Runtime,
    overlay_manager: &'a OverlayManager<N>,
    env: &'a ExecutionEnv<Evm>,
    parent_header: &'a SealedHeader<N::BlockHeader>,
    state_provider_factory: OverlayStateProviderFactory<P, N>,
    config: &'a TreeConfig,
    parallel_bal_execution: bool,
    state: &'a mut EngineApiTreeState<N>,
}

impl<N, P, Evm> fmt::Debug for StateRootJobContext<'_, N, P, Evm>
where
    N: NodePrimitives,
    Evm: ConfigureEvm<Primitives = N>,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StateRootJobContext")
            .field("parallel_bal_execution", &self.parallel_bal_execution)
            .field("has_pending_sparse_trie_prune", &self.state.pending_sparse_trie_prune())
            .finish_non_exhaustive()
    }
}

impl<'a, N, P, Evm> StateRootJobContext<'a, N, P, Evm>
where
    N: NodePrimitives,
    Evm: ConfigureEvm<Primitives = N>,
{
    /// Creates a new state-root job context.
    #[expect(clippy::too_many_arguments)]
    pub(crate) const fn new(
        executor: &'a reth_tasks::Runtime,
        overlay_manager: &'a OverlayManager<N>,
        env: &'a ExecutionEnv<Evm>,
        parent_header: &'a SealedHeader<N::BlockHeader>,
        state_provider_factory: OverlayStateProviderFactory<P, N>,
        config: &'a TreeConfig,
        parallel_bal_execution: bool,
        state: &'a mut EngineApiTreeState<N>,
    ) -> Self {
        Self {
            executor,
            overlay_manager,
            env,
            parent_header,
            state_provider_factory,
            config,
            parallel_bal_execution,
            state,
        }
    }

    /// Returns the execution environment for the block.
    pub const fn env(&self) -> &ExecutionEnv<Evm> {
        self.env
    }

    /// Returns the sealed parent block header.
    pub const fn parent_header(&self) -> &SealedHeader<N::BlockHeader> {
        self.parent_header
    }

    /// Returns the task runtime used by state-root work.
    pub const fn executor(&self) -> &reth_tasks::Runtime {
        self.executor
    }

    /// Returns true when validation will use the parallel BAL execution path.
    pub const fn parallel_bal_execution(&self) -> bool {
        self.parallel_bal_execution
    }

    /// Consumes the pending sparse trie prune request as in-memory parent-chain blocks, if any.
    ///
    /// Custom strategies that maintain a reusable sparse trie should call this when starting the
    /// corresponding job. Strategies that do not use the request should leave it pending.
    pub fn take_sparse_trie_prune_blocks(&mut self) -> Option<Vec<ExecutedBlock<N>>> {
        self.state.take_sparse_trie_prune_blocks(self.env.parent_hash)
    }
}

/// Prepared per-block state-root work and its update-stream capabilities.
///
/// The capabilities are populated by the strategy's `prepare` according to the execution
/// mode: the execution hook on the serial path, the hashed update stream on the parallel BAL
/// path, never both. Each capability is taken once by the code that produces its messages
/// and is not retained here, so the task's update channel closes when the producers are done.
pub struct PreparedStateRootJob<N: NodePrimitives> {
    job: Box<dyn StateRootJob<N>>,
    execution_hook: Option<StateRootUpdateHook>,
    hint_stream: Option<StateRootHintStream>,
    hashed_update_stream: Option<StateRootUpdateStream>,
    hashed_state_rx: Option<mpsc::Receiver<Arc<HashedPostState>>>,
}

impl<N: NodePrimitives> fmt::Debug for PreparedStateRootJob<N> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PreparedStateRootJob")
            .field("name", &self.job.name())
            .field("has_execution_hook", &self.execution_hook.is_some())
            .field("has_hint_stream", &self.hint_stream.is_some())
            .field("has_hashed_update_stream", &self.hashed_update_stream.is_some())
            .field("has_hashed_state_rx", &self.hashed_state_rx.is_some())
            .finish()
    }
}

impl<N: NodePrimitives> PreparedStateRootJob<N> {
    /// Creates a prepared state-root job without update-stream capabilities.
    pub const fn new(
        job: Box<dyn StateRootJob<N>>,
        hashed_state_rx: Option<mpsc::Receiver<Arc<HashedPostState>>>,
    ) -> Self {
        Self {
            job,
            execution_hook: None,
            hint_stream: None,
            hashed_update_stream: None,
            hashed_state_rx,
        }
    }

    /// Attaches the execution hook capability (serial execution path).
    pub fn with_execution_hook(mut self, hook: StateRootUpdateHook) -> Self {
        self.execution_hook = Some(hook);
        self
    }

    /// Attaches the hint stream capability.
    pub fn with_hint_stream(mut self, hint_stream: StateRootHintStream) -> Self {
        self.hint_stream = Some(hint_stream);
        self
    }

    /// Attaches the hashed update stream capability (parallel BAL path).
    pub fn with_hashed_update_stream(mut self, stream: StateRootUpdateStream) -> Self {
        self.hashed_update_stream = Some(stream);
        self
    }

    /// Returns the job name used in logs.
    pub fn name(&self) -> &'static str {
        self.job.name()
    }

    /// Takes the execution hook, present only when the job wants normal execution updates.
    pub fn take_execution_hook(&mut self) -> Option<Box<dyn OnStateHook + 'static>> {
        self.execution_hook.take().map(|hook| Box::new(hook) as Box<dyn OnStateHook + 'static>)
    }

    /// Takes the hint stream for transaction prewarming.
    pub const fn take_hint_stream(&mut self) -> Option<StateRootHintStream> {
        self.hint_stream.take()
    }

    /// Takes the hashed update stream, present only on the parallel BAL path.
    pub const fn take_hashed_update_stream(&mut self) -> Option<StateRootUpdateStream> {
        self.hashed_update_stream.take()
    }

    /// Takes the optional hashed-state receiver produced by the job.
    ///
    /// The sender behind a returned receiver must either deliver one value or be dropped;
    /// validation blocks on it while hashing the post state, so a job that keeps the sender
    /// alive without sending stalls block validation.
    pub const fn take_hashed_state_rx(&mut self) -> Option<mpsc::Receiver<Arc<HashedPostState>>> {
        self.hashed_state_rx.take()
    }

    /// Completes the job after execution.
    pub fn finish(
        &mut self,
        block: &RecoveredBlock<N::Block>,
        output: Arc<BlockExecutionOutput<N::Receipt>>,
        hashed_state: &LazyHashedPostState,
    ) -> ProviderResult<StateRootJobOutcome> {
        self.job.finish(block, output, hashed_state)
    }
}

/// Per-block state-root job prepared before execution and finished after execution.
pub trait StateRootJob<N: NodePrimitives>: Send {
    /// Human-readable strategy name used in logs.
    fn name(&self) -> &'static str;

    /// Completes the job after execution.
    ///
    /// Called at most once per prepared job; implementations may panic if called again.
    fn finish(
        &mut self,
        block: &RecoveredBlock<N::Block>,
        output: Arc<BlockExecutionOutput<N::Receipt>>,
        hashed_state: &LazyHashedPostState,
    ) -> ProviderResult<StateRootJobOutcome>;
}

/// Outcome of a per-block state-root job.
#[derive(Debug)]
pub struct StateRootJobOutcome {
    /// Computed state root.
    pub state_root: B256,
    /// Trie updates associated with the computed state root.
    pub trie_updates: Arc<TrieUpdates>,
    /// Hashed post state recomputed by a fallback path.
    ///
    /// When set, the root was not derived from the streamed updates, so validation replaces its
    /// streaming-derived hashed post state with this one and re-runs hashed-state checks.
    pub hashed_state: Option<Arc<HashedPostState>>,
}

impl StateRootJobOutcome {
    /// Creates a state-root job outcome.
    pub const fn new(state_root: B256, trie_updates: Arc<TrieUpdates>) -> Self {
        Self { state_root, trie_updates, hashed_state: None }
    }

    /// Sets the hashed post state recomputed by a fallback path.
    pub fn with_hashed_state(mut self, hashed_state: Option<Arc<HashedPostState>>) -> Self {
        self.hashed_state = hashed_state;
        self
    }
}

/// Receiver for the raced serial state-root fallback: root, trie updates, and the hashed
/// post state the fallback recomputed.
type SerialFallbackRx = mpsc::Receiver<ProviderResult<(B256, TrieUpdates, Arc<HashedPostState>)>>;

/// Default state-root strategy used by engine-tree validation.
///
/// Covers the built-in modes: the sparse-trie state-root task, plus the skipped and
/// synchronous modes selected by [`TreeConfig`].
///
/// Custom strategies can hold this type and delegate to it for blocks where they want the
/// default behavior.
#[derive(Default)]
pub struct DefaultStateRootStrategy {
    metrics: SparseTrieTaskMetrics,
}

impl fmt::Debug for DefaultStateRootStrategy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DefaultStateRootStrategy").finish_non_exhaustive()
    }
}

impl DefaultStateRootStrategy {
    /// Transaction count threshold below which proof workers are halved, since fewer transactions
    /// produce fewer state changes and most workers would be idle overhead.
    const SMALL_BLOCK_PROOF_WORKER_TX_THRESHOLD: usize = 30;

    /// Spawns the default state-root computation pipeline.
    ///
    /// The authoritative update capability taken from the returned handle must be dropped or
    /// explicitly finished after execution so the task observes the end of the update stream.
    /// An unknown transaction count uses the full proof-worker pool.
    #[instrument(level = "debug", target = "engine::tree::payload_processor", skip_all)]
    fn spawn_state_root<N, F>(
        &self,
        executor: &reth_tasks::Runtime,
        multiproof_provider_factory: F,
        options: StateRootTaskOptions<'_, N>,
    ) -> StateRootHandle
    where
        N: NodePrimitives,
        F: DatabaseProviderROFactory<Provider: TrieCursorFactory + HashedCursorFactory>
            + Clone
            + Send
            + Sync
            + 'static,
    {
        let StateRootTaskOptions {
            parent_header,
            sparse_trie,
            transaction_count,
            config,
            pending_sparse_trie_prune_blocks,
        } = options;
        let (updates_tx, from_multi_proof) = crossbeam_channel::unbounded();
        let (cancel_guard, cancel_rx) = StateRootTaskCancelGuard::channel();
        let (proof_result_tx, proof_result_rx) =
            crossbeam_channel::unbounded::<ProofResultMessage>();

        let task_ctx = ProofTaskCtx::new(multiproof_provider_factory);
        #[cfg(feature = "trie-debug")]
        let task_ctx = task_ctx.with_proof_jitter(config.proof_jitter());
        let halve_workers = transaction_count
            .is_some_and(|count| count <= Self::SMALL_BLOCK_PROOF_WORKER_TX_THRESHOLD);
        let proof_handle =
            ProofWorkerHandle::new(executor, task_ctx, halve_workers, proof_result_tx.clone());

        let (state_root_tx, state_root_rx) = mpsc::channel();
        let (hashed_state_tx, hashed_state_rx) = mpsc::channel();
        let parent_state_root = parent_header.state_root();

        self.spawn_sparse_trie_task(
            executor,
            proof_handle,
            proof_result_tx,
            proof_result_rx,
            state_root_tx,
            hashed_state_tx,
            from_multi_proof,
            cancel_rx,
            SparseTrieTaskOptions {
                parent_header,
                sparse_trie,
                chunk_size: config.multiproof_chunk_size(),
                pending_sparse_trie_prune_blocks: if config.disable_sparse_trie_cache_pruning() {
                    None
                } else {
                    pending_sparse_trie_prune_blocks
                },
            },
        );

        StateRootHandle::new(
            parent_state_root,
            updates_tx,
            cancel_guard,
            state_root_rx,
            hashed_state_rx,
        )
    }

    /// Spawns the sparse-trie task and stages its trie for publication by the validating job.
    #[expect(clippy::too_many_arguments)]
    fn spawn_sparse_trie_task<N: NodePrimitives>(
        &self,
        executor: &reth_tasks::Runtime,
        proof_worker_handle: ProofWorkerHandle,
        proof_result_tx: CrossbeamSender<ProofResultMessage>,
        proof_result_rx: CrossbeamReceiver<ProofResultMessage>,
        state_root_tx: mpsc::Sender<Result<StateRootComputeOutcome, StateRootTaskError>>,
        hashed_state_tx: mpsc::Sender<Arc<HashedPostState>>,
        from_multi_proof: CrossbeamReceiver<StateRootMessage>,
        cancel_rx: CrossbeamReceiver<()>,
        options: SparseTrieTaskOptions<N>,
    ) {
        let SparseTrieTaskOptions {
            parent_header,
            sparse_trie,
            chunk_size,
            pending_sparse_trie_prune_blocks,
        } = options;
        let trie_metrics = self.metrics.clone();
        let executor = executor.clone();

        let parent_span = Span::current();
        executor.clone().spawn_blocking_named("sparse-trie", move || {
            reth_tasks::once!(increase_thread_priority);

            let parent_hash = parent_header.hash();
            let parent_state_root = parent_header.state_root();
            let new_epoch = TrieNodeEpoch::new(parent_header.number().saturating_add(1));

            let _enter = debug_span!(
                target: "engine::tree::payload_processor",
                parent: parent_span,
                "sparse_trie_task"
            )
            .entered();

            let new_sparse_state_trie = || {
                debug!(
                    target: "engine::tree::payload_processor",
                    "Creating new sparse trie - no preserved trie available"
                );
                let default_trie =
                    RevealableSparseTrie::blind_from(ArenaParallelSparseTrie::default());
                SparseStateTrie::default()
                    .with_accounts_trie(default_trie.clone())
                    .with_default_storage_trie(default_trie)
                    .with_updates(true)
            };

            let mut sparse_trie_anchor_hash = parent_hash;
            let mut reused_preserved_sparse_trie = false;
            let SparseTrieState::Input(preserved_sparse_trie) =
                std::mem::take(&mut *sparse_trie.trie.lock())
            else {
                unreachable!("sparse trie input already consumed")
            };
            let sparse_state_trie = match preserved_sparse_trie {
                Some(preserved) => {
                    let start = Instant::now();
                    let preserved_anchor_hash = preserved.anchor_hash();
                    let preserved = preserved.into_trie_for(parent_hash);
                    trie_metrics
                        .sparse_trie_cache_wait_duration_histogram
                        .record(start.elapsed().as_secs_f64());

                    match preserved {
                        Ok(Some(trie)) => {
                            sparse_trie_anchor_hash = preserved_anchor_hash;
                            reused_preserved_sparse_trie = true;
                            trie
                        }
                        Ok(None) => new_sparse_state_trie(),
                        Err(err) => {
                            let _ =
                                state_root_tx.send(Err(StateRootTaskError::Other(err.to_string())));
                            return;
                        }
                    }
                }
                None => new_sparse_state_trie(),
            };
            let prune_target = sparse_trie_prune_target(
                sparse_trie_anchor_hash,
                reused_preserved_sparse_trie,
                parent_header.num_hash(),
                pending_sparse_trie_prune_blocks.as_deref(),
            );
            let mut task = SparseTrieCacheTask::new_with_trie(
                &executor,
                from_multi_proof,
                cancel_rx,
                hashed_state_tx,
                proof_worker_handle,
                proof_result_tx,
                proof_result_rx,
                trie_metrics.clone(),
                sparse_state_trie,
                parent_state_root,
                new_epoch,
                chunk_size,
            );

            let result = task.run();
            // Stage before sending the result. Validation accepts it in `finish`; payload
            // building associates it with the block hash only after assembling the block.
            // Hold the slot until delivery succeeds so a builder that abandoned the receiver
            // cannot publish a trie whose finalization will be cancelled below.
            let mut staged = sparse_trie.trie.lock();
            let trie_completer = if let Ok(outcome) = &result {
                let preserved_anchor_hash =
                    prune_target.map_or(sparse_trie_anchor_hash, |(_, anchor_hash)| anchor_hash);
                let (tx, rx) = mpsc::channel();
                *staged = SparseTrieState::Output {
                    state_root: outcome.state_root,
                    anchor_hash: preserved_anchor_hash,
                    trie: rx,
                };
                Some(tx)
            } else {
                None
            };

            if state_root_tx.send(result).is_err() {
                *staged = SparseTrieState::default();
                drop(staged);
                debug!(
                    target: "engine::tree::payload_processor",
                    "State root receiver dropped, dropping trie"
                );
                let (trie, deferred) = task.into_cleared_trie();
                executor.spawn_drop(trie);
                executor.spawn_drop(deferred);
                return;
            }
            drop(staged);

            let _enter =
                debug_span!(target: "engine::tree::payload_processor", "preserve").entered();
            let (trie_to_drop, deferred) = if let Some(completer) = trie_completer {
                let start = Instant::now();
                let (mut trie, deferred) = task.into_trie_for_reuse();
                if let Some((prune_before, _)) = prune_target {
                    let prune_start = Instant::now();
                    trie.prune(prune_before);
                    trie_metrics
                        .sparse_trie_prune_duration_histogram
                        .record(prune_start.elapsed().as_secs_f64());
                }
                trie_metrics
                    .into_trie_for_reuse_duration_histogram
                    .record(start.elapsed().as_secs_f64());
                trie_metrics
                    .sparse_trie_retained_storage_tries
                    .set(trie.retained_storage_tries_count() as f64);
                (completer.send(trie).err().map(|err| err.0), deferred)
            } else {
                debug!(
                    target: "engine::tree::payload_processor",
                    "State root task has no reusable trie to publish, dropping trie"
                );
                let (trie, deferred) = task.into_cleared_trie();
                (Some(trie), deferred)
            };
            if let Some(trie) = trie_to_drop {
                executor.spawn_drop(trie);
            }
            executor.spawn_drop(deferred);
        });
    }
}

/// Per-job slot holding the parent's trie before computation and the worker's trie afterward.
///
/// The worker takes the input before computing, then stages the output before sending its result.
/// Publication requires the completed block's hash and a matching state root.
#[derive(Debug)]
struct SparseTrieSlot {
    trie: Mutex<SparseTrieState>,
}

impl SparseTrieSlot {
    const fn new(input: Option<PreservedSparseTrie>) -> Self {
        Self { trie: Mutex::new(SparseTrieState::Input(input)) }
    }

    fn publish<N: NodePrimitives>(
        &self,
        overlay_manager: &OverlayManager<N>,
        block_hash: B256,
        state_root: B256,
    ) {
        let mut staged = self.trie.lock();
        match std::mem::take(&mut *staged) {
            SparseTrieState::Output { state_root: computed, anchor_hash, trie }
                if computed == state_root =>
            {
                overlay_manager.store_sparse_trie(PreservedSparseTrie::pending(
                    trie,
                    block_hash,
                    anchor_hash,
                ));
            }
            // A builder may finish through another strategy before the task consumes its input.
            SparseTrieState::Input(input) => *staged = SparseTrieState::Input(input),
            SparseTrieState::Output { .. } => {}
        }
    }
}

#[derive(Debug)]
enum SparseTrieState {
    Input(Option<PreservedSparseTrie>),
    Output { state_root: B256, anchor_hash: B256, trie: mpsc::Receiver<SparseStateTrie> },
}

impl Default for SparseTrieState {
    fn default() -> Self {
        Self::Input(None)
    }
}

struct SparseTrieTaskOptions<N: NodePrimitives> {
    parent_header: SealedHeader<N::BlockHeader>,
    sparse_trie: Arc<SparseTrieSlot>,
    chunk_size: usize,
    /// `None` disables pruning. `Some(Vec::new())` prunes nodes older than the current block.
    pending_sparse_trie_prune_blocks: Option<Vec<ExecutedBlock<N>>>,
}

struct StateRootTaskOptions<'a, N: NodePrimitives> {
    parent_header: SealedHeader<N::BlockHeader>,
    sparse_trie: Arc<SparseTrieSlot>,
    transaction_count: Option<usize>,
    config: &'a TreeConfig,
    pending_sparse_trie_prune_blocks: Option<Vec<ExecutedBlock<N>>>,
}

fn sparse_trie_prune_target<N: NodePrimitives>(
    current_anchor: B256,
    reused_preserved_sparse_trie: bool,
    parent: alloy_eips::BlockNumHash,
    pending_blocks: Option<&[ExecutedBlock<N>]>,
) -> Option<(TrieNodeEpoch, B256)> {
    let blocks = pending_blocks?;
    let Some(oldest) = blocks.last() else {
        // Pruning the now-durable parent moves the proof source forward with it.
        return Some((TrieNodeEpoch::new(parent.number.saturating_add(1)), parent.hash))
    };
    let anchor = if !reused_preserved_sparse_trie ||
        blocks.iter().any(|block| block.recovered_block().hash() == current_anchor)
    {
        current_anchor
    } else {
        oldest.recovered_block().parent_hash()
    };
    Some((TrieNodeEpoch::new(oldest.recovered_block().number()), anchor))
}

impl<N, P, Evm> StateRootStrategy<N, P, Evm> for DefaultStateRootStrategy
where
    N: NodePrimitives,
    P: DatabaseProviderFactory + Clone + 'static,
    P::Provider: BlockNumReader
        + PruneCheckpointReader
        + StageCheckpointReader
        + ChangeSetReader
        + StorageChangeSetReader
        + StorageSettingsCache
        + 'static,
    OverlayStateProviderFactory<P, N>: DatabaseProviderROFactory<
            Provider: TrieCursorFactory
                          + HashedCursorFactory
                          + HashedPostStateProvider
                          + StateRootProvider
                          + Send,
        > + Clone
        + 'static,
    Evm: ConfigureEvm<Primitives = N> + 'static,
{
    fn prepare(
        &self,
        mut ctx: StateRootJobContext<'_, N, P, Evm>,
    ) -> ProviderResult<PreparedStateRootJob<N>> {
        if ctx.config.skip_state_root() {
            return Ok(PreparedStateRootJob::new(Box::new(SkippedStateRootJob {}), None))
        }

        if !ctx.config.use_state_root_task() {
            return Ok(PreparedStateRootJob::new(
                Box::new(SynchronousStateRootJob {
                    state_provider_factory: ctx.state_provider_factory,
                }),
                None,
            ))
        }

        let pending_sparse_trie_prune_blocks = ctx.take_sparse_trie_prune_blocks();
        let StateRootJobContext {
            executor,
            overlay_manager,
            env,
            parent_header,
            state_provider_factory,
            config,
            parallel_bal_execution,
            state: _,
        } = ctx;

        let preserved_sparse_trie = overlay_manager.take_sparse_trie();
        let proof_state_provider_factory = if let Some(anchor_hash) = preserved_sparse_trie
            .as_ref()
            .filter(|trie| trie.block_hash() == parent_header.hash())
            .map(|trie| trie.anchor_hash())
        {
            state_provider_factory.clone().with_skip_overlay_for_reused_sparse_trie(anchor_hash)
        } else {
            state_provider_factory.clone()
        };

        let sparse_trie = Arc::new(SparseTrieSlot::new(preserved_sparse_trie));
        let mut handle = self.spawn_state_root(
            executor,
            proof_state_provider_factory,
            StateRootTaskOptions {
                parent_header: parent_header.clone(),
                sparse_trie: sparse_trie.clone(),
                transaction_count: Some(env.transaction_count),
                config,
                pending_sparse_trie_prune_blocks,
            },
        );

        // The execution mode decides who finishes the update stream: the execution hook on
        // the serial path, the BAL streamer on the parallel path. Both come from one slot in
        // the handle, so only one of them can exist.
        let (hashed_update_stream, execution_hook): (
            Option<StateRootUpdateStream>,
            Option<StateRootUpdateHook>,
        ) = match parallel_bal_execution {
            true => (Some(handle.take_hashed_update_stream()), None),
            false => (None, Some(handle.take_execution_hook())),
        };
        let hint_stream = handle.take_hint_stream();

        let hashed_state_rx = Some(handle.take_hashed_state_rx());

        let mut prepared = PreparedStateRootJob::new(
            Box::new(SparseTrieStateRootJob {
                sparse_trie,
                overlay_manager: overlay_manager.clone(),
                handle,
                state_provider_factory,
                executor: executor.clone(),
                timeout: config.state_root_task_timeout(),
                compare_trie_updates: config.always_compare_trie_updates(),
                metrics: BlockValidationMetrics::default(),
            }),
            hashed_state_rx,
        )
        .with_hint_stream(hint_stream);
        if let Some(hook) = execution_hook {
            prepared = prepared.with_execution_hook(hook);
        }
        if let Some(stream) = hashed_update_stream {
            prepared = prepared.with_hashed_update_stream(stream);
        }
        Ok(prepared)
    }

    fn prepare_payload_builder(
        &self,
        mut ctx: PayloadStateRootJobContext<'_, N, P>,
    ) -> ProviderResult<Option<PayloadStateRootHandle>> {
        // Sharing the engine state-root task with the payload builder is opt-in, and needs a
        // host that can run the task pipeline at all.
        if !ctx.config.share_sparse_trie_with_payload_builder() ||
            ctx.config.skip_state_root() ||
            !ctx.config.has_enough_parallelism()
        {
            return Ok(None)
        }

        let pending_sparse_trie_prune_blocks = ctx.take_sparse_trie_prune_blocks();
        let parent_header = SealedHeader::new(ctx.parent_header().clone(), ctx.parent_hash());
        let preserved_sparse_trie = ctx.overlay_manager.take_sparse_trie();
        let proof_state_provider_factory = if let Some(anchor_hash) = preserved_sparse_trie
            .as_ref()
            .filter(|trie| trie.block_hash() == ctx.parent_hash())
            .map(|trie| trie.anchor_hash())
        {
            ctx.state_provider_factory.clone().with_skip_overlay_for_reused_sparse_trie(anchor_hash)
        } else {
            ctx.state_provider_factory.clone()
        };
        let sparse_trie = Arc::new(SparseTrieSlot::new(preserved_sparse_trie));
        let overlay_manager = ctx.overlay_manager.clone();
        Ok(Some(
            self.spawn_state_root(
                ctx.executor,
                proof_state_provider_factory,
                StateRootTaskOptions {
                    parent_header,
                    sparse_trie: sparse_trie.clone(),
                    // Tx count unknown at FCU time (block built incrementally): full proof workers.
                    transaction_count: None,
                    config: ctx.config,
                    pending_sparse_trie_prune_blocks,
                },
            )
            .into_payload_state_root_handle()
            .with_on_payload_built(move |block_hash, state_root| {
                sparse_trie.publish(&overlay_manager, block_hash, state_root);
            }),
        ))
    }
}

#[derive(Debug)]
struct SkippedStateRootJob {}

impl<N: NodePrimitives> StateRootJob<N> for SkippedStateRootJob {
    fn name(&self) -> &'static str {
        "skipped"
    }

    fn finish(
        &mut self,
        block: &RecoveredBlock<N::Block>,
        _output: Arc<BlockExecutionOutput<N::Receipt>>,
        _hashed_state: &LazyHashedPostState,
    ) -> ProviderResult<StateRootJobOutcome> {
        Ok(StateRootJobOutcome::new(block.header().state_root(), Arc::new(TrieUpdates::default())))
    }
}

#[derive(Debug)]
struct SynchronousStateRootJob<N: NodePrimitives, P> {
    state_provider_factory: OverlayStateProviderFactory<P, N>,
}

impl<N, P> StateRootJob<N> for SynchronousStateRootJob<N, P>
where
    N: NodePrimitives,
    P: DatabaseProviderFactory + Clone + 'static,
    P::Provider: BlockNumReader
        + PruneCheckpointReader
        + StageCheckpointReader
        + ChangeSetReader
        + StorageChangeSetReader
        + StorageSettingsCache
        + 'static,
    OverlayStateProviderFactory<P, N>: DatabaseProviderROFactory<Provider: StateRootProvider>,
{
    fn name(&self) -> &'static str {
        "synchronous"
    }

    fn finish(
        &mut self,
        _block: &RecoveredBlock<N::Block>,
        _output: Arc<BlockExecutionOutput<N::Receipt>>,
        hashed_state: &LazyHashedPostState,
    ) -> ProviderResult<StateRootJobOutcome> {
        let provider = self.state_provider_factory.database_provider_ro()?;
        let (state_root, trie_updates) =
            provider.state_root_with_updates(hashed_state.get().as_ref().clone())?;
        Ok(StateRootJobOutcome::new(state_root, Arc::new(trie_updates)))
    }
}

#[derive(Debug)]
struct SparseTrieStateRootJob<N: NodePrimitives, P> {
    sparse_trie: Arc<SparseTrieSlot>,
    overlay_manager: OverlayManager<N>,
    handle: StateRootHandle,
    state_provider_factory: OverlayStateProviderFactory<P, N>,
    executor: reth_tasks::Runtime,
    timeout: Option<Duration>,
    compare_trie_updates: bool,
    metrics: BlockValidationMetrics,
}

impl<N, P> SparseTrieStateRootJob<N, P>
where
    N: NodePrimitives,
    P: DatabaseProviderFactory + Clone + 'static,
    P::Provider: BlockNumReader
        + PruneCheckpointReader
        + StageCheckpointReader
        + ChangeSetReader
        + StorageChangeSetReader
        + StorageSettingsCache
        + 'static,
    OverlayStateProviderFactory<P, N>: DatabaseProviderROFactory<
            Provider: TrieCursorFactory
                          + HashedCursorFactory
                          + HashedPostStateProvider
                          + StateRootProvider
                          + Send,
        > + Clone
        + 'static,
{
    fn serial_fallback(
        &self,
        output: Arc<BlockExecutionOutput<N::Receipt>>,
    ) -> ProviderResult<SerialFallbackRx> {
        let provider = self.state_provider_factory.database_provider_ro()?;
        let (fallback_tx, fallback_rx) = mpsc::channel();
        self.executor.spawn_blocking_named("serial-root", move || {
            let result = (|| {
                let hashed_state = Arc::new(provider.hashed_post_state(&output.state)?);
                let (root, updates) =
                    provider.state_root_with_updates(hashed_state.as_ref().clone())?;
                Ok((root, updates, hashed_state))
            })();
            let _ = fallback_tx.send(result);
        });

        Ok(fallback_rx)
    }

    /// Recomputes the state root serially from the execution output.
    ///
    /// Used when the state-root task failed or produced a wrong root, so the recomputed hashed
    /// post state is returned in the outcome for validation to re-check against.
    fn compute_serial(
        &self,
        output: &BlockExecutionOutput<N::Receipt>,
    ) -> ProviderResult<StateRootJobOutcome> {
        let provider = self.state_provider_factory.database_provider_ro()?;
        let hashed_state = Arc::new(provider.hashed_post_state(&output.state)?);
        let (state_root, trie_updates) =
            provider.state_root_with_updates(hashed_state.as_ref().clone())?;
        self.metrics.state_root_task_fallback_success_total.increment(1);
        Ok(StateRootJobOutcome::new(state_root, Arc::new(trie_updates))
            .with_hashed_state(Some(hashed_state)))
    }

    /// Converts a task outcome into a job outcome, recomputing serially when the task returned
    /// a root that does not match the block header. A state-root-task bug then costs latency
    /// instead of marking a valid block invalid; if the serial root also mismatches, validation
    /// rejects the block.
    fn verified_sparse_outcome(
        &self,
        block: &RecoveredBlock<N::Block>,
        output: &BlockExecutionOutput<N::Receipt>,
        outcome: StateRootComputeOutcome,
    ) -> ProviderResult<StateRootJobOutcome> {
        let outcome = self.sparse_outcome(output, outcome);
        if outcome.state_root == block.header().state_root() {
            self.sparse_trie.publish(&self.overlay_manager, block.hash(), outcome.state_root);
            return Ok(outcome)
        }
        warn!(
            target: "engine::tree::state_root_strategy",
            state_root = ?outcome.state_root,
            block_state_root = ?block.header().state_root(),
            "State root task returned incorrect state root, recomputing serially"
        );
        self.compute_serial(output)
    }

    fn sparse_outcome(
        &self,
        output: &BlockExecutionOutput<N::Receipt>,
        outcome: StateRootComputeOutcome,
    ) -> StateRootJobOutcome {
        let StateRootComputeOutcome { state_root, trie_updates, hashed_state: _hashed_state } =
            outcome;

        if self.compare_trie_updates {
            compare_trie_updates_with_serial(
                self.state_provider_factory.clone(),
                output,
                trie_updates.as_ref().clone(),
            );
        }

        StateRootJobOutcome::new(state_root, trie_updates)
    }
}

impl<N, P> StateRootJob<N> for SparseTrieStateRootJob<N, P>
where
    N: NodePrimitives,
    P: DatabaseProviderFactory + Clone + 'static,
    P::Provider: BlockNumReader
        + PruneCheckpointReader
        + StageCheckpointReader
        + ChangeSetReader
        + StorageChangeSetReader
        + StorageSettingsCache
        + 'static,
    OverlayStateProviderFactory<P, N>: DatabaseProviderROFactory<
            Provider: TrieCursorFactory
                          + HashedCursorFactory
                          + HashedPostStateProvider
                          + StateRootProvider
                          + Send,
        > + Clone
        + 'static,
{
    fn name(&self) -> &'static str {
        "sparse-trie"
    }

    fn finish(
        &mut self,
        block: &RecoveredBlock<N::Block>,
        output: Arc<BlockExecutionOutput<N::Receipt>>,
        _hashed_state: &LazyHashedPostState,
    ) -> ProviderResult<StateRootJobOutcome> {
        if self.timeout.is_none() {
            return match self.handle.state_root() {
                Ok(outcome) => self.verified_sparse_outcome(block, &output, outcome),
                Err(err) => {
                    debug!(target: "engine::tree::state_root_strategy", %err, "State root task failed, falling back to serial root");
                    self.compute_serial(&output)
                }
            }
        }

        let timeout = self.timeout.expect("checked above");
        let task_rx = self.handle.take_state_root_rx();
        let fallback_rx = match task_rx.recv_timeout(timeout) {
            Ok(Ok(outcome)) => return self.verified_sparse_outcome(block, &output, outcome),
            Ok(Err(err)) => {
                debug!(target: "engine::tree::state_root_strategy", %err, "State root task failed, falling back to serial root");
                self.serial_fallback(output.clone())?
            }
            Err(RecvTimeoutError::Timeout) => {
                warn!(target: "engine::tree::state_root_strategy", ?timeout, "State root task timed out, racing serial fallback");
                self.metrics.state_root_task_timeout_total.increment(1);
                self.serial_fallback(output.clone())?
            }
            Err(RecvTimeoutError::Disconnected) => {
                debug!(target: "engine::tree::state_root_strategy", "State root task dropped, falling back to serial root");
                self.serial_fallback(output.clone())?
            }
        };

        loop {
            if let Ok(Ok(outcome)) = task_rx.try_recv() {
                let outcome = self.sparse_outcome(&output, outcome);
                if outcome.state_root == block.header().state_root() {
                    self.sparse_trie.publish(
                        &self.overlay_manager,
                        block.hash(),
                        outcome.state_root,
                    );
                    return Ok(outcome)
                }
                // A wrong task root falls through to the serial fallback already racing below.
                warn!(
                    target: "engine::tree::state_root_strategy",
                    state_root = ?outcome.state_root,
                    block_state_root = ?block.header().state_root(),
                    "State root task returned incorrect state root, using serial fallback"
                );
            }

            match fallback_rx.try_recv() {
                Ok(Ok((state_root, trie_updates, hashed_state))) => {
                    self.metrics.state_root_task_fallback_success_total.increment(1);
                    return Ok(StateRootJobOutcome::new(state_root, Arc::new(trie_updates))
                        .with_hashed_state(Some(hashed_state)))
                }
                Ok(Err(err)) => return Err(err),
                Err(mpsc::TryRecvError::Empty) => {}
                Err(mpsc::TryRecvError::Disconnected) => {
                    return Err(ProviderError::other(std::io::Error::other(
                        "serial state root fallback task dropped",
                    )))
                }
            }

            std::thread::sleep(Duration::from_millis(1));
        }
    }
}

fn compare_trie_updates_with_serial<N, P>(
    state_provider_factory: OverlayStateProviderFactory<P, N>,
    output: &BlockExecutionOutput<N::Receipt>,
    task_trie_updates: TrieUpdates,
) -> bool
where
    N: NodePrimitives,
    P: DatabaseProviderFactory,
    P::Provider: BlockNumReader
        + PruneCheckpointReader
        + StageCheckpointReader
        + ChangeSetReader
        + StorageChangeSetReader
        + StorageSettingsCache
        + 'static,
    OverlayStateProviderFactory<P, N>: DatabaseProviderROFactory<
        Provider: TrieCursorFactory
                      + HashedCursorFactory
                      + HashedPostStateProvider
                      + StateRootProvider,
    >,
{
    debug!(target: "engine::tree::state_root_strategy", "Comparing trie updates with serial computation");

    match state_provider_factory.database_provider_ro().and_then(|provider| {
        let hashed_state = provider.hashed_post_state(&output.state)?;
        provider.state_root_with_updates(hashed_state)
    }) {
        Ok((serial_root, serial_trie_updates)) => {
            debug!(
                target: "engine::tree::state_root_strategy",
                ?serial_root,
                "Serial state root computation finished for comparison"
            );

            match state_provider_factory.database_provider_ro() {
                Ok(provider) => match super::trie_updates::compare_trie_updates(
                    &provider,
                    task_trie_updates,
                    serial_trie_updates,
                ) {
                    Ok(has_diff) => return has_diff,
                    Err(err) => {
                        warn!(
                            target: "engine::tree::state_root_strategy",
                            %err,
                            "Error comparing trie updates"
                        );
                        return true;
                    }
                },
                Err(err) => {
                    warn!(
                        target: "engine::tree::state_root_strategy",
                        %err,
                        "Failed to get database provider for trie update comparison"
                    );
                }
            }
        }
        Err(err) => {
            warn!(
                target: "engine::tree::state_root_strategy",
                %err,
                "Failed to compute serial state root for comparison"
            );
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::constants::KECCAK_EMPTY;
    use alloy_primitives::{map::HashMap, Address, U256};
    use rand::Rng;
    use reth_chain_state::test_utils::TestBlockBuilder;
    use reth_chainspec::ChainSpec;
    use reth_db_common::init::init_genesis;
    use reth_ethereum_primitives::EthPrimitives;
    use reth_evm::OnStateHook;
    use reth_evm_ethereum::EthEvmConfig;
    use reth_primitives_traits::{Account, StorageEntry};
    use reth_provider::{
        providers::BlockchainProvider, test_utils::create_test_provider_factory_with_chain_spec,
        HashingWriter,
    };
    use reth_storage_overlay::{OverlayManager, OverlayStateProviderFactory};
    use reth_testing_utils::generators;
    use reth_trie::test_utils::state_root;
    use revm::state::{AccountInfo, AccountStatus, EvmState, EvmStorageSlot, TransactionId};

    #[test]
    fn finish_publishes_only_accepted_sparse_trie() {
        let factory = create_test_provider_factory_with_chain_spec(Arc::new(ChainSpec::default()));
        let genesis_hash = init_genesis(&factory).unwrap();
        let provider = BlockchainProvider::new(factory).unwrap();
        let runtime = reth_tasks::Runtime::test();

        for timeout in [None, Some(Duration::from_secs(1))] {
            for (finish, matching_root) in [(false, true), (true, true), (true, false)] {
                let overlay_manager = OverlayManager::<EthPrimitives>::default();
                let block = RecoveredBlock::new_unhashed(
                    reth_ethereum_primitives::Block {
                        header: alloy_consensus::Header {
                            state_root: B256::with_last_byte(1),
                            ..Default::default()
                        },
                        ..Default::default()
                    },
                    Vec::new(),
                );
                let (completer, trie) = mpsc::channel();
                let sparse_trie = Arc::new(SparseTrieSlot {
                    trie: Mutex::new(SparseTrieState::Output {
                        state_root: if matching_root { block.state_root } else { B256::ZERO },
                        anchor_hash: genesis_hash,
                        trie,
                    }),
                });
                let (result_tx, result_rx) = mpsc::channel();
                result_tx
                    .send(Ok(StateRootComputeOutcome {
                        state_root: if matching_root { block.state_root } else { B256::ZERO },
                        trie_updates: Default::default(),
                        hashed_state: Default::default(),
                    }))
                    .unwrap();
                completer.send(SparseStateTrie::default()).unwrap();
                let mut job = SparseTrieStateRootJob {
                    sparse_trie,
                    overlay_manager: overlay_manager.clone(),
                    handle: StateRootHandle::new(
                        B256::ZERO,
                        crossbeam_channel::unbounded().0,
                        StateRootTaskCancelGuard::channel().0,
                        result_rx,
                        mpsc::channel().1,
                    ),
                    state_provider_factory: OverlayStateProviderFactory::new(
                        provider.clone(),
                        overlay_manager.overlay_builder(genesis_hash),
                    ),
                    executor: runtime.clone(),
                    timeout,
                    compare_trie_updates: false,
                    metrics: Default::default(),
                };
                assert!(overlay_manager.take_sparse_trie().is_none());
                if finish {
                    job.finish(
                        &block,
                        Arc::new(BlockExecutionOutput::default()),
                        &LazyHashedPostState::ready(Default::default()),
                    )
                    .unwrap();
                }
                drop(job);
                let preserved = overlay_manager.take_sparse_trie();
                assert_eq!(preserved.is_some(), finish && matching_root);
                if let Some(preserved) = preserved {
                    assert!(preserved.into_trie_for(block.hash()).unwrap().is_some());
                }
            }
        }
    }

    #[test]
    fn abandoned_sparse_trie_does_not_replace_shared_cache() {
        for completed in [false, true] {
            let overlay_manager = OverlayManager::<EthPrimitives>::default();
            let block_hash = B256::with_last_byte(1);
            let (completer, trie) = mpsc::channel();
            let sparse_trie = SparseTrieSlot {
                trie: Mutex::new(SparseTrieState::Output {
                    state_root: B256::ZERO,
                    anchor_hash: B256::ZERO,
                    trie,
                }),
            };
            let next_hash = B256::with_last_byte(2);
            overlay_manager.store_sparse_trie(PreservedSparseTrie::anchored(
                SparseStateTrie::default(),
                next_hash,
                block_hash,
            ));

            if completed {
                completer.send(SparseStateTrie::default()).unwrap();
                drop(sparse_trie);
            } else {
                drop(sparse_trie);
                assert!(completer.send(SparseStateTrie::default()).is_err());
            }
            assert_eq!(overlay_manager.take_sparse_trie().unwrap().block_hash(), next_hash);
        }
    }

    #[test]
    fn failed_parent_trie_leaves_slot_empty() {
        let factory = create_test_provider_factory_with_chain_spec(Arc::new(ChainSpec::default()));
        let genesis_hash = init_genesis(&factory).unwrap();
        let runtime = reth_tasks::Runtime::test();
        {
            let (completer, trie) = mpsc::channel();
            let preserved = PreservedSparseTrie::pending(trie, genesis_hash, B256::ZERO);
            drop(completer);
            let sparse_trie = Arc::new(SparseTrieSlot::new(Some(preserved)));
            let mut handle = DefaultStateRootStrategy::default().spawn_state_root(
                &runtime,
                OverlayStateProviderFactory::new(
                    factory,
                    OverlayManager::<EthPrimitives>::default().overlay_builder(genesis_hash),
                ),
                StateRootTaskOptions::<EthPrimitives> {
                    parent_header: SealedHeader::new(Default::default(), genesis_hash),
                    sparse_trie: sparse_trie.clone(),
                    transaction_count: None,
                    config: &TreeConfig::default(),
                    pending_sparse_trie_prune_blocks: None,
                },
            );
            assert!(handle.state_root().is_err());
            assert!(matches!(*sparse_trie.trie.lock(), SparseTrieState::Input(None)));
        }
        for name in ["sparse-trie", "storage-workers", "account-workers"] {
            runtime.spawn_blocking_named(name, || {}).get();
        }
    }

    #[test]
    fn sparse_trie_prune_target_uses_requested_range() {
        let parent = alloy_eips::BlockNumHash { hash: B256::with_last_byte(9), number: 9 };
        assert_eq!(
            sparse_trie_prune_target::<EthPrimitives>(B256::ZERO, false, parent, None),
            None
        );
        assert_eq!(
            sparse_trie_prune_target::<EthPrimitives>(B256::ZERO, false, parent, Some(&[])),
            Some((TrieNodeEpoch::new(10), parent.hash))
        );

        let mut blocks: Vec<_> = TestBlockBuilder::eth().get_executed_blocks(7..10).collect();
        blocks.reverse();

        let anchor = blocks[0].recovered_block().hash();
        assert_eq!(
            sparse_trie_prune_target(anchor, true, parent, Some(&blocks)),
            Some((TrieNodeEpoch::new(7), anchor))
        );
    }

    #[test]
    fn sparse_trie_prune_target_advances_to_prune_anchor() {
        let blocks: Vec<_> = TestBlockBuilder::eth().get_executed_blocks(1..5).collect();
        let reused_anchor_hash = blocks[0].recovered_block().hash();
        let expected_prune_anchor = blocks[1].recovered_block().hash();
        let parent = blocks[3].recovered_block().num_hash();
        let prune_blocks: Vec<_> = blocks.into_iter().skip(2).rev().collect();

        assert_eq!(
            sparse_trie_prune_target(reused_anchor_hash, true, parent, Some(&prune_blocks),),
            Some((TrieNodeEpoch::new(3), expected_prune_anchor))
        );
    }

    #[test]
    fn sparse_trie_prune_target_does_not_move_backwards_when_anchor_is_in_prune_range() {
        let blocks: Vec<_> = TestBlockBuilder::eth().get_executed_blocks(1..5).collect();
        let reused_anchor_hash = blocks[2].recovered_block().hash();
        let mut prune_blocks = blocks;
        prune_blocks.reverse();
        let prune_anchor = prune_blocks.last().unwrap().recovered_block().parent_hash();

        assert_ne!(reused_anchor_hash, prune_anchor);
        assert_eq!(
            sparse_trie_prune_target(
                reused_anchor_hash,
                true,
                prune_blocks[0].recovered_block().num_hash(),
                Some(&prune_blocks),
            ),
            Some((TrieNodeEpoch::new(1), reused_anchor_hash))
        );
    }

    #[test]
    fn sparse_trie_prune_target_keeps_parent_for_fresh_trie() {
        let mut blocks: Vec<_> = TestBlockBuilder::eth().get_executed_blocks(1..3).collect();
        blocks.reverse();
        let parent_hash = B256::with_last_byte(0xaa);

        assert_eq!(
            sparse_trie_prune_target(
                parent_hash,
                false,
                blocks[0].recovered_block().num_hash(),
                Some(&blocks),
            ),
            Some((TrieNodeEpoch::new(1), parent_hash))
        );
    }

    #[test]
    fn sparse_trie_prune_target_advances_reused_anchor_when_parent_becomes_durable() {
        let blocks: Vec<_> = TestBlockBuilder::eth().get_executed_blocks(1..4).collect();
        let old_anchor = blocks[1].recovered_block().hash();
        let parent = blocks[2].recovered_block().num_hash();

        assert_eq!(
            sparse_trie_prune_target::<EthPrimitives>(old_anchor, true, parent, Some(&[])),
            Some((TrieNodeEpoch::new(4), parent.hash))
        );
    }

    fn create_mock_state_updates(num_accounts: usize, updates_per_account: usize) -> Vec<EvmState> {
        let mut rng = generators::rng();
        let all_addresses: Vec<Address> = (0..num_accounts).map(|_| rng.random()).collect();
        let mut updates = Vec::with_capacity(updates_per_account);

        for _ in 0..updates_per_account {
            let num_accounts_in_update = rng.random_range(1..=num_accounts);
            let mut state_update = EvmState::default();

            for &address in &all_addresses[0..num_accounts_in_update] {
                let mut storage = HashMap::default();
                if rng.random_bool(0.7) {
                    for _ in 0..rng.random_range(1..10) {
                        let slot = U256::from(rng.random::<u64>());
                        storage.insert(
                            slot,
                            EvmStorageSlot::new_changed(
                                U256::ZERO,
                                U256::from(rng.random::<u64>()),
                                TransactionId::ZERO,
                            ),
                        );
                    }
                }

                let mut account = revm::state::Account::default();
                account.info = AccountInfo {
                    balance: U256::from(rng.random::<u64>()),
                    nonce: rng.random::<u64>(),
                    code_hash: KECCAK_EMPTY,
                    code: Some(Default::default()),
                    account_id: None,
                };
                account.storage = storage;
                account.status = AccountStatus::Touched;
                account.transaction_id = TransactionId::ZERO;
                state_update.insert(address, account);
            }

            updates.push(state_update);
        }

        updates
    }

    #[test]
    fn payload_builder_trie_matches_serial_root_and_can_be_reused() {
        reth_tracing::init_test_tracing();

        let factory = create_test_provider_factory_with_chain_spec(Arc::new(ChainSpec::default()));
        let genesis_hash = init_genesis(&factory).unwrap();
        let state_updates = create_mock_state_updates(10, 10);
        let mut accumulated_state: HashMap<Address, (Account, HashMap<B256, U256>)> =
            HashMap::default();

        {
            let provider_rw = factory.provider_rw().expect("failed to get provider");
            for update in &state_updates {
                let account_updates = update.iter().map(|(address, account)| {
                    (*address, Some(Account::from_revm_account(account)))
                });
                provider_rw
                    .insert_account_for_hashing(account_updates)
                    .expect("failed to insert accounts");

                let storage_updates = update.iter().map(|(address, account)| {
                    let storage_entries = account.storage.iter().map(|(slot, value)| {
                        StorageEntry { key: B256::from(*slot), value: value.present_value }
                    });
                    (*address, storage_entries)
                });
                provider_rw
                    .insert_storage_for_hashing(storage_updates)
                    .expect("failed to insert storage");
            }
            provider_rw.commit().expect("failed to commit changes");
        }

        for update in &state_updates {
            for (address, account) in update {
                let storage: HashMap<B256, U256> = account
                    .storage
                    .iter()
                    .map(|(key, value)| (B256::from(*key), value.present_value))
                    .collect();
                let entry = accumulated_state.entry(*address).or_default();
                entry.0 = Account::from_revm_account(account);
                entry.1.extend(storage);
            }
        }

        let provider_factory = BlockchainProvider::new(factory).unwrap();
        let runtime = reth_tasks::Runtime::test();
        let overlay_manager = OverlayManager::<EthPrimitives>::default();
        let config = TreeConfig::default()
            .with_has_enough_parallelism(true)
            .with_share_sparse_trie_with_payload_builder(true);
        let mut state = EngineApiTreeState::new(
            10,
            10,
            config.invalid_header_hit_eviction_threshold(),
            alloy_eips::BlockNumHash::new(0, genesis_hash),
            crate::engine::EngineApiKind::Ethereum,
            overlay_manager.clone(),
        );
        let mut root_from_regular = state_root(accumulated_state.clone());
        let mut preserved: Option<PreservedSparseTrie> = None;
        for (number, (reuse, return_payload, matching_root)) in [
            (false, false, true),
            (false, true, false),
            (false, true, true),
            (true, true, true),
            (true, false, true),
        ]
        .into_iter()
        .enumerate()
        {
            assert_eq!(preserved.is_some(), reuse);
            let parent_hash =
                preserved.as_ref().map_or(genesis_hash, PreservedSparseTrie::block_hash);
            let block_hash = B256::with_last_byte(number as u8 + 1);
            if let Some(preserved) = preserved.take() {
                overlay_manager.store_sparse_trie(preserved);
            }
            let parent_header = alloy_consensus::Header {
                number: number as u64,
                state_root: if reuse { root_from_regular } else { B256::ZERO },
                ..Default::default()
            };
            let mut state_root_handle = <DefaultStateRootStrategy as StateRootStrategy<
                EthPrimitives,
                _,
                EthEvmConfig,
            >>::prepare_payload_builder(
                &DefaultStateRootStrategy::default(),
                PayloadStateRootJobContext::new(
                    &runtime,
                    &overlay_manager,
                    parent_hash,
                    &parent_header,
                    1,
                    &mut state,
                    OverlayStateProviderFactory::new(
                        provider_factory.clone(),
                        overlay_manager.overlay_builder(genesis_hash),
                    ),
                    &config,
                ),
            )
            .unwrap()
            .unwrap();
            let on_payload_built = state_root_handle.take_on_payload_built().unwrap();
            let mut state_hook = state_root_handle.take_state_hook();
            if reuse {
                let mut update = state_updates.last().unwrap().clone();
                for (address, account) in &mut update {
                    account.info.nonce += number as u64;
                    accumulated_state.get_mut(address).unwrap().0 =
                        Account::from_revm_account(account);
                }
                state_hook.on_state(update);
                root_from_regular = state_root(accumulated_state.clone());
            } else {
                for update in &state_updates {
                    state_hook.on_state(update.clone());
                }
            }
            drop(state_hook);

            let root_from_task = state_root_handle.state_root().expect("task failed").state_root;
            assert_eq!(root_from_task, root_from_regular);
            assert!(overlay_manager.take_sparse_trie().is_none());
            drop(state_root_handle);
            if return_payload {
                on_payload_built(
                    block_hash,
                    if matching_root { root_from_task } else { B256::ZERO },
                );
            } else {
                drop(on_payload_built);
            }
            preserved = overlay_manager.take_sparse_trie();
            assert_eq!(preserved.is_some(), return_payload && matching_root);
            if let Some(preserved) = &preserved {
                assert_eq!(preserved.block_hash(), block_hash);
                assert_eq!(preserved.anchor_hash(), genesis_hash);
            }
        }
        for name in ["sparse-trie", "trie-hashing", "storage-workers", "account-workers", "drop"] {
            runtime.spawn_blocking_named(name, || {}).get();
        }
    }
}
