use crate::{
    backfill::{BackfillAction, BackfillSyncState},
    chain::FromOrchestrator,
    engine::{DownloadRequest, EngineApiEvent, EngineApiKind, EngineApiRequest, FromEngine},
    persistence::PersistenceHandle,
};
use alloy_eips::BlockNumHash;
use alloy_primitives::{map::B256Map, B256};
use alloy_rpc_types_engine::{ForkchoiceState, PayloadStatus, PayloadStatusEnum};
use error::InsertBlockFatalError;
use payload_build::PayloadBuildTracker;
use reth_chain_state::{
    CanonicalInMemoryState, CanonicalInMemoryStateWriter, ExecutedBlock, ExecutionTimingStats,
    InMemoryStateWriter,
};
use reth_consensus::FullConsensus;
use reth_engine_primitives::{
    BeaconEngineMessage, ConsensusEngineEvent, ExecutionPayload, ForkchoiceStateTracker,
    NewPayloadTimings, OnForkChoiceUpdated,
};
use reth_errors::ProviderResult;
use reth_evm::ConfigureEvm;
use reth_payload_builder::PayloadBuilderHandle;
use reth_payload_primitives::{BuiltPayload, PayloadTypes};
use reth_primitives_traits::{FastInstant as Instant, NodePrimitives};
use reth_provider::{
    BalProvider, BlockReader, ChangeSetReader, DatabaseProviderFactory, PruneCheckpointReader,
    StageCheckpointReader, StateProviderFactory, StateReader, StorageChangeSetReader,
    StorageSettingsCache,
};
use reth_revm::database::StateProviderDatabase;
use reth_storage_overlay::OverlayManager;
use reth_tasks::{spawn_os_thread, utils::increase_thread_priority};
use state::TreeState;
use std::{fmt::Debug, ops, sync::Arc, time::Duration};

use crossbeam_channel::{Receiver, Sender};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
use tracing::*;

mod block_buffer;
mod canonical;
pub mod error;
mod forkchoice;
mod insert;
pub mod instrumented_state;
mod invalid;
mod invalid_headers;
mod metrics;
mod payload_build;
pub mod payload_processor;
pub mod payload_validator;
mod persist;
mod persistence_state;
pub mod precompile_cache;
pub mod state_root_strategy;
mod sync;
#[cfg(test)]
mod tests;
mod trie_updates;
mod txpool_prewarm;
pub mod types;

use crate::persistence::PersistenceResult;
pub use block_buffer::BlockBuffer;
pub use invalid_headers::InvalidHeaderCache;
pub use metrics::EngineApiMetrics;
pub use payload_processor::*;
pub use payload_validator::{BasicEngineValidator, EngineValidator};
pub use persistence_state::PersistenceState;
pub use reth_engine_primitives::TreeConfig;
pub use reth_execution_cache::{
    CachedStateCacheMetrics, CachedStateMetrics, CachedStateMetricsSource, CachedStateProvider,
    ExecutionCache, PayloadExecutionCache, SavedCache, TxPoolPrewarmCacheSnapshot,
};
pub use txpool_prewarm::{
    Source as TxPoolPrewarmSource, Transaction as TxPoolPrewarmTransaction,
    Transactions as TxPoolPrewarmTransactions,
};
pub use types::{ExecutionEnv, ValidationOutcome, ValidationOutput};

pub mod state;

/// The minimum number of blocks to retain in the changeset cache after eviction.
///
/// This ensures that recent changesets are kept in memory for potential reorgs,
/// even when the finalized block is not set (e.g., on L2s like Optimism).
const CHANGESET_CACHE_RETENTION_BLOCKS: u64 = 64;

/// Tracks the state of the engine api internals.
///
/// This type has a single owner. The executed blocks and the canonical chain tracking that it
/// writes are read concurrently through their read handles.
#[derive(Debug)]
pub struct EngineApiTreeState<N: NodePrimitives> {
    /// Reads the executed in-memory blocks and the canonical chain.
    tree_state: TreeState<N>,
    /// Write access to the executed in-memory blocks and the canonical chain tracking.
    canonical_in_memory_state_writer: CanonicalInMemoryStateWriter<N>,
    /// Whether the next sparse trie task should attempt cache pruning during trie preservation.
    pending_sparse_trie_prune: bool,
    /// Tracks the forkchoice state updates received by the CL.
    forkchoice_state_tracker: ForkchoiceStateTracker,
    /// Buffer of detached blocks.
    buffer: BlockBuffer<N::Block>,
    /// Tracks the header of invalid payloads that were rejected by the engine because they're
    /// invalid.
    invalid_headers: InvalidHeaderCache,
}

