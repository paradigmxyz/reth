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
    metrics::BlockValidationMetrics,
    multiproof::{StateRootComputeOutcome, StateRootHandle, StateRootMessage},
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
use std::{
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
    /// Returns `true` when the strategy wants access to the retained sparse-trie prune handle.
    fn needs_sparse_trie_prune(&self, _config: &TreeConfig) -> bool {
        false
    }

    /// Prepares a per-block state-root job before execution starts.
    #[expect(clippy::too_many_arguments)]
    fn prepare(
        &self,
        payload_processor: &PayloadProcessor<Evm>,
        env: &ExecutionEnv<Evm>,
        provider_builder: StateProviderBuilder<N, P>,
        overlay_factory: OverlayStateProviderFactory<P, N>,
        config: &TreeConfig,
        parallel_bal_execution: bool,
        pending_sparse_trie_prune: Option<TriePrefixSetsMut>,
    ) -> ProviderResult<Box<dyn StateRootJob<N>>>;
}

/// Per-block state-root job prepared before execution and finished after execution.
pub trait StateRootJob<N: NodePrimitives>: Send {
    /// Human-readable strategy name used in logs.
    fn name(&self) -> &'static str;

    /// Sender used by prewarm/BAL paths to stream sparse-trie updates, if this job needs them.
    fn sparse_trie_updates_tx(&self) -> Option<crossbeam_channel::Sender<StateRootMessage>> {
        None
    }

    /// Hook installed on the EVM database before execution, if this job streams normal state.
    fn execution_hook(&self) -> Option<Box<dyn OnStateHook + 'static>> {
        None
    }

    /// Optional hashed-state receiver produced by a streaming state-root job.
    ///
    /// The sender behind a returned receiver must either deliver one value or be dropped;
    /// validation blocks on it while hashing the post state, so a job that keeps the sender
    /// alive without sending stalls block validation.
    fn take_hashed_state_rx(&mut self) -> Option<mpsc::Receiver<HashedPostState>> {
        None
    }

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
    /// Changed trie node base paths retained while computing the root, if the job tracks them.
    pub changed_paths: Option<Arc<TriePrefixSetsMut>>,
    /// Hashed post state recomputed by a fallback path.
    ///
    /// When set, the root was not derived from the streamed updates, so validation replaces its
    /// streaming-derived hashed post state with this one and re-runs hashed-state checks.
    pub hashed_state: Option<Arc<HashedPostState>>,
}

impl StateRootJobOutcome {
    /// Creates a state-root job outcome without changed paths.
    pub const fn new(state_root: B256, trie_updates: Arc<TrieUpdates>) -> Self {
        Self { state_root, trie_updates, changed_paths: None, hashed_state: None }
    }

