//! State-root strategies for engine-tree block validation.
//!
//! A [`StateRootStrategy`] is installed once per node, via
//! `BasicEngineValidator::with_state_root_strategy`, and consulted for every block that engine
//! validation executes. For each block the strategy prepares a [`StateRootJob`] before execution
//! starts, and validation finishes the job after execution to obtain the state root that is
//! checked against the block header.
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
//! A prepared job exposes [`StateRootStreams`] views over its sink. Exactly one authoritative
//! source fires per block, and the job does not control which one:
//!
//! - On the parallel BAL execution path, prewarm converts the block access list and delivers
//!   pre-hashed updates through the hashed-update stream, terminated by `on_updates_finished`.
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
//! Returning empty trie updates in the outcome means the trie tables are no longer maintained:
//! `eth_getProof` and anything else that reads the stored trie will not work for new blocks.
//! Returning no changed paths opts the block out of sparse-trie cache pruning.

use crate::tree::{
    multiproof::{StateRootComputeOutcome, StateRootHandle, StateRootStreams},
    payload_processor::PayloadProcessor,
    payload_validator::LazyHashedPostState,
    ExecutionEnv, StateProviderBuilder, TreeConfig,
};
use alloy_primitives::B256;
use reth_errors::ProviderResult;
use reth_evm::{ConfigureEvm, OnStateHook};
use reth_primitives_traits::{AlloyBlockHeader, NodePrimitives, RecoveredBlock};
use reth_provider::{
    providers::OverlayStateProviderFactory, BlockExecutionOutput, BlockReader,
    DatabaseProviderFactory, DatabaseProviderROFactory, HashedPostStateProvider, ProviderError,
    StateProviderFactory, StateReader, StateRootProvider,
};
use reth_trie::{
    hashed_cursor::HashedCursorFactory, prefix_set::TriePrefixSetsMut,
    trie_cursor::TrieCursorFactory, updates::TrieUpdates, HashedPostState,
};
#[cfg(feature = "trie-debug")]
use reth_trie_sparse::debug_recorder::TrieDebugRecorder;
pub use reth_trie_sparse::SparseTrieRetainedPaths;
use std::{
    fmt,
    sync::{
        mpsc::{self, RecvTimeoutError},
        Arc,
    },
    time::Duration,
};
use tracing::{debug, warn};

/// Strategy used by engine-tree validation to prepare per-block state-root work.
pub trait StateRootStrategy<N, P, Evm>: Send + Sync
where
    N: NodePrimitives,
    Evm: ConfigureEvm<Primitives = N>,
{
    /// Prepares a per-block state-root job before execution starts.
    fn prepare(
        &self,
        ctx: StateRootJobContext<'_, N, P, Evm>,
    ) -> ProviderResult<PreparedStateRootJob<N>>;
}

/// Data available while preparing one state-root job.
pub struct StateRootJobContext<'a, N, P, Evm>
where
    N: NodePrimitives,
    Evm: ConfigureEvm<Primitives = N>,
{
    payload_processor: &'a PayloadProcessor<Evm>,
    env: &'a ExecutionEnv<Evm>,
    provider_builder: StateProviderBuilder<N, P>,
    overlay_factory: OverlayStateProviderFactory<P, N>,
    config: &'a TreeConfig,
    parallel_bal_execution: bool,
    pending_sparse_trie_prune: Option<SparseTrieRetainedPaths>,
}

impl<N, P, Evm> fmt::Debug for StateRootJobContext<'_, N, P, Evm>
where
    N: NodePrimitives,
    Evm: ConfigureEvm<Primitives = N>,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StateRootJobContext")
            .field("parallel_bal_execution", &self.parallel_bal_execution)
            .field("has_pending_sparse_trie_prune", &self.pending_sparse_trie_prune.is_some())
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
        payload_processor: &'a PayloadProcessor<Evm>,
        env: &'a ExecutionEnv<Evm>,
        provider_builder: StateProviderBuilder<N, P>,
        overlay_factory: OverlayStateProviderFactory<P, N>,
        config: &'a TreeConfig,
        parallel_bal_execution: bool,
        pending_sparse_trie_prune: Option<SparseTrieRetainedPaths>,
    ) -> Self {
        Self {
            payload_processor,
            env,
            provider_builder,
            overlay_factory,
            config,
            parallel_bal_execution,
            pending_sparse_trie_prune,
        }
    }

    /// Returns the execution environment for the block.
    pub const fn env(&self) -> &ExecutionEnv<Evm> {
        self.env
    }

    /// Returns the task runtime used by payload processing.
    pub const fn executor(&self) -> &reth_tasks::Runtime {
        self.payload_processor.executor()
    }

    /// Returns true when validation will use the parallel BAL execution path.
    pub const fn parallel_bal_execution(&self) -> bool {
        self.parallel_bal_execution
    }

    /// Returns a clone of the state provider builder.
    pub fn provider_builder(&self) -> StateProviderBuilder<N, P>
    where
        P: Clone,
    {
        self.provider_builder.clone()
    }
}