impl<N: NodePrimitives> EngineApiTreeState<N> {
    fn new(
        block_buffer_limit: u32,
        max_invalid_header_cache_length: u32,
        invalid_header_hit_eviction_threshold: u8,
        canonical_in_memory_state: CanonicalInMemoryState<N>,
        in_memory_state_writer: InMemoryStateWriter<N>,
        overlay_manager: OverlayManager<N>,
    ) -> Self {
        assert!(
            in_memory_state_writer.writes_to(overlay_manager.in_memory_state()),
            "the writer does not write to the in-memory state of the overlay manager"
        );
        Self {
            invalid_headers: InvalidHeaderCache::new(
                max_invalid_header_cache_length,
                invalid_header_hit_eviction_threshold,
            ),
            buffer: BlockBuffer::new(block_buffer_limit),
            tree_state: TreeState::new(canonical_in_memory_state.clone(), overlay_manager),
            canonical_in_memory_state_writer: CanonicalInMemoryStateWriter::new(
                canonical_in_memory_state,
                in_memory_state_writer,
            ),
            pending_sparse_trie_prune: false,
            forkchoice_state_tracker: ForkchoiceStateTracker::default(),
        }
    }

    /// Returns a reference to the tree state.
    pub const fn tree_state(&self) -> &TreeState<N> {
        &self.tree_state
    }

    /// Returns whether sparse trie pruning is pending.
    pub const fn pending_sparse_trie_prune(&self) -> bool {
        self.pending_sparse_trie_prune
    }

    /// Sets whether sparse trie pruning is pending for the next sparse trie task.
    pub const fn set_pending_sparse_trie_prune(&mut self, pending: bool) {
        self.pending_sparse_trie_prune = pending;
    }

    /// Takes a pending sparse trie prune request, if any, and snapshots the in-memory parent chain
    /// ending at `parent_hash`.
    ///
    /// `None` means no prune request is pending. `Some(Vec::new())` means a prune was requested,
    /// but no in-memory parent-chain blocks were found for the parent hash; the sparse trie task
    /// should still prune nodes cached before the current block's epoch.
    pub fn take_sparse_trie_prune_blocks(
        &mut self,
        parent_hash: B256,
    ) -> Option<Vec<ExecutedBlock<N>>> {
        if !self.pending_sparse_trie_prune {
            return None
        }

        self.pending_sparse_trie_prune = false;
        Some(self.tree_state.blocks_by_hash(parent_hash).unwrap_or_default())
    }

    /// Returns true if the block has been marked as invalid.
    pub fn has_invalid_header(&mut self, hash: &B256) -> bool {
        self.invalid_headers.get(hash).is_some()
    }

    /// Tracks a newly executed block. It becomes the pending block if it is built on the
    /// canonical head.
    ///
    /// A block that is already tracked stays in its section.
    fn insert_executed(&mut self, executed: ExecutedBlock<N>) {
        let hash = executed.recovered_block().hash();
        if self.canonical_in_memory_state_writer.insert_executed(executed) {
            self.tree_state.overlay_manager().precompute_child_execution_overlays(hash);
        }
    }
}

/// The outcome of a tree operation.
#[derive(Debug)]
pub struct TreeOutcome<T> {
    /// The outcome of the operation.
    pub outcome: T,
    /// An optional event to tell the caller to do something.
    pub event: Option<TreeEvent>,
    /// Whether the block was already seen, meaning no real execution happened during this
    /// `newPayload` call.
    pub already_seen: bool,
}

impl<T> TreeOutcome<T> {
    /// Create new tree outcome.
    pub const fn new(outcome: T) -> Self {
        Self { outcome, event: None, already_seen: false }
    }

    /// Set event on the outcome.
    pub fn with_event(mut self, event: TreeEvent) -> Self {
        self.event = Some(event);
        self
    }

    /// Set the `already_seen` flag on the outcome.
    pub const fn with_already_seen(mut self, value: bool) -> Self {
        self.already_seen = value;
        self
    }
}

/// Result of trying to insert a new payload in [`EngineApiTreeHandler`].
#[derive(Debug)]
pub struct TryInsertPayloadResult {
    /// - `Valid`: Payload successfully validated and inserted
    /// - `Syncing`: Parent missing, payload buffered for later
    /// - Error status: Payload is invalid
    pub status: PayloadStatus,
    /// Whether the block was already seen
    pub already_seen: bool,
}

impl TryInsertPayloadResult {
    /// Convert the result into a [`TreeOutcome`].
    #[inline]
    pub fn into_outcome(self) -> TreeOutcome<PayloadStatus> {
        TreeOutcome::new(self.status).with_already_seen(self.already_seen)
    }
}