    /// Sets the changed trie node base paths retained while computing the root.
    pub fn with_changed_paths(mut self, changed_paths: Option<Arc<TriePrefixSetsMut>>) -> Self {
        self.changed_paths = changed_paths;
        self
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
    fn needs_sparse_trie_prune(&self, config: &TreeConfig) -> bool {
        !config.skip_state_root() && !config.state_root_fallback() && config.use_state_root_task()
    }

    fn prepare(
        &self,
        payload_processor: &PayloadProcessor<Evm>,
        env: &ExecutionEnv<Evm>,
        provider_builder: StateProviderBuilder<N, P>,
        overlay_factory: OverlayStateProviderFactory<P, N>,
        config: &TreeConfig,
        parallel_bal_execution: bool,
        pending_sparse_trie_prune: Option<TriePrefixSetsMut>,
    ) -> ProviderResult<Box<dyn StateRootJob<N>>> {
        if config.skip_state_root() {
            return Ok(Box::new(SkippedStateRootJob {}))
        }

        // `state_root_fallback` forces serial computation for tests and debugging. Hosts
        // without enough parallelism for the state-root task pipeline also compute the root
        // synchronously, since the pipeline's threads can starve each other there; see
        // [`TreeConfig::use_state_root_task`].
        if config.state_root_fallback() || !config.use_state_root_task() {
            return Ok(Box::new(SynchronousStateRootJob { provider_builder }))
        }

        let handle = payload_processor.spawn_state_root(
            overlay_factory.clone(),
            env.parent_state_root,
            Some(env.transaction_count),
            config,
            pending_sparse_trie_prune,
        );

        Ok(Box::new(SparseTrieStateRootJob {
            handle,
            provider_builder,
            overlay_factory,
            executor: payload_processor.executor().clone(),
            timeout: config.state_root_task_timeout(),
            compare_trie_updates: config.always_compare_trie_updates(),
            install_execution_hook: !parallel_bal_execution,
            metrics: BlockValidationMetrics::default(),
        }))
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
    install_execution_hook: bool,
    metrics: BlockValidationMetrics,
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
    ) -> ProviderResult<SerialFallbackRx> {
        let provider = provider_builder.build()?;
        let (fallback_tx, fallback_rx) = mpsc::channel();
        executor.spawn_blocking_named("serial-root", move || {
            let result = (|| {
                let hashed_state = Arc::new(provider.hashed_post_state(&output.state));
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
        let provider = self.provider_builder.clone().build()?;
        let hashed_state = Arc::new(provider.hashed_post_state(&output.state));
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
        let outcome = self.sparse_outcome(block, output, outcome);
        if outcome.state_root == block.header().state_root() {
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

    fn sparse_trie_updates_tx(&self) -> Option<crossbeam_channel::Sender<StateRootMessage>> {
        Some(self.handle.updates_tx().clone())
    }

    fn execution_hook(&self) -> Option<Box<dyn OnStateHook + 'static>> {
        self.install_execution_hook
            .then(|| Box::new(self.handle.state_hook()) as Box<dyn OnStateHook + 'static>)
    }

    fn take_hashed_state_rx(&mut self) -> Option<mpsc::Receiver<HashedPostState>> {
        Some(self.handle.take_hashed_state_rx())
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
                Self::serial_fallback(
                    &self.executor,
                    self.provider_builder.clone(),
                    output.clone(),
                )?
            }
            Err(RecvTimeoutError::Timeout) => {
                warn!(target: "engine::tree::state_root_strategy", ?timeout, "State root task timed out, racing serial fallback");
                self.metrics.state_root_task_timeout_total.increment(1);
                Self::serial_fallback(
                    &self.executor,
                    self.provider_builder.clone(),
                    output.clone(),
                )?
            }
            Err(RecvTimeoutError::Disconnected) => {
                debug!(target: "engine::tree::state_root_strategy", "State root task dropped, falling back to serial root");
                Self::serial_fallback(
                    &self.executor,
                    self.provider_builder.clone(),
                    output.clone(),
                )?
            }
        };

        loop {
            if let Ok(Ok(outcome)) = task_rx.try_recv() {
                let outcome = self.sparse_outcome(block, &output, outcome);
                if outcome.state_root == block.header().state_root() {
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
    debug!(target: "engine::tree::state_root_strategy", "Comparing trie updates with serial computation");

    match state_provider_builder.build().and_then(|provider| {
        let hashed_state = provider.hashed_post_state(&output.state);
        provider.state_root_with_updates(hashed_state)
    }) {
        Ok((serial_root, serial_trie_updates)) => {
            debug!(
                target: "engine::tree::state_root_strategy",
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
                    target: "engine::tree::state_root_strategy",
                    %path,
                    "Wrote trie debug recorders to file"
                );
            }
            Err(err) => {
                warn!(
                    target: "engine::tree::state_root_strategy",
                    %err,
                    %path,
                    "Failed to write trie debug recorders"
                );
            }
        },
        Err(err) => {
            warn!(
                target: "engine::tree::state_root_strategy",
                %err,
                "Failed to serialize trie debug recorders"
            );
        }
    }
}