/// Prepared per-block state-root work and its stream wiring.
pub struct PreparedStateRootJob<N: NodePrimitives> {
    job: Box<dyn StateRootJob<N>>,
    streams: StateRootStreams,
    hashed_state_rx: Option<mpsc::Receiver<HashedPostState>>,
}

impl<N: NodePrimitives> fmt::Debug for PreparedStateRootJob<N> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PreparedStateRootJob")
            .field("name", &self.job.name())
            .field("streams", &self.streams)
            .field("has_hashed_state_rx", &self.hashed_state_rx.is_some())
            .finish()
    }
}

impl<N: NodePrimitives> PreparedStateRootJob<N> {
    /// Creates a prepared state-root job.
    pub const fn new(
        job: Box<dyn StateRootJob<N>>,
        streams: StateRootStreams,
        hashed_state_rx: Option<mpsc::Receiver<HashedPostState>>,
    ) -> Self {
        Self { job, streams, hashed_state_rx }
    }

    /// Returns the job name used in logs.
    pub fn name(&self) -> &'static str {
        self.job.name()
    }

    /// Returns stream views used by prewarm.
    pub fn streams(&self) -> StateRootStreams {
        self.streams.clone()
    }

    /// Takes the execution hook, if the job wants normal execution updates.
    pub fn take_execution_hook(&mut self) -> Option<Box<dyn OnStateHook + 'static>> {
        self.streams
            .take_execution_stream()
            .map(|stream| Box::new(stream.state_hook()) as Box<dyn OnStateHook + 'static>)
    }

    /// Takes the optional hashed-state receiver produced by the job.
    pub fn take_hashed_state_rx(&mut self) -> Option<mpsc::Receiver<HashedPostState>> {
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
    /// Changed trie node base paths retained while computing the root, if the job tracks them.
    pub changed_paths: Option<Arc<TriePrefixSetsMut>>,
}

impl StateRootJobOutcome {
    /// Creates a state-root job outcome without changed paths.
    pub const fn new(state_root: B256, trie_updates: Arc<TrieUpdates>) -> Self {
        Self { state_root, trie_updates, changed_paths: None }
    }

    /// Sets the changed trie node base paths retained while computing the root.
    pub fn with_changed_paths(mut self, changed_paths: Option<Arc<TriePrefixSetsMut>>) -> Self {
        self.changed_paths = changed_paths;
        self
    }
}

/// Default state-root strategy used by engine-tree validation.
///
/// Covers the built-in modes: the sparse-trie state-root task, plus the skipped and
/// synchronous modes selected by [`TreeConfig`].
///
/// Custom strategies can hold this type and delegate to it for blocks where they want the
/// default behavior.
#[derive(Debug, Default)]
pub struct DefaultStateRootStrategy;