/// Events that are triggered by Tree Chain
#[derive(Debug)]
pub enum TreeEvent {
    /// Tree action is needed.
    TreeAction(TreeAction),
    /// Backfill action is needed.
    BackfillAction(BackfillAction),
    /// Block download is needed.
    Download(DownloadRequest),
}

impl TreeEvent {
    /// Returns true if the event is a backfill action.
    const fn is_backfill_action(&self) -> bool {
        matches!(self, Self::BackfillAction(_))
    }
}

/// The actions that can be performed on the tree.
#[derive(Debug)]
pub enum TreeAction {
    /// Make target canonical.
    MakeCanonical {
        /// The sync target head hash
        sync_target_head: B256,
    },
}

/// The engine API tree handler implementation.
///
/// This type is responsible for processing engine API requests, maintaining the canonical state and
/// emitting events.
pub struct EngineApiTreeHandler<N, P, T, V, C>
where
    N: NodePrimitives,
    T: PayloadTypes,
    C: ConfigureEvm<Primitives = N> + 'static,
{
    provider: P,
    consensus: Arc<dyn FullConsensus<N>>,
    payload_validator: V,
    /// Keeps track of internals such as the executed blocks, the canonical chain and the buffered
    /// blocks.
    state: EngineApiTreeState<N>,
    /// The half for sending messages to the engine.
    ///
    /// This is kept so that we can queue in messages to ourself that we can process later, for
    /// example distributing workload across multiple messages that would otherwise take too long
    /// to process. E.g. we might receive a range of downloaded blocks and we want to process
    /// them one by one so that we can handle incoming engine API in between and don't become
    /// unresponsive. This can happen during live sync transition where we're trying to close the
    /// gap (up to 3 epochs of blocks in the worst case).
    incoming_tx: Sender<FromEngine<EngineApiRequest<T, N>, N::Block>>,
    /// Incoming engine API requests.
    incoming: Receiver<FromEngine<EngineApiRequest<T, N>, N::Block>>,
    /// Outgoing events that are emitted to the handler.
    outgoing: UnboundedSender<EngineApiEvent<N>>,
    /// Channels to the persistence layer.
    persistence: PersistenceHandle<N>,
    /// Tracks the state changes of the persistence task.
    persistence_state: PersistenceState,
    /// Flag indicating the state of the node's backfill synchronization process.
    backfill_sync_state: BackfillSyncState,
    /// Handle to the payload builder that will receive payload attributes for valid forkchoice
    /// updates
    payload_builder: PayloadBuilderHandle<T>,
    /// Configuration settings.
    config: TreeConfig,
    /// Metrics for the engine api.
    metrics: EngineApiMetrics,
    /// The engine API variant of this handler
    engine_kind: EngineApiKind,
    /// The EVM configuration.
    evm_config: C,
    /// Timing statistics for executed blocks, keyed by block hash.
    /// Stored here (not in `ExecutedBlock`) to avoid leaking observability concerns into the block
    /// type. Entries are removed when blocks are persisted or invalidated.
    execution_timing_stats: B256Map<Box<ExecutionTimingStats>>,
    /// Tracks payload jobs that may still access in-memory overlay state.
    payload_builds: PayloadBuildTracker,
    /// Notifies the engine when the final active payload job finishes.
    payload_build_finished: Receiver<()>,
    /// Task runtime for spawning blocking work on named, reusable threads.
    runtime: reth_tasks::Runtime,
}

impl<N, P: Debug, T: PayloadTypes + Debug, V: Debug, C> std::fmt::Debug
    for EngineApiTreeHandler<N, P, T, V, C>
where
    N: NodePrimitives,
    C: Debug + ConfigureEvm<Primitives = N>,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EngineApiTreeHandler")
            .field("provider", &self.provider)
            .field("consensus", &self.consensus)
            .field("payload_validator", &self.payload_validator)
            .field("state", &self.state)
            .field("incoming_tx", &self.incoming_tx)
            .field("persistence", &self.persistence)
            .field("persistence_state", &self.persistence_state)
            .field("backfill_sync_state", &self.backfill_sync_state)
            .field("payload_builder", &self.payload_builder)
            .field("config", &self.config)
            .field("metrics", &self.metrics)
            .field("engine_kind", &self.engine_kind)
            .field("evm_config", &self.evm_config)
            .field("execution_timing_stats", &self.execution_timing_stats.len())
            .field("payload_builds_active", &self.payload_builds.is_active())
            .field("runtime", &self.runtime)
            .finish()
    }
}

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
    /// Creates a new [`EngineApiTreeHandler`].
    ///
    /// `provider` must read the in-memory state that `state` writes to. The handles `state`
    /// was created from read that in-memory state, or creating it panicked; `provider` is not
    /// checked.
    #[expect(clippy::too_many_arguments)]
    pub fn new(
        provider: P,
        consensus: Arc<dyn FullConsensus<N>>,
        payload_validator: V,
        outgoing: UnboundedSender<EngineApiEvent<N>>,
        state: EngineApiTreeState<N>,
        persistence: PersistenceHandle<N>,
        persistence_state: PersistenceState,
        payload_builder: PayloadBuilderHandle<T>,
        config: TreeConfig,
        engine_kind: EngineApiKind,
        evm_config: C,
        runtime: reth_tasks::Runtime,
    ) -> Self {
        let (incoming_tx, incoming) = crossbeam_channel::unbounded();

        let (payload_builds, payload_build_finished) = PayloadBuildTracker::new();

        Self {
            provider,
            consensus,
            payload_validator,
            incoming,
            outgoing,
            persistence,
            persistence_state,
            backfill_sync_state: BackfillSyncState::Idle,
            state,
            payload_builder,
            config,
            metrics: Default::default(),
            incoming_tx,
            engine_kind,
            evm_config,
            execution_timing_stats: B256Map::default(),
            payload_builds,
            payload_build_finished,
            runtime,
        }
    }

    /// Creates a new [`EngineApiTreeHandler`] instance and spawns it in its
    /// own thread.
    ///
    /// Returns the sender through which incoming requests can be sent to the task and the receiver
    /// end of a [`EngineApiEvent`] unbounded channel to receive events from the engine.
    ///
    /// `provider` must serve the in-memory blocks that `in_memory_state_writer` writes. The
    /// canonical head of `canonical_in_memory_state` must be the last persisted block.
    ///
    /// # Panics
    ///
    /// - If `canonical_in_memory_state` or `overlay_manager` do not read the in-memory state that
    ///   `in_memory_state_writer` writes to.
    /// - If the in-memory state has canonical blocks and the highest of them is not the canonical
    ///   head of `canonical_in_memory_state`.
    #[expect(clippy::complexity)]
    pub fn spawn_new(
        provider: P,
        consensus: Arc<dyn FullConsensus<N>>,
        payload_validator: V,
        persistence: PersistenceHandle<N>,
        payload_builder: PayloadBuilderHandle<T>,
        canonical_in_memory_state: CanonicalInMemoryState<N>,
        in_memory_state_writer: InMemoryStateWriter<N>,
        overlay_manager: OverlayManager<N>,
        config: TreeConfig,
        kind: EngineApiKind,
        evm_config: C,
        runtime: reth_tasks::Runtime,
    ) -> (Sender<FromEngine<EngineApiRequest<T, N>, N::Block>>, UnboundedReceiver<EngineApiEvent<N>>)
    {
        let last_persisted_block = canonical_in_memory_state.get_canonical_num_hash();
        let persistence_state = PersistenceState {
            last_persisted_block,
            last_state_trie_persisted_block: last_persisted_block,
            rx: None,
        };

        let (tx, outgoing) = unbounded_channel();
        let state = EngineApiTreeState::new(
            config.block_buffer_limit(),
            config.max_invalid_header_cache_length(),
            config.invalid_header_hit_eviction_threshold(),
            canonical_in_memory_state,
            in_memory_state_writer,
            overlay_manager,
        );

        let task = Self::new(
            provider,
            consensus,
            payload_validator,
            tx,
            state,
            persistence,
            persistence_state,
            payload_builder,
            config,
            kind,
            evm_config,
            runtime,
        );
        let incoming = task.incoming_tx.clone();
        spawn_os_thread("engine", || {
            increase_thread_priority();
            task.run()
        });
        (incoming, outgoing)
    }

    /// Returns a [`TreeOutcome`] indicating the forkchoice head is valid and canonical.
    fn valid_outcome(state: ForkchoiceState) -> TreeOutcome<OnForkChoiceUpdated> {
        TreeOutcome::new(OnForkChoiceUpdated::valid(PayloadStatus::new(
            PayloadStatusEnum::Valid,
            Some(state.head_block_hash),
        )))
    }

    /// Returns a new [`Sender`] to send messages to this type.
    pub fn sender(&self) -> Sender<FromEngine<EngineApiRequest<T, N>, N::Block>> {
        self.incoming_tx.clone()
    }

    /// Run the engine API handler.
    ///
    /// This will block the current thread and process incoming messages.
    pub fn run(mut self) {
        loop {
            // Each iteration has three phases:
            //
            // 1. Non-blocking poll for persistence completion. If the background flush already
            //    landed, absorb the result now so the gap calculation below is fresh.
            // 2. Decide how to wait for the next event. When the canonical-to-persisted gap beyond
            //    the in-memory buffer reaches the backpressure threshold we only block on the
            //    persistence receiver, leaving new engine requests sitting in the unbounded
            //    upstream channel.
            // 3. Handle the event (engine message or persistence completion) and kick off a new
            //    persistence cycle if the threshold is met again.
            //
            // The net effect: when the unbuffered persistence gap reaches the threshold, we stop
            // processing incoming messages and let them queue in the channel. This is only a soft
            // form of backpressure: it delays replies and, more importantly, prevents executing
            // further blocks that would pile up in the persistence queue - where each block
            // carries heavier state (eg. trie updates) than the raw payload sitting in the engine
            // channel.
            //
            // Standard Ethereum CLs won't truly back off - the engine API has no
            // backpressure semantics, and CLs typically timeout after ≈8s and resend - so
            // this cannot prevent the incoming channel from growing under sustained load.
            // But it shifts the bottleneck to the lighter-weight incoming queue rather than
            // the costlier persistence pipeline. Other clients that respect reply latency
            // can treat the delayed responses as a signal to chill out.
            match self.try_poll_persistence() {
                Ok(true) => {
                    if let Err(err) = self.advance_persistence() {
                        error!(target: "engine::tree", %err, "Advancing persistence failed");
                        return
                    }
                    continue;
                }
                Ok(false) => {}
                Err(err) => {
                    error!(target: "engine::tree", %err, "Polling persistence failed");
                    return
                }
            }

            let event = if self.should_backpressure() {
                self.metrics.engine.backpressure_active.set(1.0);
                let stall_start = Instant::now();
                let event = self.wait_for_persistence_event();
                self.metrics.engine.backpressure_stall_duration.record(stall_start.elapsed());
                event
            } else {
                self.metrics.engine.backpressure_active.set(0.0);
                self.wait_for_event()
            };

            match event {
                LoopEvent::EngineMessage(msg) => {
                    debug!(target: "engine::tree", %msg, "received new engine message");
                    match self.on_engine_message(msg) {
                        Ok(ops::ControlFlow::Break(())) => return,
                        Ok(ops::ControlFlow::Continue(())) => {}
                        Err(fatal) => {
                            error!(target: "engine::tree", %fatal, "insert block fatal error");
                            return
                        }
                    }
                }
                LoopEvent::PersistenceComplete { result, start_time } => {
                    if let Err(err) = self.on_persistence_complete(result, start_time) {
                        error!(target: "engine::tree", %err, "Persistence complete handling failed");
                        return
                    }
                }
                LoopEvent::PayloadBuildFinished => {}
                LoopEvent::Disconnected => {
                    error!(target: "engine::tree", "Channel disconnected");
                    return
                }
            }

            // Always check if we need to trigger new persistence after any event:
            // - After engine messages: new blocks may have been inserted that exceed the
            //   persistence threshold
            // - After persistence completion: we can now persist more blocks if needed
            if let Err(err) = self.advance_persistence() {
                error!(target: "engine::tree", %err, "Advancing persistence failed");
                return
            }
        }
    }

    /// Blocks until the in-flight persistence task completes, used when we are under
    /// backpressure.
    ///
    /// Unlike `wait_for_event`, this deliberately does not read from the tree input channel. Any
    /// requests sent to the tree remain queued upstream until persistence catches up.
    fn wait_for_persistence_event(&mut self) -> LoopEvent<T, N> {
        let maybe_persistence = self.persistence_state.rx.take();

        if let Some((persistence_rx, start_time, _action)) = maybe_persistence {
            match persistence_rx.recv() {
                Ok(result) => LoopEvent::PersistenceComplete { result, start_time },
                Err(_) => LoopEvent::Disconnected,
            }
        } else {
            self.wait_for_event()
        }
    }

    /// Blocks until the next event is ready.
    ///
    /// Uses biased selection to prioritize persistence completion so in-memory state is updated and
    /// further writes are unblocked.
    fn wait_for_event(&mut self) -> LoopEvent<T, N> {
        // Take ownership of persistence rx if present
        let maybe_persistence = self.persistence_state.rx.take();

        if let Some((persistence_rx, start_time, action)) = maybe_persistence {
            // Biased select prioritizes persistence completion to update in memory state and
            // unblock further writes
            crossbeam_channel::select_biased! {
                recv(persistence_rx) -> result => {
                    // Don't put it back - consumed (oneshot-like behavior)
                    match result {
                        Ok(result) => LoopEvent::PersistenceComplete {
                            result,
                            start_time,
                        },
                        Err(_) => LoopEvent::Disconnected,
                    }
                },
                recv(self.payload_build_finished) -> result => {
                    // Put the persistence rx back - we didn't consume it.
                    self.persistence_state.rx = Some((persistence_rx, start_time, action));
                    match result {
                        Ok(()) => LoopEvent::PayloadBuildFinished,
                        Err(_) => LoopEvent::Disconnected,
                    }
                },
                recv(self.incoming) -> msg => {
                    // Put the persistence rx back - we didn't consume it
                    self.persistence_state.rx = Some((persistence_rx, start_time, action));
                    match msg {
                        Ok(m) => LoopEvent::EngineMessage(m),
                        Err(_) => LoopEvent::Disconnected,
                    }
                },
            }
        } else {
            // No persistence in progress - wait on an incoming message or payload job completion.
            crossbeam_channel::select_biased! {
                recv(self.payload_build_finished) -> result => match result {
                    Ok(()) => LoopEvent::PayloadBuildFinished,
                    Err(_) => LoopEvent::Disconnected,
                },
                recv(self.incoming) -> msg => match msg {
                    Ok(m) => LoopEvent::EngineMessage(m),
                    Err(_) => LoopEvent::Disconnected,
                },
            }
        }
    }

    /// Handles a message from the engine.
    ///
    /// Returns `ControlFlow::Break(())` if the engine should terminate.
    fn on_engine_message(
        &mut self,
        msg: FromEngine<EngineApiRequest<T, N>, N::Block>,
    ) -> Result<ops::ControlFlow<()>, InsertBlockFatalError> {
        match msg {
            FromEngine::Event(event) => match event {
                FromOrchestrator::BackfillSyncStarted => {
                    debug!(target: "engine::tree", "received backfill sync started event");
                    self.backfill_sync_state = BackfillSyncState::Active;
                }
                FromOrchestrator::BackfillSyncFinished(ctrl) => {
                    self.on_backfill_sync_finished(ctrl)?;
                }
                FromOrchestrator::Terminate { tx } => {
                    debug!(target: "engine::tree", "received terminate request");
                    if let Err(err) = self.finish_termination(tx) {
                        error!(target: "engine::tree", %err, "Termination failed");
                    }
                    return Ok(ops::ControlFlow::Break(()))
                }
            },
            FromEngine::Request(request) => {
                match request {
                    EngineApiRequest::InsertExecutedBlock(payload) => {
                        let block_num_hash = payload.recovered_block.num_hash();
                        if block_num_hash.number <= self.state.tree_state.canonical_block_number() {
                            // outdated block that can be skipped
                            return Ok(ops::ControlFlow::Continue(()))
                        }

                        if self.state.tree_state.contains_hash(&block_num_hash.hash) {
                            // block already known to the tree (e.g. delivered via newPayload first)
                            return Ok(ops::ControlFlow::Continue(()))
                        }

                        debug!(target: "engine::tree", block=?block_num_hash, "inserting already executed block");
                        let now = Instant::now();

                        let block = match self.payload_validator.on_inserted_executed_block(payload)
                        {
                            Ok(block) => block,
                            Err(err) => {
                                warn!(target: "engine::tree", %err, block=?block_num_hash, "Failed to insert already executed block");
                                return Ok(ops::ControlFlow::Continue(()))
                            }
                        };

                        self.state.insert_executed(block.clone());

                        self.metrics.engine.inserted_already_executed_blocks.increment(1);
                        self.emit_event(EngineApiEvent::BeaconConsensus(
                            ConsensusEngineEvent::CanonicalBlockAdded(block, now.elapsed()),
                        ));
                    }
                    EngineApiRequest::Beacon(request) => {
                        match request {
                            BeaconEngineMessage::ForkchoiceUpdated { state, payload_attrs, tx } => {
                                let has_attrs = payload_attrs.is_some();

                                let start = Instant::now();
                                let mut output = self.on_forkchoice_updated(state, payload_attrs);

                                if let Ok(res) = &mut output {
                                    // track last received forkchoice state
                                    self.state
                                        .forkchoice_state_tracker
                                        .set_latest(state, res.outcome.forkchoice_status());

                                    // emit an event about the handled FCU
                                    self.emit_event(ConsensusEngineEvent::ForkchoiceUpdated(
                                        state,
                                        res.outcome.forkchoice_status(),
                                    ));

                                    // handle the event if any
                                    self.on_maybe_tree_event(res.event.take())?;
                                }

                                if let Err(ref err) = output {
                                    error!(target: "engine::tree", %err, ?state, "Error processing forkchoice update");
                                }

                                self.metrics.engine.forkchoice_updated.update_response_metrics(
                                    start,
                                    &mut self.metrics.engine.new_payload.latest_finish_at,
                                    has_attrs,
                                    &output,
                                );

                                if let Err(err) =
                                    tx.send(output.map(|o| o.outcome).map_err(Into::into))
                                {
                                    self.metrics
                                        .engine
                                        .failed_forkchoice_updated_response_deliveries
                                        .increment(1);
                                    warn!(target: "engine::tree", ?state, elapsed=?start.elapsed(), "Failed to deliver forkchoiceUpdated response, receiver dropped (request cancelled): {err:?}");
                                }
                            }
                            BeaconEngineMessage::NewPayload { payload, tx } => {
                                let start = Instant::now();
                                let gas_used = payload.gas_used();
                                let num_hash = payload.num_hash();
                                let mut output = self.on_new_payload(payload);
                                self.metrics.engine.new_payload.update_response_metrics(
                                    start,
                                    &mut self.metrics.engine.forkchoice_updated.latest_finish_at,
                                    &output,
                                    gas_used,
                                );

                                let maybe_event =
                                    output.as_mut().ok().and_then(|out| out.event.take());

                                // emit response
                                if let Err(err) =
                                    tx.send(output.map(|o| o.outcome).map_err(Into::into))
                                {
                                    warn!(target: "engine::tree", payload=?num_hash, elapsed=?start.elapsed(), "Failed to deliver newPayload response, receiver dropped (request cancelled): {err:?}");
                                    self.metrics
                                        .engine
                                        .failed_new_payload_response_deliveries
                                        .increment(1);
                                }

                                // handle the event if any
                                self.on_maybe_tree_event(maybe_event)?;
                            }
                            BeaconEngineMessage::RethNewPayload {
                                payload,
                                wait_for_persistence,
                                wait_for_caches,
                                tx,
                                enqueued_at,
                            } => {
                                debug!(
                                    target: "engine::tree",
                                    wait_for_persistence,
                                    wait_for_caches,
                                    "Processing reth_newPayload"
                                );

                                let backpressure_wait = enqueued_at.elapsed();

                                let explicit_persistence_wait = if wait_for_persistence {
                                    let pending_persistence = self.persistence_state.rx.take();
                                    if let Some((rx, start_time, _action)) = pending_persistence {
                                        let (persistence_tx, persistence_rx) =
                                            std::sync::mpsc::channel();
                                        self.runtime.spawn_blocking_named(
                                            "wait-persist",
                                            move || {
                                                let start = Instant::now();
                                                let result = rx
                                                    .recv()
                                                    .expect("persistence state channel closed");
                                                let _ = persistence_tx.send((
                                                    result,
                                                    start_time,
                                                    start.elapsed(),
                                                ));
                                            },
                                        );
                                        let (result, start_time, wait_duration) = persistence_rx
                                            .recv()
                                            .expect("persistence result channel closed");
                                        let _ = self.on_persistence_complete(result, start_time);
                                        wait_duration
                                    } else {
                                        Duration::ZERO
                                    }
                                } else {
                                    Duration::ZERO
                                };

                                let cache_wait = wait_for_caches
                                    .then(|| self.payload_validator.wait_for_caches());

                                let start = Instant::now();
                                let gas_used = payload.gas_used();
                                let num_hash = payload.num_hash();
                                let mut output = self.on_new_payload(payload);
                                let latency = start.elapsed();
                                self.metrics.engine.new_payload.update_response_metrics(
                                    start,
                                    &mut self.metrics.engine.forkchoice_updated.latest_finish_at,
                                    &output,
                                    gas_used,
                                );

                                let maybe_event =
                                    output.as_mut().ok().and_then(|out| out.event.take());

                                let timings = NewPayloadTimings {
                                    latency,
                                    persistence_wait: backpressure_wait + explicit_persistence_wait,
                                    execution_cache_wait: cache_wait
                                        .map(|wait| wait.execution_cache),
                                    sparse_trie_wait: cache_wait.map(|wait| wait.sparse_trie),
                                };
                                if let Err(err) = tx
                                    .send(output.map(|o| (o.outcome, timings)).map_err(Into::into))
                                {
                                    error!(
                                        target: "engine::tree",
                                        payload=?num_hash,
                                        elapsed=?latency,
                                        "Failed to send event: {err:?}"
                                    );
                                    self.metrics
                                        .engine
                                        .failed_new_payload_response_deliveries
                                        .increment(1);
                                }

                                self.on_maybe_tree_event(maybe_event)?;
                            }
                        }
                    }
                }
            }
            FromEngine::DownloadedBlocks(blocks) => {
                if let Some(event) = self.on_downloaded(blocks)? {
                    self.on_tree_event(event)?;
                }
            }
        }
        Ok(ops::ControlFlow::Continue(()))
    }

    /// Convenience function to handle an optional tree event.
    fn on_maybe_tree_event(&mut self, event: Option<TreeEvent>) -> ProviderResult<()> {
        if let Some(event) = event {
            self.on_tree_event(event)?;
        }

        Ok(())
    }

    /// Handles a tree event.
    ///
    /// Returns an error if a [`TreeAction::MakeCanonical`] results in a fatal error.
    fn on_tree_event(&mut self, event: TreeEvent) -> ProviderResult<()> {
        match event {
            TreeEvent::TreeAction(action) => match action {
                TreeAction::MakeCanonical { sync_target_head } => {
                    self.make_canonical(sync_target_head)?;
                }
            },
            TreeEvent::BackfillAction(action) => {
                self.emit_event(EngineApiEvent::BackfillAction(action));
            }
            TreeEvent::Download(action) => {
                self.emit_event(EngineApiEvent::Download(action));
            }
        }

        Ok(())
    }

    /// Emits an outgoing event to the engine.
    fn emit_event(&mut self, event: impl Into<EngineApiEvent<N>>) {
        let event = event.into();

        if let EngineApiEvent::BackfillAction(action) = event {
            debug_assert_eq!(
                self.backfill_sync_state,
                BackfillSyncState::Idle,
                "backfill action should only be emitted when backfill is idle"
            );

            let persistence_in_progress = self.persistence_state.in_progress();
            let state_trie_needs_catchup = self.persistence_state.last_state_trie_persisted_block !=
                self.persistence_state.last_persisted_block;
            if self.payload_builds.is_active() ||
                persistence_in_progress ||
                state_trie_needs_catchup
            {
                // Backfill can remove the same in-memory blocks as an active payload job or a
                // persistence task. Enter pending mode to prevent new payload jobs and
                // re-evaluate the current sync target after all readers and writes drain.
                debug!(
                    target: "engine::tree",
                    last_persisted_block = self.persistence_state.last_persisted_block.number,
                    last_state_trie_persisted_block = self
                        .persistence_state
                        .last_state_trie_persisted_block
                        .number,
                    "deferring backfill until persistence and payload jobs drain"
                );
                self.backfill_sync_state = BackfillSyncState::PendingRevalidation;
                return
            }

            self.dispatch_backfill_action(action);
            return
        }

        self.send_event(event);
    }

    /// Sends an event to the orchestrator.
    fn send_event(&self, event: EngineApiEvent<N>) {
        let _ = self.outgoing.send(event).inspect_err(
            |err| error!(target: "engine::tree", "Failed to send internal event: {err:?}"),
        );
    }
}