impl<N, P, Evm> StateRootStrategy<N, P, Evm> for DefaultStateRootStrategy
where
    N: NodePrimitives,
    P: DatabaseProviderFactory
        + BlockReader<Header = N::BlockHeader>
        + StateProviderFactory
        + StateReader
        + Clone
        + 'static,
    OverlayStateProviderFactory<P, N>: DatabaseProviderROFactory<Provider: TrieCursorFactory + HashedCursorFactory>
        + Clone
        + Send
        + Sync
        + 'static,
    Evm: ConfigureEvm<Primitives = N> + 'static,
{
    fn prepare(
        &self,
        ctx: StateRootJobContext<'_, N, P, Evm>,
    ) -> ProviderResult<PreparedStateRootJob<N>> {
        let StateRootJobContext {
            payload_processor,
            env,
            provider_builder,
            overlay_factory,
            config,
            parallel_bal_execution,
            pending_sparse_trie_prune,
        } = ctx;

        if config.skip_state_root() {
            return Ok(PreparedStateRootJob::new(
                Box::new(SkippedStateRootJob {}),
                StateRootStreams::empty(),
                None,
            ))
        }

        if config.state_root_fallback() {
            return Ok(PreparedStateRootJob::new(
                Box::new(SynchronousStateRootJob { provider_builder }),
                StateRootStreams::empty(),
                None,
            ))
        }

        let halve_workers =
            env.transaction_count <= PayloadProcessor::<Evm>::SMALL_BLOCK_PROOF_WORKER_TX_THRESHOLD;
        let mut handle = payload_processor.spawn_state_root(
            overlay_factory.clone(),
            env.parent_state_root,
            halve_workers,
            config,
            pending_sparse_trie_prune,
        );
        let streams = handle.streams(!parallel_bal_execution);
        let hashed_state_rx = Some(handle.take_hashed_state_rx());

        Ok(PreparedStateRootJob::new(
            Box::new(SparseTrieStateRootJob {
                handle,
                provider_builder,
                overlay_factory,
                executor: payload_processor.executor().clone(),
                timeout: config.state_root_task_timeout(),
                compare_trie_updates: config.always_compare_trie_updates(),
            }),
            streams,
            hashed_state_rx,
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
    provider_builder: StateProviderBuilder<N, P>,
}

impl<N, P> StateRootJob<N> for SynchronousStateRootJob<N, P>
where
    N: NodePrimitives,
    P: BlockReader + StateProviderFactory + StateReader + Clone + Send + Sync + 'static,
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
        let provider = self.provider_builder.clone().build()?;
        let (state_root, trie_updates) =
            provider.state_root_with_updates(hashed_state.get().as_ref().clone())?;
        Ok(StateRootJobOutcome::new(state_root, Arc::new(trie_updates)))
    }
}

#[derive(Debug)]
struct SparseTrieStateRootJob<N: NodePrimitives, P> {
    handle: StateRootHandle,
    provider_builder: StateProviderBuilder<N, P>,
    overlay_factory: OverlayStateProviderFactory<P, N>,
    executor: reth_tasks::Runtime,
    timeout: Option<Duration>,
    compare_trie_updates: bool,
}

impl<N, P> SparseTrieStateRootJob<N, P>
where
    N: NodePrimitives,
    P: StateProviderFactory + Clone + Send + Sync + 'static,
    P: BlockReader + StateReader,
    OverlayStateProviderFactory<P, N>: DatabaseProviderROFactory<Provider: TrieCursorFactory + HashedCursorFactory>
        + Clone
        + Send
        + Sync
        + 'static,
{
    fn serial_fallback(
        executor: &reth_tasks::Runtime,
        provider_builder: StateProviderBuilder<N, P>,
        output: Arc<BlockExecutionOutput<N::Receipt>>,
    ) -> ProviderResult<mpsc::Receiver<ProviderResult<(B256, TrieUpdates)>>> {
        let provider = provider_builder.build()?;
        let (fallback_tx, fallback_rx) = mpsc::channel();
        executor.spawn_blocking_named("serial-root", move || {
            let hashed_state = provider.hashed_post_state(&output.state);
            let state_root_result = provider
                .state_root_with_updates(hashed_state.clone())
                .map(|(root, updates)| (root, updates));
            let _ = fallback_tx.send(state_root_result);
        });

        Ok(fallback_rx)
    }

    fn sparse_outcome(
        &self,
        _block: &RecoveredBlock<N::Block>,
        output: &BlockExecutionOutput<N::Receipt>,
        outcome: StateRootComputeOutcome,
    ) -> StateRootJobOutcome {
        let StateRootComputeOutcome {
            state_root,
            trie_updates,
            changed_paths,
            #[cfg(feature = "trie-debug")]
            debug_recorders,
        } = outcome;

        if self.compare_trie_updates {
            let _has_diff = compare_trie_updates_with_serial(
                self.provider_builder.clone(),
                self.overlay_factory.clone(),
                output,
                trie_updates.as_ref().clone(),
            );
            #[cfg(feature = "trie-debug")]
            if _has_diff {
                write_trie_debug_recorders(_block.header().number(), &debug_recorders);
            }
        }

        #[cfg(feature = "trie-debug")]
        if state_root != _block.header().state_root() {
            write_trie_debug_recorders(_block.header().number(), &debug_recorders);
        }

        StateRootJobOutcome::new(state_root, trie_updates).with_changed_paths(changed_paths)
    }
}

impl<N, P> StateRootJob<N> for SparseTrieStateRootJob<N, P>
where
    N: NodePrimitives,
    P: BlockReader + StateProviderFactory + StateReader + Clone + Send + Sync + 'static,
    OverlayStateProviderFactory<P, N>: DatabaseProviderROFactory<Provider: TrieCursorFactory + HashedCursorFactory>
        + Clone
        + Send
        + Sync
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
                Ok(outcome) => Ok(self.sparse_outcome(block, &output, outcome)),
                Err(err) => {
                    debug!(target: "engine::tree::payload_validator", %err, "State root task failed, falling back to serial root");
                    let provider = self.provider_builder.clone().build()?;
                    let hashed_state = provider.hashed_post_state(&output.state);
                    let (state_root, trie_updates) =
                        provider.state_root_with_updates(hashed_state.clone())?;
                    Ok(StateRootJobOutcome::new(state_root, Arc::new(trie_updates)))
                }
            }
        }

        let timeout = self.timeout.expect("checked above");
        let task_rx = self.handle.take_state_root_rx();
        let fallback_rx = match task_rx.recv_timeout(timeout) {
            Ok(Ok(outcome)) => return Ok(self.sparse_outcome(block, &output, outcome)),
            Ok(Err(err)) => {
                debug!(target: "engine::tree::payload_validator", %err, "State root task failed, falling back to serial root");
                Self::serial_fallback(
                    &self.executor,
                    self.provider_builder.clone(),
                    output.clone(),
                )?
            }
            Err(RecvTimeoutError::Timeout) => {
                debug!(target: "engine::tree::payload_validator", ?timeout, "State root task timed out, racing serial fallback");
                Self::serial_fallback(
                    &self.executor,
                    self.provider_builder.clone(),
                    output.clone(),
                )?
            }
            Err(RecvTimeoutError::Disconnected) => {
                debug!(target: "engine::tree::payload_validator", "State root task dropped, falling back to serial root");
                Self::serial_fallback(
                    &self.executor,
                    self.provider_builder.clone(),
                    output.clone(),
                )?
            }
        };

        loop {
            if let Ok(result) = task_rx.try_recv() {
                if let Ok(outcome) = result {
                    return Ok(self.sparse_outcome(block, &output, outcome))
                }
            }

            match fallback_rx.try_recv() {
                Ok(Ok((state_root, trie_updates))) => {
                    return Ok(StateRootJobOutcome::new(state_root, Arc::new(trie_updates)))
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
    state_provider_builder: StateProviderBuilder<N, P>,
    overlay_factory: OverlayStateProviderFactory<P, N>,
    output: &BlockExecutionOutput<N::Receipt>,
    task_trie_updates: TrieUpdates,
) -> bool
where
    N: NodePrimitives,
    P: BlockReader + StateProviderFactory + StateReader + Clone,
    OverlayStateProviderFactory<P, N>:
        DatabaseProviderROFactory<Provider: TrieCursorFactory + HashedCursorFactory>,
{
    debug!(target: "engine::tree::payload_validator", "Comparing trie updates with serial computation");

    match state_provider_builder.build().and_then(|provider| {
        let hashed_state = provider.hashed_post_state(&output.state);
        provider.state_root_with_updates(hashed_state)
    }) {
        Ok((serial_root, serial_trie_updates)) => {
            debug!(
                target: "engine::tree::payload_validator",
                ?serial_root,
                "Serial state root computation finished for comparison"
            );

            match overlay_factory.database_provider_ro() {
                Ok(provider) => match super::trie_updates::compare_trie_updates(
                    &provider,
                    task_trie_updates,
                    serial_trie_updates,
                ) {
                    Ok(has_diff) => return has_diff,
                    Err(err) => {
                        warn!(
                            target: "engine::tree::payload_validator",
                            %err,
                            "Error comparing trie updates"
                        );
                        return true;
                    }
                },
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
    false
}

/// Writes trie debug recorders to a JSON file for the given block number.
///
/// The file is written to the current working directory as `trie_debug_block_{block_number}.json`.
#[cfg(feature = "trie-debug")]
fn write_trie_debug_recorders(block_number: u64, recorders: &[(Option<B256>, TrieDebugRecorder)]) {
    let path = format!("trie_debug_block_{block_number}.json");
    match serde_json::to_string_pretty(recorders) {
        Ok(json) => match std::fs::write(&path, json) {
            Ok(()) => {
                warn!(
                    target: "engine::tree::payload_validator",
                    %path,
                    "Wrote trie debug recorders to file"
                );
            }
            Err(err) => {
                warn!(
                    target: "engine::tree::payload_validator",
                    %err,
                    %path,
                    "Failed to write trie debug recorders"
                );
            }
        },
        Err(err) => {
            warn!(
                target: "engine::tree::payload_validator",
                %err,
                "Failed to serialize trie debug recorders"
            );
        }
    }
}