/// Events received in the main engine loop.
#[derive(Debug)]
enum LoopEvent<T, N>
where
    N: NodePrimitives,
    T: PayloadTypes,
{
    /// An engine API message was received.
    EngineMessage(FromEngine<EngineApiRequest<T, N>, N::Block>),
    /// A persistence task completed.
    PersistenceComplete {
        /// The unified result of the persistence operation.
        result: PersistenceResult,
        /// When the persistence operation started.
        start_time: Instant,
    },
    /// The last active payload job has finished, so suppressed persistence may resume.
    PayloadBuildFinished,
    /// A channel was disconnected.
    Disconnected,
}

/// Block inclusion can be valid, accepted, or invalid. Invalid blocks are returned as an error
/// variant.
///
/// If we don't know the block's parent, we return `Disconnected`, as we can't claim that the block
/// is valid or not.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BlockStatus {
    /// The block is valid: its parent state was available, so it was executed and inserted into
    /// the tree.
    ///
    /// Note: this does not imply the block extends the canonical chain. Blocks on a fork are
    /// executed and inserted the same way and report this status as well.
    Valid,
    /// The block may be valid and has an unknown missing ancestor.
    Disconnected {
        /// Current canonical head.
        head: BlockNumHash,
        /// The lowest ancestor block that is not connected to the canonical chain.
        missing_ancestor: BlockNumHash,
    },
}

/// How a payload was inserted if it was valid.
///
/// If the payload was valid, but has already been seen, [`InsertPayloadOk::AlreadySeen`] is
/// returned, otherwise [`InsertPayloadOk::Inserted`] is returned.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InsertPayloadOk {
    /// The payload was valid, but we have already seen it.
    AlreadySeen(BlockStatus),
    /// The payload was valid and inserted into the tree.
    Inserted(BlockStatus),
}

/// Result of waiting for caches to become available.
#[derive(Debug, Clone, Copy, Default)]
pub struct CacheWaitDurations {
    /// Time spent waiting for the execution cache lock, excluding post-unlock cache destruction.
    pub execution_cache: Duration,
    /// Time spent waiting for the sparse trie lock.
    pub sparse_trie: Duration,
}

/// Trait for types that can wait for caches to become available.
///
/// Used by `reth_newPayload` to wait for cache updates before starting payload processing.
/// Removed execution-cache allocations may still be destroyed concurrently after unlocking.
pub trait WaitForCaches {
    /// Waits for cache updates to complete.
    ///
    /// Returns the time spent waiting for each cache separately.
    fn wait_for_caches(&self) -> CacheWaitDurations;
}
