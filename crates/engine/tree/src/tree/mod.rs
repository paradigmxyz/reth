use crate::{
    backfill::{BackfillAction, BackfillSyncState},
    chain::FromOrchestrator,
    engine::{DownloadRequest, EngineApiEvent, EngineApiKind, EngineApiRequest, FromEngine},
    persistence::PersistenceHandle,
    tree::{error::InsertPayloadError, metrics::EngineApiMetrics, payload_validator::TreeCtx},
};
use alloy_consensus::BlockHeader;
use alloy_eips::{eip1898::BlockWithParent, merge::EPOCH_SLOTS, BlockNumHash, NumHash};
use alloy_evm::block::StateChangeSource;
use alloy_primitives::B256;
use alloy_rpc_types_engine::{
    ForkchoiceState, PayloadStatus, PayloadStatusEnum, PayloadValidationError,
};
use error::{InsertBlockError, InsertBlockFatalError};
use reth_chain_state::{
    CanonicalInMemoryState, ComputedTrieData, ExecutedBlock, MemoryOverlayStateProvider,
    NewCanonicalChain,
};
use reth_consensus::{Consensus, FullConsensus};
use reth_engine_primitives::{
    BeaconEngineMessage, BeaconOnNewPayloadError, ConsensusEngineEvent, ExecutionPayload,
    ForkchoiceStateTracker, OnForkChoiceUpdated,
};
use reth_errors::{ConsensusError, ProviderResult};
use reth_evm::{ConfigureEvm, OnStateHook};
use reth_payload_builder::PayloadBuilderHandle;
use reth_payload_primitives::{
    BuiltPayload, EngineApiMessageVersion, NewPayloadError, PayloadBuilderAttributes, PayloadTypes,
};
use reth_primitives_traits::{NodePrimitives, RecoveredBlock, SealedBlock, SealedHeader};
use reth_provider::{
    BlockReader, DatabaseProviderFactory, HashedPostStateProvider, ProviderError, StateProviderBox,
    StateProviderFactory, StateReader, TransactionVariant, TrieReader,
};
use reth_revm::database::StateProviderDatabase;
use reth_stages_api::ControlFlow;
use revm::state::EvmState;
use state::TreeState;
use std::{fmt::Debug, ops, sync::Arc, time::Instant};

use crossbeam_channel::{Receiver, Sender};
use tokio::sync::{
    mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender},
    oneshot,
};
use tracing::*;

mod block_buffer;
mod cached_state;
pub mod error;
pub mod instrumented_state;
mod invalid_headers;
mod metrics;
mod payload_processor;
pub mod payload_validator;
mod persistence_state;
pub mod precompile_cache;
#[cfg(test)]
mod tests;
#[expect(unused)]
mod trie_updates;

use crate::tree::error::AdvancePersistenceError;
pub use block_buffer::BlockBuffer;
pub use invalid_headers::InvalidHeaderCache;
pub use payload_processor::*;
pub use payload_validator::{BasicEngineValidator, EngineValidator};
pub use persistence_state::PersistenceState;
pub use reth_engine_primitives::TreeConfig;

pub mod state;

/// The largest gap for which the tree will be used to sync individual blocks by downloading them.
///
/// This is the default threshold, and represents the distance (gap) from the local head to a
/// new (canonical) block, e.g. the forkchoice head block. If the block distance from the local head
/// exceeds this threshold, the pipeline will be used to backfill the gap more efficiently.
///
/// E.g.: Local head `block.number` is 100 and the forkchoice head `block.number` is 133 (more than
/// an epoch has slots), then this exceeds the threshold at which the pipeline should be used to
/// backfill this gap.
pub(crate) const MIN_BLOCKS_FOR_PIPELINE_RUN: u64 = EPOCH_SLOTS;

/// A builder for creating state providers that can be used across threads.
#[derive(Clone, Debug)]
pub struct StateProviderBuilder<N: NodePrimitives, P> {
    /// The provider factory used to create providers.
    provider_factory: P,
    /// The historical block hash to fetch state from.
    historical: B256,
    /// The blocks that form the chain from historical to target and are in memory.
    overlay: Option<Vec<ExecutedBlock<N>>>,
}

impl<N: NodePrimitives, P> StateProviderBuilder<N, P> {
    /// Creates a new state provider from the provider factory, historical block hash and optional
    /// overlaid blocks.
    pub const fn new(
        provider_factory: P,
        historical: B256,
        overlay: Option<Vec<ExecutedBlock<N>>>,
    ) -> Self {
        Self { provider_factory, historical, overlay }
    }
}

impl<N: NodePrimitives, P> StateProviderBuilder<N, P>
where
    P: BlockReader + StateProviderFactory + StateReader + Clone,
{
    /// Creates a new state provider from this builder.
    pub fn build(&self) -> ProviderResult<StateProviderBox> {
        let mut provider = self.provider_factory.state_by_block_hash(self.historical)?;
        if let Some(overlay) = self.overlay.clone() {
            provider = Box::new(MemoryOverlayStateProvider::new(provider, overlay))
        }
        Ok(provider)
    }
}

/// Tracks the state of the engine api internals.
///
/// This type is not shareable.
#[derive(Debug)]
pub struct EngineApiTreeState<N: NodePrimitives> {
    /// Tracks the state of the blockchain tree.
    tree_state: TreeState<N>,
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
        canonical_block: BlockNumHash,
        engine_kind: EngineApiKind,
    ) -> Self {
        Self {
            invalid_headers: InvalidHeaderCache::new(max_invalid_header_cache_length),
            buffer: BlockBuffer::new(block_buffer_limit),
            tree_state: TreeState::new(canonical_block, engine_kind),
            forkchoice_state_tracker: ForkchoiceStateTracker::default(),
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
}

impl<T> TreeOutcome<T> {
    /// Create new tree outcome.
    pub const fn new(outcome: T) -> Self {
        Self { outcome, event: None }
    }

    /// Set event on the outcome.
    pub fn with_event(mut self, event: TreeEvent) -> Self {
        self.event = Some(event);
        self
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

/// Wrapper struct that combines metrics and state hook
struct MeteredStateHook {
    metrics: reth_evm::metrics::ExecutorMetrics,
    inner_hook: Box<dyn OnStateHook>,
}

impl OnStateHook for MeteredStateHook {
    fn on_state(&mut self, source: StateChangeSource, state: &EvmState) {
        // Update the metrics for the number of accounts, storage slots and bytecodes loaded
        let accounts = state.keys().len();
        let storage_slots = state.values().map(|account| account.storage.len()).sum::<usize>();
        let bytecodes = state.values().filter(|account| !account.info.is_empty_code_hash()).count();

        self.metrics.accounts_loaded_histogram.record(accounts as f64);
        self.metrics.storage_slots_loaded_histogram.record(storage_slots as f64);
        self.metrics.bytecodes_loaded_histogram.record(bytecodes as f64);

        // Call the original state hook
        self.inner_hook.on_state(source, state);
    }
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
    /// Keeps track of internals such as executed and buffered blocks.
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
    /// Keeps track of the state of the canonical chain that isn't persisted yet.
    /// This is intended to be accessed from external sources, such as rpc.
    canonical_in_memory_state: CanonicalInMemoryState<N>,
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
            .field("canonical_in_memory_state", &self.canonical_in_memory_state)
            .field("payload_builder", &self.payload_builder)
            .field("config", &self.config)
            .field("metrics", &self.metrics)
            .field("engine_kind", &self.engine_kind)
            .field("evm_config", &self.evm_config)
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
        + HashedPostStateProvider
        + TrieReader
        + Clone
        + 'static,
    <P as DatabaseProviderFactory>::Provider:
        BlockReader<Block = N::Block, Header = N::BlockHeader>,
    C: ConfigureEvm<Primitives = N> + 'static,
    T: PayloadTypes<BuiltPayload: BuiltPayload<Primitives = N>>,
    V: EngineValidator<T>,
{
    /// Creates a new [`EngineApiTreeHandler`].
    #[expect(clippy::too_many_arguments)]
    pub fn new(
        provider: P,
        consensus: Arc<dyn FullConsensus<N>>,
        payload_validator: V,
        outgoing: UnboundedSender<EngineApiEvent<N>>,
        state: EngineApiTreeState<N>,
        canonical_in_memory_state: CanonicalInMemoryState<N>,
        persistence: PersistenceHandle<N>,
        persistence_state: PersistenceState,
        payload_builder: PayloadBuilderHandle<T>,
        config: TreeConfig,
        engine_kind: EngineApiKind,
        evm_config: C,
    ) -> Self {
        let (incoming_tx, incoming) = crossbeam_channel::unbounded();

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
            canonical_in_memory_state,
            payload_builder,
            config,
            metrics: Default::default(),
            incoming_tx,
            engine_kind,
            evm_config,
        }
    }

    /// Creates a new [`EngineApiTreeHandler`] instance and spawns it in its
    /// own thread.
    ///
    /// Returns the sender through which incoming requests can be sent to the task and the receiver
    /// end of a [`EngineApiEvent`] unbounded channel to receive events from the engine.
    #[expect(clippy::complexity)]
    pub fn spawn_new(
        provider: P,
        consensus: Arc<dyn FullConsensus<N>>,
        payload_validator: V,
        persistence: PersistenceHandle<N>,
        payload_builder: PayloadBuilderHandle<T>,
        canonical_in_memory_state: CanonicalInMemoryState<N>,
        config: TreeConfig,
        kind: EngineApiKind,
        evm_config: C,
    ) -> (Sender<FromEngine<EngineApiRequest<T, N>, N::Block>>, UnboundedReceiver<EngineApiEvent<N>>)
    {
        let best_block_number = provider.best_block_number().unwrap_or(0);
        let header = provider.sealed_header(best_block_number).ok().flatten().unwrap_or_default();

        let persistence_state = PersistenceState {
            last_persisted_block: BlockNumHash::new(best_block_number, header.hash()),
            rx: None,
        };

        let (tx, outgoing) = unbounded_channel();
        let state = EngineApiTreeState::new(
            config.block_buffer_limit(),
            config.max_invalid_header_cache_length(),
            header.num_hash(),
            kind,
        );

        let task = Self::new(
            provider,
            consensus,
            payload_validator,
            tx,
            state,
            canonical_in_memory_state,
            persistence,
            persistence_state,
            payload_builder,
            config,
            kind,
            evm_config,
        );
        let incoming = task.incoming_tx.clone();
        std::thread::Builder::new().name("Engine Task".to_string()).spawn(|| task.run()).unwrap();
        (incoming, outgoing)
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
            match self.wait_for_event() {
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

    /// Blocks until the next event is ready: either an incoming engine message or a persistence
    /// completion (if one is in progress).
    ///
    /// Uses biased selection to prioritize persistence completion to update in-memory state and
    /// unblock further writes.
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
                        Ok(value) => LoopEvent::PersistenceComplete {
                            result: value,
                            start_time,
                        },
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
            // No persistence in progress - just wait on incoming
            match self.incoming.recv() {
                Ok(m) => LoopEvent::EngineMessage(m),
                Err(_) => LoopEvent::Disconnected,
            }
        }
    }

    /// Invoked when previously requested blocks were downloaded.
    ///
    /// If the block count exceeds the configured batch size we're allowed to execute at once, this
    /// will execute the first batch and send the remaining blocks back through the channel so that
    /// block request processing isn't blocked for a long time.
    fn on_downloaded(
        &mut self,
        mut blocks: Vec<SealedBlock<N::Block>>,
    ) -> Result<Option<TreeEvent>, InsertBlockFatalError> {
        if blocks.is_empty() {
            // nothing to execute
            return Ok(None)
        }

        trace!(target: "engine::tree", block_count = %blocks.len(), "received downloaded blocks");
        let batch = self.config.max_execute_block_batch_size().min(blocks.len());
        for block in blocks.drain(..batch) {
            if let Some(event) = self.on_downloaded_block(block)? {
                let needs_backfill = event.is_backfill_action();
                self.on_tree_event(event)?;
                if needs_backfill {
                    // can exit early if backfill is needed
                    return Ok(None)
                }
            }
        }

        // if we still have blocks to execute, send them as a followup request
        if !blocks.is_empty() {
            let _ = self.incoming_tx.send(FromEngine::DownloadedBlocks(blocks));
        }

        Ok(None)
    }

    /// When the Consensus layer receives a new block via the consensus gossip protocol,
    /// the transactions in the block are sent to the execution layer in the form of a
    /// [`PayloadTypes::ExecutionData`], for example
    /// [`ExecutionData`](reth_payload_primitives::PayloadTypes::ExecutionData). The
    /// Execution layer executes the transactions and validates the state in the block header,
    /// then passes validation data back to Consensus layer, that adds the block to the head of
    /// its own blockchain and attests to it. The block is then broadcast over the consensus p2p
    /// network in the form of a "Beacon block".
    ///
    /// These responses should adhere to the [Engine API Spec for
    /// `engine_newPayload`](https://github.com/ethereum/execution-apis/blob/main/src/engine/paris.md#specification).
    ///
    /// This returns a [`PayloadStatus`] that represents the outcome of a processed new payload and
    /// returns an error if an internal error occurred.
    #[instrument(
        level = "debug",
        target = "engine::tree",
        skip_all,
        fields(block_hash = %payload.block_hash(), block_num = %payload.block_number()),
    )]
    fn on_new_payload(
        &mut self,
        payload: T::ExecutionData,
    ) -> Result<TreeOutcome<PayloadStatus>, InsertBlockFatalError> {
        trace!(target: "engine::tree", "invoked new payload");

        // start timing for the new payload process
        let start = Instant::now();

        // Ensures that the given payload does not violate any consensus rules that concern the
        // block's layout, like:
        //    - missing or invalid base fee
        //    - invalid extra data
        //    - invalid transactions
        //    - incorrect hash
        //    - the versioned hashes passed with the payload do not exactly match transaction
        //      versioned hashes
        //    - the block does not contain blob transactions if it is pre-cancun
        //
        // This validates the following engine API rule:
        //
        // 3. Given the expected array of blob versioned hashes client software **MUST** run its
        //    validation by taking the following steps:
        //
        //   1. Obtain the actual array by concatenating blob versioned hashes lists
        //      (`tx.blob_versioned_hashes`) of each [blob
        //      transaction](https://eips.ethereum.org/EIPS/eip-4844#new-transaction-type) included
        //      in the payload, respecting the order of inclusion. If the payload has no blob
        //      transactions the expected array **MUST** be `[]`.
        //
        //   2. Return `{status: INVALID, latestValidHash: null, validationError: errorMessage |
        //      null}` if the expected and the actual arrays don't match.
        //
        // This validation **MUST** be instantly run in all cases even during active sync process.

        let num_hash = payload.num_hash();
        let engine_event = ConsensusEngineEvent::BlockReceived(num_hash);
        self.emit_event(EngineApiEvent::BeaconConsensus(engine_event));

        let block_hash = num_hash.hash;

        // Check for invalid ancestors
        if let Some(invalid) = self.find_invalid_ancestor(&payload) {
            let status = self.handle_invalid_ancestor_payload(payload, invalid)?;
            return Ok(TreeOutcome::new(status));
        }

        // record pre-execution phase duration
        self.metrics.block_validation.record_payload_validation(start.elapsed().as_secs_f64());

        let status = if self.backfill_sync_state.is_idle() {
            self.try_insert_payload(payload)?
        } else {
            self.try_buffer_payload(payload)?
        };

        let mut outcome = TreeOutcome::new(status);
        // if the block is valid and it is the current sync target head, make it canonical
        if outcome.outcome.is_valid() && self.is_sync_target_head(block_hash) {
            // Only create the canonical event if this block isn't already the canonical head
            if self.state.tree_state.canonical_block_hash() != block_hash {
                outcome = outcome.with_event(TreeEvent::TreeAction(TreeAction::MakeCanonical {
                    sync_target_head: block_hash,
                }));
            }
        }

        // record total newPayload duration
        self.metrics.block_validation.total_duration.record(start.elapsed().as_secs_f64());

        Ok(outcome)
    }

    /// Processes a payload during normal sync operation.
    ///
    /// Returns:
    /// - `Valid`: Payload successfully validated and inserted
    /// - `Syncing`: Parent missing, payload buffered for later
    /// - Error status: Payload is invalid
    #[instrument(level = "debug", target = "engine::tree", skip_all)]
    fn try_insert_payload(
        &mut self,
        payload: T::ExecutionData,
    ) -> Result<PayloadStatus, InsertBlockFatalError> {
        let block_hash = payload.block_hash();
        let num_hash = payload.num_hash();
        let parent_hash = payload.parent_hash();
        let mut latest_valid_hash = None;

        match self.insert_payload(payload) {
            Ok(status) => {
                let status = match status {
                    InsertPayloadOk::Inserted(BlockStatus::Valid) => {
                        latest_valid_hash = Some(block_hash);
                        self.try_connect_buffered_blocks(num_hash)?;
                        PayloadStatusEnum::Valid
                    }
                    InsertPayloadOk::AlreadySeen(BlockStatus::Valid) => {
                        latest_valid_hash = Some(block_hash);
                        PayloadStatusEnum::Valid
                    }
                    InsertPayloadOk::Inserted(BlockStatus::Disconnected { .. }) |
                    InsertPayloadOk::AlreadySeen(BlockStatus::Disconnected { .. }) => {
                        // not known to be invalid, but we don't know anything else
                        PayloadStatusEnum::Syncing
                    }
                };

                Ok(PayloadStatus::new(status, latest_valid_hash))
            }
            Err(error) => match error {
                InsertPayloadError::Block(error) => Ok(self.on_insert_block_error(error)?),
                InsertPayloadError::Payload(error) => {
                    Ok(self.on_new_payload_error(error, num_hash, parent_hash)?)
                }
            },
        }
    }

    /// Stores a payload for later processing during backfill sync.
    ///
    /// During backfill, the node lacks the state needed to validate payloads,
    /// so they are buffered (stored in memory) until their parent blocks are synced.
    ///
    /// Returns:
    /// - `Syncing`: Payload successfully buffered
    /// - Error status: Payload is malformed or invalid
    fn try_buffer_payload(
        &mut self,
        payload: T::ExecutionData,
    ) -> Result<PayloadStatus, InsertBlockFatalError> {
        let parent_hash = payload.parent_hash();
        let num_hash = payload.num_hash();

        match self.payload_validator.convert_payload_to_block(payload) {
            // if the block is well-formed, buffer it for later
            Ok(block) => {
                if let Err(error) = self.buffer_block(block) {
                    Ok(self.on_insert_block_error(error)?)
                } else {
                    Ok(PayloadStatus::from_status(PayloadStatusEnum::Syncing))
                }
            }
            Err(error) => Ok(self.on_new_payload_error(error, num_hash, parent_hash)?),
        }
    }

    /// Returns the new chain for the given head.
    ///
    /// This also handles reorgs.
    ///
    /// Note: This does not update the tracked state and instead returns the new chain based on the
    /// given head.
    fn on_new_head(&self, new_head: B256) -> ProviderResult<Option<NewCanonicalChain<N>>> {
        // get the executed new head block
        let Some(new_head_block) = self.state.tree_state.blocks_by_hash.get(&new_head) else {
            debug!(target: "engine::tree", new_head=?new_head, "New head block not found in inmemory tree state");
            self.metrics.engine.executed_new_block_cache_miss.increment(1);
            return Ok(None)
        };

        let new_head_number = new_head_block.recovered_block().number();
        let mut current_canonical_number = self.state.tree_state.current_canonical_head.number;

        let mut new_chain = vec![new_head_block.clone()];
        let mut current_hash = new_head_block.recovered_block().parent_hash();
        let mut current_number = new_head_number - 1;

        // Walk back the new chain until we reach a block we know about
        //
        // This is only done for in-memory blocks, because we should not have persisted any blocks
        // that are _above_ the current canonical head.
        while current_number > current_canonical_number {
            if let Some(block) = self.state.tree_state.executed_block_by_hash(current_hash).cloned()
            {
                current_hash = block.recovered_block().parent_hash();
                current_number -= 1;
                new_chain.push(block);
            } else {
                warn!(target: "engine::tree", current_hash=?current_hash, "Sidechain block not found in TreeState");
                // This should never happen as we're walking back a chain that should connect to
                // the canonical chain
                return Ok(None)
            }
        }

        // If we have reached the current canonical head by walking back from the target, then we
        // know this represents an extension of the canonical chain.
        if current_hash == self.state.tree_state.current_canonical_head.hash {
            new_chain.reverse();

            // Simple extension of the current chain
            return Ok(Some(NewCanonicalChain::Commit { new: new_chain }))
        }

        // We have a reorg. Walk back both chains to find the fork point.
        let mut old_chain = Vec::new();
        let mut old_hash = self.state.tree_state.current_canonical_head.hash;

        // If the canonical chain is ahead of the new chain,
        // gather all blocks until new head number.
        while current_canonical_number > current_number {
            if let Some(block) = self.canonical_block_by_hash(old_hash)? {
                old_hash = block.recovered_block().parent_hash();
                old_chain.push(block);
                current_canonical_number -= 1;
            } else {
                // This shouldn't happen as we're walking back the canonical chain
                warn!(target: "engine::tree", current_hash=?old_hash, "Canonical block not found in TreeState");
                return Ok(None)
            }
        }

        // Both new and old chain pointers are now at the same height.
        debug_assert_eq!(current_number, current_canonical_number);

        // Walk both chains from specified hashes at same height until
        // a common ancestor (fork block) is reached.
        while old_hash != current_hash {
            if let Some(block) = self.canonical_block_by_hash(old_hash)? {
                old_hash = block.recovered_block().parent_hash();
                old_chain.push(block);
            } else {
                // This shouldn't happen as we're walking back the canonical chain
                warn!(target: "engine::tree", current_hash=?old_hash, "Canonical block not found in TreeState");
                return Ok(None)
            }

            if let Some(block) = self.state.tree_state.executed_block_by_hash(current_hash).cloned()
            {
                current_hash = block.recovered_block().parent_hash();
                new_chain.push(block);
            } else {
                // This shouldn't happen as we've already walked this path
                warn!(target: "engine::tree", invalid_hash=?current_hash, "New chain block not found in TreeState");
                return Ok(None)
            }
        }
        new_chain.reverse();
        old_chain.reverse();

        Ok(Some(NewCanonicalChain::Reorg { new: new_chain, old: old_chain }))
    }

    /// Updates the latest block state to the specified canonical ancestor.
    ///
    /// This method ensures that the latest block tracks the given canonical header by resetting
    ///
    /// # Arguments
    /// * `canonical_header` - The canonical header to set as the new head
    ///
    /// # Returns
    /// * `ProviderResult<()>` - Ok(()) on success, error if state update fails
    ///
    /// Caution: This unwinds the canonical chain
    fn update_latest_block_to_canonical_ancestor(
        &mut self,
        canonical_header: &SealedHeader<N::BlockHeader>,
    ) -> ProviderResult<()> {
        debug!(target: "engine::tree", head = ?canonical_header.num_hash(), "Update latest block to canonical ancestor");
        let current_head_number = self.state.tree_state.canonical_block_number();
        let new_head_number = canonical_header.number();
        let new_head_hash = canonical_header.hash();

        // Update tree state with the new canonical head
        self.state.tree_state.set_canonical_head(canonical_header.num_hash());

        // Handle the state update based on whether this is an unwind scenario
        if new_head_number < current_head_number {
            debug!(
                target: "engine::tree",
                current_head = current_head_number,
                new_head = new_head_number,
                new_head_hash = ?new_head_hash,
                "FCU unwind detected: reverting to canonical ancestor"
            );

            self.handle_canonical_chain_unwind(current_head_number, canonical_header)
        } else {
            debug!(
                target: "engine::tree",
                previous_head = current_head_number,
                new_head = new_head_number,
                new_head_hash = ?new_head_hash,
                "Advancing latest block to canonical ancestor"
            );
            self.handle_chain_advance_or_same_height(canonical_header)
        }
    }

    /// Handles chain unwind scenarios by collecting blocks to remove and performing an unwind back
    /// to the canonical header
    fn handle_canonical_chain_unwind(
        &self,
        current_head_number: u64,
        canonical_header: &SealedHeader<N::BlockHeader>,
    ) -> ProviderResult<()> {
        let new_head_number = canonical_header.number();
        debug!(
            target: "engine::tree",
            from = current_head_number,
            to = new_head_number,
            "Handling unwind: collecting blocks to remove from in-memory state"
        );

        // Collect blocks that need to be removed from memory
        let old_blocks =
            self.collect_blocks_for_canonical_unwind(new_head_number, current_head_number);

        // Load and apply the canonical ancestor block
        self.apply_canonical_ancestor_via_reorg(canonical_header, old_blocks)
    }

    /// Collects blocks from memory that need to be removed during an unwind to a canonical block.
    fn collect_blocks_for_canonical_unwind(
        &self,
        new_head_number: u64,
        current_head_number: u64,
    ) -> Vec<ExecutedBlock<N>> {
        let mut old_blocks =
            Vec::with_capacity((current_head_number.saturating_sub(new_head_number)) as usize);

        for block_num in (new_head_number + 1)..=current_head_number {
            if let Some(block_state) = self.canonical_in_memory_state.state_by_number(block_num) {
                let executed_block = block_state.block_ref().clone();
                old_blocks.push(executed_block);
                debug!(
                    target: "engine::tree",
                    block_number = block_num,
                    "Collected block for removal from in-memory state"
                );
            }
        }

        if old_blocks.is_empty() {
            debug!(
                target: "engine::tree",
                "No blocks found in memory to remove, will clear and reset state"
            );
        }

        old_blocks
    }

    /// Applies the canonical ancestor block via a reorg operation.
    fn apply_canonical_ancestor_via_reorg(
        &self,
        canonical_header: &SealedHeader<N::BlockHeader>,
        old_blocks: Vec<ExecutedBlock<N>>,
    ) -> ProviderResult<()> {
        let new_head_hash = canonical_header.hash();
        let new_head_number = canonical_header.number();

        // Try to load the canonical ancestor's block
        match self.canonical_block_by_hash(new_head_hash)? {
            Some(executed_block) => {
                // Perform the reorg to properly handle the unwind
                self.canonical_in_memory_state.update_chain(NewCanonicalChain::Reorg {
                    new: vec![executed_block],
                    old: old_blocks,
                });

                // CRITICAL: Update the canonical head after the reorg
                // This ensures get_canonical_head() returns the correct block
                self.canonical_in_memory_state.set_canonical_head(canonical_header.clone());

                debug!(
                    target: "engine::tree",
                    block_number = new_head_number,
                    block_hash = ?new_head_hash,
                    "Successfully loaded canonical ancestor into memory via reorg"
                );
            }
            None => {
                // Fallback: update header only if block cannot be found
                warn!(
                    target: "engine::tree",
                    block_hash = ?new_head_hash,
                    "Could not find canonical ancestor block, updating header only"
                );
                self.canonical_in_memory_state.set_canonical_head(canonical_header.clone());
            }
        }

        Ok(())
    }

    /// Handles chain advance or same height scenarios.
    fn handle_chain_advance_or_same_height(
        &self,
        canonical_header: &SealedHeader<N::BlockHeader>,
    ) -> ProviderResult<()> {
        let new_head_number = canonical_header.number();
        let new_head_hash = canonical_header.hash();

        // Update the canonical head header
        self.canonical_in_memory_state.set_canonical_head(canonical_header.clone());

        // Load the block into memory if it's not already present
        self.ensure_block_in_memory(new_head_number, new_head_hash)
    }

    /// Ensures a block is loaded into memory if not already present.
    fn ensure_block_in_memory(&self, block_number: u64, block_hash: B256) -> ProviderResult<()> {
        // Check if block is already in memory
        if self.canonical_in_memory_state.state_by_number(block_number).is_some() {
            return Ok(());
        }

        // Try to load the block from storage
        if let Some(executed_block) = self.canonical_block_by_hash(block_hash)? {
            self.canonical_in_memory_state
                .update_chain(NewCanonicalChain::Commit { new: vec![executed_block] });

            debug!(
                target: "engine::tree",
                block_number,
                block_hash = ?block_hash,
                "Added canonical block to in-memory state"
            );
        }

        Ok(())
    }

    /// Invoked when we receive a new forkchoice update message. Calls into the blockchain tree
    /// to resolve chain forks and ensure that the Execution Layer is working with the latest valid
    /// chain.
    ///
    /// These responses should adhere to the [Engine API Spec for
    /// `engine_forkchoiceUpdated`](https://github.com/ethereum/execution-apis/blob/main/src/engine/paris.md#specification-1).
    ///
    /// Returns an error if an internal error occurred like a database error.
    #[instrument(level = "debug", target = "engine::tree", skip_all, fields(head = % state.head_block_hash, safe = % state.safe_block_hash,finalized = % state.finalized_block_hash))]
    fn on_forkchoice_updated(
        &mut self,
        state: ForkchoiceState,
        attrs: Option<T::PayloadAttributes>,
        version: EngineApiMessageVersion,
    ) -> ProviderResult<TreeOutcome<OnForkChoiceUpdated>> {
        trace!(target: "engine::tree", ?attrs, "invoked forkchoice update");

        // Record metrics
        self.record_forkchoice_metrics();

        // Pre-validation of forkchoice state
        if let Some(early_result) = self.validate_forkchoice_state(state)? {
            return Ok(TreeOutcome::new(early_result));
        }

        // Return early if we are on the correct fork
        if let Some(result) = self.handle_canonical_head(state, &attrs, version)? {
            return Ok(result);
        }

        // Attempt to apply a chain update when the head differs from our canonical chain.
        // This handles reorgs and chain extensions by making the specified head canonical.
        if let Some(result) = self.apply_chain_update(state, &attrs, version)? {
            return Ok(result);
        }

        // Fallback that ensures to catch up to the network's state.
        self.handle_missing_block(state)
    }

    /// Records metrics for forkchoice updated calls
    fn record_forkchoice_metrics(&self) {
        self.canonical_in_memory_state.on_forkchoice_update_received();
    }

    /// Pre-validates the forkchoice state and returns early if validation fails.
    ///
    /// Returns `Some(OnForkChoiceUpdated)` if validation fails and an early response should be
    /// returned. Returns `None` if validation passes and processing should continue.
    fn validate_forkchoice_state(
        &mut self,
        state: ForkchoiceState,
    ) -> ProviderResult<Option<OnForkChoiceUpdated>> {
        if state.head_block_hash.is_zero() {
            return Ok(Some(OnForkChoiceUpdated::invalid_state()));
        }

        // Check if the new head hash is connected to any ancestor that we previously marked as
        // invalid
        let lowest_buffered_ancestor_fcu = self.lowest_buffered_ancestor_or(state.head_block_hash);
        if let Some(status) = self.check_invalid_ancestor(lowest_buffered_ancestor_fcu)? {
            return Ok(Some(OnForkChoiceUpdated::with_invalid(status)));
        }

        if !self.backfill_sync_state.is_idle() {
            // We can only process new forkchoice updates if the pipeline is idle, since it requires
            // exclusive access to the database
            trace!(target: "engine::tree", "Pipeline is syncing, skipping forkchoice update");
            return Ok(Some(OnForkChoiceUpdated::syncing()));
        }

        Ok(None)
    }

    /// Handles the case where the forkchoice head is already canonical.
    ///
    /// Returns `Some(TreeOutcome<OnForkChoiceUpdated>)` if the head is already canonical and
    /// processing is complete. Returns `None` if the head is not canonical and processing
    /// should continue.
    fn handle_canonical_head(
        &self,
        state: ForkchoiceState,
        attrs: &Option<T::PayloadAttributes>, // Changed to reference
        version: EngineApiMessageVersion,
    ) -> ProviderResult<Option<TreeOutcome<OnForkChoiceUpdated>>> {
        // Process the forkchoice update by trying to make the head block canonical
        //
        // We can only process this forkchoice update if:
        // - we have the `head` block
        // - the head block is part of a chain that is connected to the canonical chain. This
        //   includes reorgs.
        //
        // Performing a FCU involves:
        // - marking the FCU's head block as canonical
        // - updating in memory state to reflect the new canonical chain
        // - updating canonical state trackers
        // - emitting a canonicalization event for the new chain (including reorg)
        // - if we have payload attributes, delegate them to the payload service

        if self.state.tree_state.canonical_block_hash() != state.head_block_hash {
            return Ok(None);
        }

        trace!(target: "engine::tree", "fcu head hash is already canonical");

        // Update the safe and finalized blocks and ensure their values are valid
        if let Err(outcome) = self.ensure_consistent_forkchoice_state(state) {
            // safe or finalized hashes are invalid
            return Ok(Some(TreeOutcome::new(outcome)));
        }

        // Process payload attributes if the head is already canonical
        if let Some(attr) = attrs {
            let tip = self
                .sealed_header_by_hash(self.state.tree_state.canonical_block_hash())?
                .ok_or_else(|| {
                    // If we can't find the canonical block, then something is wrong and we need
                    // to return an error
                    ProviderError::HeaderNotFound(state.head_block_hash.into())
                })?;
            // Clone only when we actually need to process the attributes
            let updated = self.process_payload_attributes(attr.clone(), &tip, state, version);
            return Ok(Some(TreeOutcome::new(updated)));
        }

        // The head block is already canonical
        let outcome = TreeOutcome::new(OnForkChoiceUpdated::valid(PayloadStatus::new(
            PayloadStatusEnum::Valid,
            Some(state.head_block_hash),
        )));
        Ok(Some(outcome))
    }

    /// Applies chain update for the new head block and processes payload attributes.
    ///
    /// This method handles the case where the forkchoice head differs from our current canonical
    /// head. It attempts to make the specified head block canonical by:
    /// - Checking if the head is already part of the canonical chain
    /// - Applying chain reorganizations (reorgs) if necessary
    /// - Processing payload attributes if provided
    /// - Returning the appropriate forkchoice update response
    ///
    /// Returns `Some(TreeOutcome<OnForkChoiceUpdated>)` if a chain update was successfully applied.
    /// Returns `None` if no chain update was needed or possible.
    fn apply_chain_update(
        &mut self,
        state: ForkchoiceState,
        attrs: &Option<T::PayloadAttributes>,
        version: EngineApiMessageVersion,
    ) -> ProviderResult<Option<TreeOutcome<OnForkChoiceUpdated>>> {
        // Check if the head is already part of the canonical chain
        if let Ok(Some(canonical_header)) = self.find_canonical_header(state.head_block_hash) {
            debug!(target: "engine::tree", head = canonical_header.number(), "fcu head block is already canonical");

            // For OpStack, or if explicitly configured, the proposers are allowed to reorg their
            // own chain at will, so we need to always trigger a new payload job if requested.
            if self.engine_kind.is_opstack() ||
                self.config.always_process_payload_attributes_on_canonical_head()
            {
                // We need to effectively unwind the _canonical_ chain to the FCU's head, which is
                // part of the canonical chain. We need to update the latest block state to reflect
                // the canonical ancestor. This ensures that state providers and the transaction
                // pool operate with the correct chain state after forkchoice update processing, and
                // new payloads built on the reorg'd head will be added to the tree immediately.
                if self.config.unwind_canonical_header() {
                    self.update_latest_block_to_canonical_ancestor(&canonical_header)?;
                }

                if let Some(attr) = attrs {
                    debug!(target: "engine::tree", head = canonical_header.number(), "handling payload attributes for canonical head");
                    // Clone only when we actually need to process the attributes
                    let updated = self.process_payload_attributes(
                        attr.clone(),
                        &canonical_header,
                        state,
                        version,
                    );
                    return Ok(Some(TreeOutcome::new(updated)));
                }
            }

            // According to the Engine API specification, client software MAY skip an update of the
            // forkchoice state and MUST NOT begin a payload build process if
            // `forkchoiceState.headBlockHash` references a `VALID` ancestor of the head
            // of canonical chain, i.e. the ancestor passed payload validation process
            // and deemed `VALID`. In the case of such an event, client software MUST
            // return `{payloadStatus: {status: VALID, latestValidHash:
            // forkchoiceState.headBlockHash, validationError: null}, payloadId: null}`

            // The head block is already canonical and we're not processing payload attributes,
            // so we're not triggering a payload job and can return right away

            let outcome = TreeOutcome::new(OnForkChoiceUpdated::valid(PayloadStatus::new(
                PayloadStatusEnum::Valid,
                Some(state.head_block_hash),
            )));
            return Ok(Some(outcome));
        }

        // Ensure we can apply a new chain update for the head block
        if let Some(chain_update) = self.on_new_head(state.head_block_hash)? {
            let tip = chain_update.tip().clone_sealed_header();
            self.on_canonical_chain_update(chain_update);

            // Update the safe and finalized blocks and ensure their values are valid
            if let Err(outcome) = self.ensure_consistent_forkchoice_state(state) {
                // safe or finalized hashes are invalid
                return Ok(Some(TreeOutcome::new(outcome)));
            }

            if let Some(attr) = attrs {
                // Clone only when we actually need to process the attributes
                let updated = self.process_payload_attributes(attr.clone(), &tip, state, version);
                return Ok(Some(TreeOutcome::new(updated)));
            }

            let outcome = TreeOutcome::new(OnForkChoiceUpdated::valid(PayloadStatus::new(
                PayloadStatusEnum::Valid,
                Some(state.head_block_hash),
            )));
            return Ok(Some(outcome));
        }

        Ok(None)
    }

    /// Handles the case where the head block is missing and needs to be downloaded.
    ///
    /// This is the fallback case when all other forkchoice update scenarios have been exhausted.
    /// Returns a `TreeOutcome` with syncing status and download event.
    fn handle_missing_block(
        &self,
        state: ForkchoiceState,
    ) -> ProviderResult<TreeOutcome<OnForkChoiceUpdated>> {
        // We don't have the block to perform the forkchoice update
        // We assume the FCU is valid and at least the head is missing,
        // so we need to start syncing to it
        //
        // find the appropriate target to sync to, if we don't have the safe block hash then we
        // start syncing to the safe block via backfill first
        let target = if self.state.forkchoice_state_tracker.is_empty() &&
        // check that safe block is valid and missing
        !state.safe_block_hash.is_zero() &&
        self.find_canonical_header(state.safe_block_hash).ok().flatten().is_none()
        {
            debug!(target: "engine::tree", "missing safe block on initial FCU, downloading safe block");
            state.safe_block_hash
        } else {
            state.head_block_hash
        };

        let target = self.lowest_buffered_ancestor_or(target);
        trace!(target: "engine::tree", %target, "downloading missing block");

        Ok(TreeOutcome::new(OnForkChoiceUpdated::valid(PayloadStatus::from_status(
            PayloadStatusEnum::Syncing,
        )))
        .with_event(TreeEvent::Download(DownloadRequest::single_block(target))))
    }

    /// Helper method to remove blocks and set the persistence state. This ensures we keep track of
    /// the current persistence action while we're removing blocks.
    fn remove_blocks(&mut self, new_tip_num: u64) {
        debug!(target: "engine::tree", ?new_tip_num, last_persisted_block_number=?self.persistence_state.last_persisted_block.number, "Removing blocks using persistence task");
        if new_tip_num < self.persistence_state.last_persisted_block.number {
            debug!(target: "engine::tree", ?new_tip_num, "Starting remove blocks job");
            let (tx, rx) = crossbeam_channel::bounded(1);
            let _ = self.persistence.remove_blocks_above(new_tip_num, tx);
            self.persistence_state.start_remove(new_tip_num, rx);
        }
    }

    /// Helper method to save blocks and set the persistence state. This ensures we keep track of
    /// the current persistence action while we're saving blocks.
    fn persist_blocks(&mut self, blocks_to_persist: Vec<ExecutedBlock<N>>) {
        if blocks_to_persist.is_empty() {
            debug!(target: "engine::tree", "Returned empty set of blocks to persist");
            return
        }

        // NOTE: checked non-empty above
        let highest_num_hash = blocks_to_persist
            .iter()
            .max_by_key(|block| block.recovered_block().number())
            .map(|b| b.recovered_block().num_hash())
            .expect("Checked non-empty persisting blocks");

        debug!(target: "engine::tree", count=blocks_to_persist.len(), blocks = ?blocks_to_persist.iter().map(|block| block.recovered_block().num_hash()).collect::<Vec<_>>(), "Persisting blocks");
        let (tx, rx) = crossbeam_channel::bounded(1);
        let _ = self.persistence.save_blocks(blocks_to_persist, tx);

        self.persistence_state.start_save(highest_num_hash, rx);
    }

    /// Triggers new persistence actions if no persistence task is currently in progress.
    ///
    /// This checks if we need to remove blocks (disk reorg) or save new blocks to disk.
    /// Persistence completion is handled separately via the `wait_for_event` method.
    fn advance_persistence(&mut self) -> Result<(), AdvancePersistenceError> {
        if !self.persistence_state.in_progress() {
            if let Some(new_tip_num) = self.find_disk_reorg()? {
                self.remove_blocks(new_tip_num)
            } else if self.should_persist() {
                let blocks_to_persist =
                    self.get_canonical_blocks_to_persist(PersistTarget::Threshold)?;
                self.persist_blocks(blocks_to_persist);
            }
        }

        Ok(())
    }

    /// Finishes termination by persisting all remaining blocks and signaling completion.
    ///
    /// This blocks until all persistence is complete. Always signals completion,
    /// even if an error occurs.
    fn finish_termination(
        &mut self,
        pending_termination: oneshot::Sender<()>,
    ) -> Result<(), AdvancePersistenceError> {
        trace!(target: "engine::tree", "finishing termination, persisting remaining blocks");
        let result = self.persist_until_complete();
        let _ = pending_termination.send(());
        result
    }

    /// Persists all remaining blocks until none are left.
    fn persist_until_complete(&mut self) -> Result<(), AdvancePersistenceError> {
        loop {
            // Wait for any in-progress persistence to complete (blocking)
            if let Some((rx, start_time, _action)) = self.persistence_state.rx.take() {
                let result = rx.recv().map_err(|_| AdvancePersistenceError::ChannelClosed)?;
                self.on_persistence_complete(result, start_time)?;
            }

            let blocks_to_persist = self.get_canonical_blocks_to_persist(PersistTarget::Head)?;

            if blocks_to_persist.is_empty() {
                debug!(target: "engine::tree", "persistence complete, signaling termination");
                return Ok(())
            }

            debug!(target: "engine::tree", count = blocks_to_persist.len(), "persisting remaining blocks before shutdown");
            self.persist_blocks(blocks_to_persist);
        }
    }

    /// Tries to poll for a completed persistence task (non-blocking).
    ///
    /// Returns `true` if a persistence task was completed, `false` otherwise.
    #[cfg(test)]
    pub fn try_poll_persistence(&mut self) -> Result<bool, AdvancePersistenceError> {
        let Some((rx, start_time, action)) = self.persistence_state.rx.take() else {
            return Ok(false);
        };

        match rx.try_recv() {
            Ok(result) => {
                self.on_persistence_complete(result, start_time)?;
                Ok(true)
            }
            Err(crossbeam_channel::TryRecvError::Empty) => {
                // Not ready yet, put it back
                self.persistence_state.rx = Some((rx, start_time, action));
                Ok(false)
            }
            Err(crossbeam_channel::TryRecvError::Disconnected) => {
                Err(AdvancePersistenceError::ChannelClosed)
            }
        }
    }

    /// Handles a completed persistence task.
    fn on_persistence_complete(
        &mut self,
        last_persisted_hash_num: Option<BlockNumHash>,
        start_time: Instant,
    ) -> Result<(), AdvancePersistenceError> {
        self.metrics.engine.persistence_duration.record(start_time.elapsed());

        let Some(BlockNumHash {
            hash: last_persisted_block_hash,
            number: last_persisted_block_number,
        }) = last_persisted_hash_num
        else {
            // if this happened, then we persisted no blocks because we sent an empty vec of blocks
            warn!(target: "engine::tree", "Persistence task completed but did not persist any blocks");
            return Ok(())
        };

        debug!(target: "engine::tree", ?last_persisted_block_hash, ?last_persisted_block_number, elapsed=?start_time.elapsed(), "Finished persisting, calling finish");
        self.persistence_state.finish(last_persisted_block_hash, last_persisted_block_number);
        self.on_new_persisted_block()?;
        Ok(())
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
                    EngineApiRequest::InsertExecutedBlock(block) => {
                        let block_num_hash = block.recovered_block().num_hash();
                        if block_num_hash.number <= self.state.tree_state.canonical_block_number() {
                            // outdated block that can be skipped
                            return Ok(ops::ControlFlow::Continue(()))
                        }

                        debug!(target: "engine::tree", block=?block_num_hash, "inserting already executed block");
                        let now = Instant::now();

                        // if the parent is the canonical head, we can insert the block as the
                        // pending block
                        if self.state.tree_state.canonical_block_hash() ==
                            block.recovered_block().parent_hash()
                        {
                            debug!(target: "engine::tree", pending=?block_num_hash, "updating pending block");
                            self.canonical_in_memory_state.set_pending_block(block.clone());
                        }

                        self.state.tree_state.insert_executed(block.clone());
                        self.payload_validator.on_inserted_executed_block(block.clone());
                        self.metrics.engine.inserted_already_executed_blocks.increment(1);
                        self.emit_event(EngineApiEvent::BeaconConsensus(
                            ConsensusEngineEvent::CanonicalBlockAdded(block, now.elapsed()),
                        ));
                    }
                    EngineApiRequest::Beacon(request) => {
                        match request {
                            BeaconEngineMessage::ForkchoiceUpdated {
                                state,
                                payload_attrs,
                                tx,
                                version,
                            } => {
                                let has_attrs = payload_attrs.is_some();

                                let start = Instant::now();
                                let mut output =
                                    self.on_forkchoice_updated(state, payload_attrs, version);

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

                                self.metrics.engine.forkchoice_updated.update_response_metrics(
                                    start,
                                    &mut self.metrics.engine.new_payload.latest_at,
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
                                    error!(target: "engine::tree", ?state, elapsed=?start.elapsed(), "Failed to send event: {err:?}");
                                }
                            }
                            BeaconEngineMessage::NewPayload { payload, tx } => {
                                let start = Instant::now();
                                let gas_used = payload.gas_used();
                                let num_hash = payload.num_hash();
                                let mut output = self.on_new_payload(payload);
                                self.metrics
                                    .engine
                                    .new_payload
                                    .update_response_metrics(start, &output, gas_used);

                                let maybe_event =
                                    output.as_mut().ok().and_then(|out| out.event.take());

                                // emit response
                                if let Err(err) =
                                    tx.send(output.map(|o| o.outcome).map_err(|e| {
                                        BeaconOnNewPayloadError::Internal(Box::new(e))
                                    }))
                                {
                                    error!(target: "engine::tree", payload=?num_hash, elapsed=?start.elapsed(), "Failed to send event: {err:?}");
                                    self.metrics
                                        .engine
                                        .failed_new_payload_response_deliveries
                                        .increment(1);
                                }

                                // handle the event if any
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

    /// Invoked if the backfill sync has finished to target.
    ///
    /// At this point we consider the block synced to the backfill target.
    ///
    /// Checks the tracked finalized block against the block on disk and requests another backfill
    /// run if the distance to the tip exceeds the threshold for another backfill run.
    ///
    /// This will also do the necessary housekeeping of the tree state, this includes:
    ///  - removing all blocks below the backfill height
    ///  - resetting the canonical in-memory state
    ///
    /// In case backfill resulted in an unwind, this will clear the tree state above the unwind
    /// target block.
    fn on_backfill_sync_finished(
        &mut self,
        ctrl: ControlFlow,
    ) -> Result<(), InsertBlockFatalError> {
        debug!(target: "engine::tree", "received backfill sync finished event");
        self.backfill_sync_state = BackfillSyncState::Idle;

        // Pipeline unwound, memorize the invalid block and wait for CL for next sync target.
        let backfill_height = if let ControlFlow::Unwind { bad_block, target } = &ctrl {
            warn!(target: "engine::tree", invalid_block=?bad_block, "Bad block detected in unwind");
            // update the `invalid_headers` cache with the new invalid header
            self.state.invalid_headers.insert(**bad_block);

            // if this was an unwind then the target is the new height
            Some(*target)
        } else {
            // backfill height is the block number that the backfill finished at
            ctrl.block_number()
        };

        // backfill height is the block number that the backfill finished at
        let Some(backfill_height) = backfill_height else { return Ok(()) };

        // state house keeping after backfill sync
        // remove all executed blocks below the backfill height
        //
        // We set the `finalized_num` to `Some(backfill_height)` to ensure we remove all state
        // before that
        let Some(backfill_num_hash) = self
            .provider
            .block_hash(backfill_height)?
            .map(|hash| BlockNumHash { hash, number: backfill_height })
        else {
            debug!(target: "engine::tree", ?ctrl, "Backfill block not found");
            return Ok(())
        };

        if ctrl.is_unwind() {
            // the node reset so we need to clear everything above that height so that backfill
            // height is the new canonical block.
            self.state.tree_state.reset(backfill_num_hash)
        } else {
            self.state.tree_state.remove_until(
                backfill_num_hash,
                self.persistence_state.last_persisted_block.hash,
                Some(backfill_num_hash),
            );
        }

        self.metrics.engine.executed_blocks.set(self.state.tree_state.block_count() as f64);
        self.metrics.tree.canonical_chain_height.set(backfill_height as f64);

        // remove all buffered blocks below the backfill height
        self.state.buffer.remove_old_blocks(backfill_height);
        // we remove all entries because now we're synced to the backfill target and consider this
        // the canonical chain
        self.canonical_in_memory_state.clear_state();

        if let Ok(Some(new_head)) = self.provider.sealed_header(backfill_height) {
            // update the tracked chain height, after backfill sync both the canonical height and
            // persisted height are the same
            self.state.tree_state.set_canonical_head(new_head.num_hash());
            self.persistence_state.finish(new_head.hash(), new_head.number());

            // update the tracked canonical head
            self.canonical_in_memory_state.set_canonical_head(new_head);
        }

        // check if we need to run backfill again by comparing the most recent finalized height to
        // the backfill height
        let Some(sync_target_state) = self.state.forkchoice_state_tracker.sync_target_state()
        else {
            return Ok(())
        };
        if sync_target_state.finalized_block_hash.is_zero() {
            // no finalized block, can't check distance
            return Ok(())
        }
        // get the block number of the finalized block, if we have it
        let newest_finalized = self
            .state
            .buffer
            .block(&sync_target_state.finalized_block_hash)
            .map(|block| block.number());

        // The block number that the backfill finished at - if the progress or newest
        // finalized is None then we can't check the distance anyways.
        //
        // If both are Some, we perform another distance check and return the desired
        // backfill target
        if let Some(backfill_target) =
            ctrl.block_number().zip(newest_finalized).and_then(|(progress, finalized_number)| {
                // Determines whether or not we should run backfill again, in case
                // the new gap is still large enough and requires running backfill again
                self.backfill_sync_target(progress, finalized_number, None)
            })
        {
            // request another backfill run
            self.emit_event(EngineApiEvent::BackfillAction(BackfillAction::Start(
                backfill_target.into(),
            )));
            return Ok(())
        };

        // Check if there are more blocks to sync between current head and FCU target
        if let Some(lowest_buffered) =
            self.state.buffer.lowest_ancestor(&sync_target_state.head_block_hash)
        {
            let current_head_num = self.state.tree_state.current_canonical_head.number;
            let target_head_num = lowest_buffered.number();

            if let Some(distance) = self.distance_from_local_tip(current_head_num, target_head_num)
            {
                // There are blocks between current head and FCU target, download them
                debug!(
                    target: "engine::tree",
                    %current_head_num,
                    %target_head_num,
                    %distance,
                    "Backfill complete, downloading remaining blocks to reach FCU target"
                );

                self.emit_event(EngineApiEvent::Download(DownloadRequest::BlockRange(
                    lowest_buffered.parent_hash(),
                    distance,
                )));
                return Ok(());
            }
        }

        // try to close the gap by executing buffered blocks that are child blocks of the new head
        self.try_connect_buffered_blocks(self.state.tree_state.current_canonical_head)
    }

    /// Attempts to make the given target canonical.
    ///
    /// This will update the tracked canonical in memory state and do the necessary housekeeping.
    fn make_canonical(&mut self, target: B256) -> ProviderResult<()> {
        if let Some(chain_update) = self.on_new_head(target)? {
            self.on_canonical_chain_update(chain_update);
        }

        Ok(())
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

        if event.is_backfill_action() {
            debug_assert_eq!(
                self.backfill_sync_state,
                BackfillSyncState::Idle,
                "backfill action should only be emitted when backfill is idle"
            );

            if self.persistence_state.in_progress() {
                // backfill sync and persisting data are mutually exclusive, so we can't start
                // backfill while we're still persisting
                debug!(target: "engine::tree", "skipping backfill file while persistence task is active");
                return
            }

            self.backfill_sync_state = BackfillSyncState::Pending;
            self.metrics.engine.pipeline_runs.increment(1);
            debug!(target: "engine::tree", "emitting backfill action event");
        }

        let _ = self.outgoing.send(event).inspect_err(
            |err| error!(target: "engine::tree", "Failed to send internal event: {err:?}"),
        );
    }

    /// Returns true if the canonical chain length minus the last persisted
    /// block is greater than or equal to the persistence threshold and
    /// backfill is not running.
    pub const fn should_persist(&self) -> bool {
        if !self.backfill_sync_state.is_idle() {
            // can't persist if backfill is running
            return false
        }

        let min_block = self.persistence_state.last_persisted_block.number;
        self.state.tree_state.canonical_block_number().saturating_sub(min_block) >
            self.config.persistence_threshold()
    }

    /// Returns a batch of consecutive canonical blocks to persist in the range
    /// `(last_persisted_number .. target]`. The expected order is oldest -> newest.
    fn get_canonical_blocks_to_persist(
        &self,
        target: PersistTarget,
    ) -> Result<Vec<ExecutedBlock<N>>, AdvancePersistenceError> {
        // We will calculate the state root using the database, so we need to be sure there are no
        // changes
        debug_assert!(!self.persistence_state.in_progress());

        let mut blocks_to_persist = Vec::new();
        let mut current_hash = self.state.tree_state.canonical_block_hash();
        let last_persisted_number = self.persistence_state.last_persisted_block.number;
        let canonical_head_number = self.state.tree_state.canonical_block_number();

        let target_number = match target {
            PersistTarget::Head => canonical_head_number,
            PersistTarget::Threshold => {
                canonical_head_number.saturating_sub(self.config.memory_block_buffer_target())
            }
        };

        debug!(
            target: "engine::tree",
            ?current_hash,
            ?last_persisted_number,
            ?canonical_head_number,
            ?target_number,
            "Returning canonical blocks to persist"
        );
        while let Some(block) = self.state.tree_state.blocks_by_hash.get(&current_hash) {
            if block.recovered_block().number() <= last_persisted_number {
                break;
            }

            if block.recovered_block().number() <= target_number {
                blocks_to_persist.push(block.clone());
            }

            current_hash = block.recovered_block().parent_hash();
        }

        // Reverse the order so that the oldest block comes first
        blocks_to_persist.reverse();

        Ok(blocks_to_persist)
    }

    /// This clears the blocks from the in-memory tree state that have been persisted to the
    /// database.
    ///
    /// This also updates the canonical in-memory state to reflect the newest persisted block
    /// height.
    ///
    /// Assumes that `finish` has been called on the `persistence_state` at least once
    fn on_new_persisted_block(&mut self) -> ProviderResult<()> {
        // If we have an on-disk reorg, we need to handle it first before touching the in-memory
        // state.
        if let Some(remove_above) = self.find_disk_reorg()? {
            self.remove_blocks(remove_above);
            return Ok(())
        }

        let finalized = self.state.forkchoice_state_tracker.last_valid_finalized();
        self.remove_before(self.persistence_state.last_persisted_block, finalized)?;
        self.canonical_in_memory_state.remove_persisted_blocks(BlockNumHash {
            number: self.persistence_state.last_persisted_block.number,
            hash: self.persistence_state.last_persisted_block.hash,
        });
        Ok(())
    }

    /// Return an [`ExecutedBlock`] from database or in-memory state by hash.
    ///
    /// Note: This function attempts to fetch the `ExecutedBlock` from either in-memory state
    /// or the database. If the required historical data (such as trie change sets) has been
    /// pruned for a given block, this operation will return an error. On archive nodes, it
    /// can retrieve any block.
    fn canonical_block_by_hash(&self, hash: B256) -> ProviderResult<Option<ExecutedBlock<N>>> {
        trace!(target: "engine::tree", ?hash, "Fetching executed block by hash");
        // check memory first
        if let Some(block) = self.state.tree_state.executed_block_by_hash(hash) {
            return Ok(Some(block.clone()))
        }

        let (block, senders) = self
            .provider
            .sealed_block_with_senders(hash.into(), TransactionVariant::WithHash)?
            .ok_or_else(|| ProviderError::HeaderNotFound(hash.into()))?
            .split_sealed();
        let execution_output = self
            .provider
            .get_state(block.header().number())?
            .ok_or_else(|| ProviderError::StateForNumberNotFound(block.header().number()))?;
        let hashed_state = self.provider.hashed_post_state(execution_output.state());
        let trie_updates = self.provider.get_block_trie_updates(block.number())?;

        let sorted_hashed_state = Arc::new(hashed_state.into_sorted());
        let sorted_trie_updates = Arc::new(trie_updates);
        // Skip building trie input and anchor for DB-loaded blocks.
        let trie_data =
            ComputedTrieData::without_trie_input(sorted_hashed_state, sorted_trie_updates);

        Ok(Some(ExecutedBlock::new(
            Arc::new(RecoveredBlock::new_sealed(block, senders)),
            Arc::new(execution_output),
            trie_data,
        )))
    }

    /// Return sealed block header from in-memory state or database by hash.
    fn sealed_header_by_hash(
        &self,
        hash: B256,
    ) -> ProviderResult<Option<SealedHeader<N::BlockHeader>>> {
        // check memory first
        let header = self.state.tree_state.sealed_header_by_hash(&hash);

        if header.is_some() {
            Ok(header)
        } else {
            self.provider.sealed_header_by_hash(hash)
        }
    }

    /// Return the parent hash of the lowest buffered ancestor for the requested block, if there
    /// are any buffered ancestors. If there are no buffered ancestors, and the block itself does
    /// not exist in the buffer, this returns the hash that is passed in.
    ///
    /// Returns the parent hash of the block itself if the block is buffered and has no other
    /// buffered ancestors.
    fn lowest_buffered_ancestor_or(&self, hash: B256) -> B256 {
        self.state
            .buffer
            .lowest_ancestor(&hash)
            .map(|block| block.parent_hash())
            .unwrap_or_else(|| hash)
    }

    /// If validation fails, the response MUST contain the latest valid hash:
    ///
    ///   - The block hash of the ancestor of the invalid payload satisfying the following two
    ///     conditions:
    ///     - It is fully validated and deemed VALID
    ///     - Any other ancestor of the invalid payload with a higher blockNumber is INVALID
    ///   - 0x0000000000000000000000000000000000000000000000000000000000000000 if the above
    ///     conditions are satisfied by a `PoW` block.
    ///   - null if client software cannot determine the ancestor of the invalid payload satisfying
    ///     the above conditions.
    fn latest_valid_hash_for_invalid_payload(
        &mut self,
        parent_hash: B256,
    ) -> ProviderResult<Option<B256>> {
        // Check if parent exists in side chain or in canonical chain.
        if self.sealed_header_by_hash(parent_hash)?.is_some() {
            return Ok(Some(parent_hash))
        }

        // iterate over ancestors in the invalid cache
        // until we encounter the first valid ancestor
        let mut current_hash = parent_hash;
        let mut current_block = self.state.invalid_headers.get(&current_hash);
        while let Some(block_with_parent) = current_block {
            current_hash = block_with_parent.parent;
            current_block = self.state.invalid_headers.get(&current_hash);

            // If current_header is None, then the current_hash does not have an invalid
            // ancestor in the cache, check its presence in blockchain tree
            if current_block.is_none() && self.sealed_header_by_hash(current_hash)?.is_some() {
                return Ok(Some(current_hash))
            }
        }
        Ok(None)
    }

    /// Prepares the invalid payload response for the given hash, checking the
    /// database for the parent hash and populating the payload status with the latest valid hash
    /// according to the engine api spec.
    fn prepare_invalid_response(&mut self, mut parent_hash: B256) -> ProviderResult<PayloadStatus> {
        // Edge case: the `latestValid` field is the zero hash if the parent block is the terminal
        // PoW block, which we need to identify by looking at the parent's block difficulty
        if let Some(parent) = self.sealed_header_by_hash(parent_hash)? &&
            !parent.difficulty().is_zero()
        {
            parent_hash = B256::ZERO;
        }

        let valid_parent_hash = self.latest_valid_hash_for_invalid_payload(parent_hash)?;
        Ok(PayloadStatus::from_status(PayloadStatusEnum::Invalid {
            validation_error: PayloadValidationError::LinksToRejectedPayload.to_string(),
        })
        .with_latest_valid_hash(valid_parent_hash.unwrap_or_default()))
    }

    /// Returns true if the given hash is the last received sync target block.
    ///
    /// See [`ForkchoiceStateTracker::sync_target_state`]
    fn is_sync_target_head(&self, block_hash: B256) -> bool {
        if let Some(target) = self.state.forkchoice_state_tracker.sync_target_state() {
            return target.head_block_hash == block_hash
        }
        false
    }

    /// Returns true if the given hash is part of the last received sync target fork choice update.
    ///
    /// See [`ForkchoiceStateTracker::sync_target_state`]
    fn is_any_sync_target(&self, block_hash: B256) -> bool {
        if let Some(target) = self.state.forkchoice_state_tracker.sync_target_state() {
            return target.contains(block_hash)
        }
        false
    }

    /// Checks if the given `check` hash points to an invalid header, inserting the given `head`
    /// block into the invalid header cache if the `check` hash has a known invalid ancestor.
    ///
    /// Returns a payload status response according to the engine API spec if the block is known to
    /// be invalid.
    fn check_invalid_ancestor_with_head(
        &mut self,
        check: B256,
        head: &SealedBlock<N::Block>,
    ) -> ProviderResult<Option<PayloadStatus>> {
        // check if the check hash was previously marked as invalid
        let Some(header) = self.state.invalid_headers.get(&check) else { return Ok(None) };

        Ok(Some(self.on_invalid_new_payload(head.clone(), header)?))
    }

    /// Invoked when a new payload received is invalid.
    fn on_invalid_new_payload(
        &mut self,
        head: SealedBlock<N::Block>,
        invalid: BlockWithParent,
    ) -> ProviderResult<PayloadStatus> {
        // populate the latest valid hash field
        let status = self.prepare_invalid_response(invalid.parent)?;

        // insert the head block into the invalid header cache
        self.state.invalid_headers.insert_with_invalid_ancestor(head.hash(), invalid);
        self.emit_event(ConsensusEngineEvent::InvalidBlock(Box::new(head)));

        Ok(status)
    }

    /// Finds any invalid ancestor for the given payload.
    ///
    /// This function walks up the chain of buffered ancestors from the payload's block
    /// hash and checks if any ancestor is marked as invalid in the tree state.
    ///
    /// The check works by:
    /// 1. Finding the lowest buffered ancestor for the given block hash
    /// 2. If the ancestor is the same as the block hash itself, using the parent hash instead
    /// 3. Checking if this ancestor is in the `invalid_headers` map
    ///
    /// Returns the invalid ancestor block info if found, or None if no invalid ancestor exists.
    fn find_invalid_ancestor(&mut self, payload: &T::ExecutionData) -> Option<BlockWithParent> {
        let parent_hash = payload.parent_hash();
        let block_hash = payload.block_hash();
        let mut lowest_buffered_ancestor = self.lowest_buffered_ancestor_or(block_hash);
        if lowest_buffered_ancestor == block_hash {
            lowest_buffered_ancestor = parent_hash;
        }

        // Check if the block has an invalid ancestor
        self.state.invalid_headers.get(&lowest_buffered_ancestor)
    }

    /// Handles a payload that has an invalid ancestor.
    ///
    /// This function validates the payload and processes it according to whether it's
    /// well-formed or malformed:
    /// 1. **Well-formed payload**: The payload is marked as invalid since it descends from a
    ///    known-bad block, which violates consensus rules
    /// 2. **Malformed payload**: Returns an appropriate error status since the payload cannot be
    ///    validated due to its own structural issues
    fn handle_invalid_ancestor_payload(
        &mut self,
        payload: T::ExecutionData,
        invalid: BlockWithParent,
    ) -> Result<PayloadStatus, InsertBlockFatalError> {
        let parent_hash = payload.parent_hash();
        let num_hash = payload.num_hash();

        // Here we might have 2 cases
        // 1. the block is well formed and indeed links to an invalid header, meaning we should
        //    remember it as invalid
        // 2. the block is not well formed (i.e block hash is incorrect), and we should just return
        //    an error and forget it
        let block = match self.payload_validator.convert_payload_to_block(payload) {
            Ok(block) => block,
            Err(error) => return Ok(self.on_new_payload_error(error, num_hash, parent_hash)?),
        };

        Ok(self.on_invalid_new_payload(block, invalid)?)
    }

    /// Checks if the given `head` points to an invalid header, which requires a specific response
    /// to a forkchoice update.
    fn check_invalid_ancestor(&mut self, head: B256) -> ProviderResult<Option<PayloadStatus>> {
        // check if the head was previously marked as invalid
        let Some(header) = self.state.invalid_headers.get(&head) else { return Ok(None) };

        // Try to prepare invalid response, but handle errors gracefully
        match self.prepare_invalid_response(header.parent) {
            Ok(status) => Ok(Some(status)),
            Err(err) => {
                debug!(target: "engine::tree", %err, "Failed to prepare invalid response for ancestor check");
                // Return a basic invalid status without latest valid hash
                Ok(Some(PayloadStatus::from_status(PayloadStatusEnum::Invalid {
                    validation_error: PayloadValidationError::LinksToRejectedPayload.to_string(),
                })))
            }
        }
    }

    /// Validate if block is correct and satisfies all the consensus rules that concern the header
    /// and block body itself.
    fn validate_block(&self, block: &SealedBlock<N::Block>) -> Result<(), ConsensusError> {
        if let Err(e) = self.consensus.validate_header(block.sealed_header()) {
            error!(target: "engine::tree", ?block, "Failed to validate header {}: {e}", block.hash());
            return Err(e)
        }

        if let Err(e) = self.consensus.validate_block_pre_execution(block) {
            error!(target: "engine::tree", ?block, "Failed to validate block {}: {e}", block.hash());
            return Err(e)
        }

        Ok(())
    }

    /// Attempts to connect any buffered blocks that are connected to the given parent hash.
    #[instrument(level = "debug", target = "engine::tree", skip(self))]
    fn try_connect_buffered_blocks(
        &mut self,
        parent: BlockNumHash,
    ) -> Result<(), InsertBlockFatalError> {
        let blocks = self.state.buffer.remove_block_with_children(&parent.hash);

        if blocks.is_empty() {
            // nothing to append
            return Ok(())
        }

        let now = Instant::now();
        let block_count = blocks.len();
        for child in blocks {
            let child_num_hash = child.num_hash();
            match self.insert_block(child) {
                Ok(res) => {
                    debug!(target: "engine::tree", child =?child_num_hash, ?res, "connected buffered block");
                    if self.is_any_sync_target(child_num_hash.hash) &&
                        matches!(res, InsertPayloadOk::Inserted(BlockStatus::Valid))
                    {
                        debug!(target: "engine::tree", child =?child_num_hash, "connected sync target block");
                        // we just inserted a block that we know is part of the canonical chain, so
                        // we can make it canonical
                        self.make_canonical(child_num_hash.hash)?;
                    }
                }
                Err(err) => {
                    if let InsertPayloadError::Block(err) = err {
                        debug!(target: "engine::tree", ?err, "failed to connect buffered block to tree");
                        if let Err(fatal) = self.on_insert_block_error(err) {
                            warn!(target: "engine::tree", %fatal, "fatal error occurred while connecting buffered blocks");
                            return Err(fatal)
                        }
                    }
                }
            }
        }

        debug!(target: "engine::tree", elapsed = ?now.elapsed(), %block_count, "connected buffered blocks");
        Ok(())
    }

    /// Pre-validates the block and inserts it into the buffer.
    fn buffer_block(
        &mut self,
        block: SealedBlock<N::Block>,
    ) -> Result<(), InsertBlockError<N::Block>> {
        if let Err(err) = self.validate_block(&block) {
            return Err(InsertBlockError::consensus_error(err, block))
        }
        self.state.buffer.insert_block(block);
        Ok(())
    }

    /// Returns true if the distance from the local tip to the block is greater than the configured
    /// threshold.
    ///
    /// If the `local_tip` is greater than the `block`, then this will return false.
    #[inline]
    const fn exceeds_backfill_run_threshold(&self, local_tip: u64, block: u64) -> bool {
        block > local_tip && block - local_tip > MIN_BLOCKS_FOR_PIPELINE_RUN
    }

    /// Returns how far the local tip is from the given block. If the local tip is at the same
    /// height or its block number is greater than the given block, this returns None.
    #[inline]
    const fn distance_from_local_tip(&self, local_tip: u64, block: u64) -> Option<u64> {
        if block > local_tip {
            Some(block - local_tip)
        } else {
            None
        }
    }

    /// Returns the target hash to sync to if the distance from the local tip to the block is
    /// greater than the threshold and we're not synced to the finalized block yet (if we've seen
    /// that block already).
    ///
    /// If this is invoked after a new block has been downloaded, the downloaded block could be the
    /// (missing) finalized block.
    fn backfill_sync_target(
        &self,
        canonical_tip_num: u64,
        target_block_number: u64,
        downloaded_block: Option<BlockNumHash>,
    ) -> Option<B256> {
        let sync_target_state = self.state.forkchoice_state_tracker.sync_target_state();

        // check if the downloaded block is the tracked finalized block
        let exceeds_backfill_threshold =
            match (downloaded_block.as_ref(), sync_target_state.as_ref()) {
                // if we downloaded the finalized block we can now check how far we're off
                (Some(downloaded_block), Some(state))
                    if downloaded_block.hash == state.finalized_block_hash =>
                {
                    self.exceeds_backfill_run_threshold(canonical_tip_num, downloaded_block.number)
                }
                _ => match sync_target_state
                    .as_ref()
                    .and_then(|state| self.state.buffer.block(&state.finalized_block_hash))
                {
                    Some(buffered_finalized) => {
                        // if we have buffered the finalized block, we should check how far we're
                        // off
                        self.exceeds_backfill_run_threshold(
                            canonical_tip_num,
                            buffered_finalized.number(),
                        )
                    }
                    None => {
                        // check if the distance exceeds the threshold for backfill sync
                        self.exceeds_backfill_run_threshold(canonical_tip_num, target_block_number)
                    }
                },
            };

        // if the number of missing blocks is greater than the max, trigger backfill
        if exceeds_backfill_threshold && let Some(state) = sync_target_state {
            // if we have already canonicalized the finalized block, we should skip backfill
            match self.provider.header_by_hash_or_number(state.finalized_block_hash.into()) {
                Err(err) => {
                    warn!(target: "engine::tree", %err, "Failed to get finalized block header");
                }
                Ok(None) => {
                    // ensure the finalized block is known (not the zero hash)
                    if !state.finalized_block_hash.is_zero() {
                        // we don't have the block yet and the distance exceeds the allowed
                        // threshold
                        return Some(state.finalized_block_hash)
                    }

                    // OPTIMISTIC SYNCING
                    //
                    // It can happen when the node is doing an
                    // optimistic sync, where the CL has no knowledge of the finalized hash,
                    // but is expecting the EL to sync as high
                    // as possible before finalizing.
                    //
                    // This usually doesn't happen on ETH mainnet since CLs use the more
                    // secure checkpoint syncing.
                    //
                    // However, optimism chains will do this. The risk of a reorg is however
                    // low.
                    debug!(target: "engine::tree", hash=?state.head_block_hash, "Setting head hash as an optimistic backfill target.");
                    return Some(state.head_block_hash)
                }
                Ok(Some(_)) => {
                    // we're fully synced to the finalized block
                }
            }
        }

        None
    }

    /// This method tries to detect whether on-disk and in-memory states have diverged. It might
    /// happen if a reorg is happening while we are persisting a block.
    fn find_disk_reorg(&self) -> ProviderResult<Option<u64>> {
        let mut canonical = self.state.tree_state.current_canonical_head;
        let mut persisted = self.persistence_state.last_persisted_block;

        let parent_num_hash = |num_hash: NumHash| -> ProviderResult<NumHash> {
            Ok(self
                .sealed_header_by_hash(num_hash.hash)?
                .ok_or(ProviderError::BlockHashNotFound(num_hash.hash))?
                .parent_num_hash())
        };

        // Happy path, canonical chain is ahead or equal to persisted chain.
        // Walk canonical chain back to make sure that it connects to persisted chain.
        while canonical.number > persisted.number {
            canonical = parent_num_hash(canonical)?;
        }

        // If we've reached persisted tip by walking the canonical chain back, everything is fine.
        if canonical == persisted {
            return Ok(None);
        }

        // At this point, we know that `persisted` block can't be reached by walking the canonical
        // chain back. In this case we need to truncate it to the first canonical block it connects
        // to.

        // Firstly, walk back until we reach the same height as `canonical`.
        while persisted.number > canonical.number {
            persisted = parent_num_hash(persisted)?;
        }

        debug_assert_eq!(persisted.number, canonical.number);

        // Now walk both chains back until we find a common ancestor.
        while persisted.hash != canonical.hash {
            canonical = parent_num_hash(canonical)?;
            persisted = parent_num_hash(persisted)?;
        }

        debug!(target: "engine::tree", remove_above=persisted.number, "on-disk reorg detected");

        Ok(Some(persisted.number))
    }

    /// Invoked when we the canonical chain has been updated.
    ///
    /// This is invoked on a valid forkchoice update, or if we can make the target block canonical.
    fn on_canonical_chain_update(&mut self, chain_update: NewCanonicalChain<N>) {
        trace!(target: "engine::tree", new_blocks = %chain_update.new_block_count(), reorged_blocks =  %chain_update.reorged_block_count(), "applying new chain update");
        let start = Instant::now();

        // update the tracked canonical head
        self.state.tree_state.set_canonical_head(chain_update.tip().num_hash());

        let tip = chain_update.tip().clone_sealed_header();
        let notification = chain_update.to_chain_notification();

        // reinsert any missing reorged blocks
        if let NewCanonicalChain::Reorg { new, old } = &chain_update {
            let new_first = new.first().map(|first| first.recovered_block().num_hash());
            let old_first = old.first().map(|first| first.recovered_block().num_hash());
            trace!(target: "engine::tree", ?new_first, ?old_first, "Reorg detected, new and old first blocks");

            self.update_reorg_metrics(old.len());
            self.reinsert_reorged_blocks(new.clone());
            self.reinsert_reorged_blocks(old.clone());
        }

        // update the tracked in-memory state with the new chain
        self.canonical_in_memory_state.update_chain(chain_update);
        self.canonical_in_memory_state.set_canonical_head(tip.clone());

        // Update metrics based on new tip
        self.metrics.tree.canonical_chain_height.set(tip.number() as f64);

        // sends an event to all active listeners about the new canonical chain
        self.canonical_in_memory_state.notify_canon_state(notification);

        // emit event
        self.emit_event(ConsensusEngineEvent::CanonicalChainCommitted(
            Box::new(tip),
            start.elapsed(),
        ));
    }

    /// This updates metrics based on the given reorg length.
    fn update_reorg_metrics(&self, old_chain_length: usize) {
        self.metrics.tree.reorgs.increment(1);
        self.metrics.tree.latest_reorg_depth.set(old_chain_length as f64);
    }

    /// This reinserts any blocks in the new chain that do not already exist in the tree
    fn reinsert_reorged_blocks(&mut self, new_chain: Vec<ExecutedBlock<N>>) {
        for block in new_chain {
            if self
                .state
                .tree_state
                .executed_block_by_hash(block.recovered_block().hash())
                .is_none()
            {
                trace!(target: "engine::tree", num=?block.recovered_block().number(), hash=?block.recovered_block().hash(), "Reinserting block into tree state");
                self.state.tree_state.insert_executed(block);
            }
        }
    }

    /// This handles downloaded blocks that are shown to be disconnected from the canonical chain.
    ///
    /// This mainly compares the missing parent of the downloaded block with the current canonical
    /// tip, and decides whether or not backfill sync should be triggered.
    fn on_disconnected_downloaded_block(
        &self,
        downloaded_block: BlockNumHash,
        missing_parent: BlockNumHash,
        head: BlockNumHash,
    ) -> Option<TreeEvent> {
        // compare the missing parent with the canonical tip
        if let Some(target) =
            self.backfill_sync_target(head.number, missing_parent.number, Some(downloaded_block))
        {
            trace!(target: "engine::tree", %target, "triggering backfill on downloaded block");
            return Some(TreeEvent::BackfillAction(BackfillAction::Start(target.into())));
        }

        // continue downloading the missing parent
        //
        // this happens if either:
        //  * the missing parent block num < canonical tip num
        //    * this case represents a missing block on a fork that is shorter than the canonical
        //      chain
        //  * the missing parent block num >= canonical tip num, but the number of missing blocks is
        //    less than the backfill threshold
        //    * this case represents a potentially long range of blocks to download and execute
        let request = if let Some(distance) =
            self.distance_from_local_tip(head.number, missing_parent.number)
        {
            trace!(target: "engine::tree", %distance, missing=?missing_parent, "downloading missing parent block range");
            DownloadRequest::BlockRange(missing_parent.hash, distance)
        } else {
            trace!(target: "engine::tree", missing=?missing_parent, "downloading missing parent block");
            // This happens when the missing parent is on an outdated
            // sidechain and we can only download the missing block itself
            DownloadRequest::single_block(missing_parent.hash)
        };

        Some(TreeEvent::Download(request))
    }

    /// Invoked with a block downloaded from the network
    ///
    /// Returns an event with the appropriate action to take, such as:
    ///  - download more missing blocks
    ///  - try to canonicalize the target if the `block` is the tracked target (head) block.
    #[instrument(level = "debug", target = "engine::tree", skip_all, fields(block_hash = %block.hash(), block_num = %block.number()))]
    fn on_downloaded_block(
        &mut self,
        block: SealedBlock<N::Block>,
    ) -> Result<Option<TreeEvent>, InsertBlockFatalError> {
        let block_num_hash = block.num_hash();
        let lowest_buffered_ancestor = self.lowest_buffered_ancestor_or(block_num_hash.hash);
        if self.check_invalid_ancestor_with_head(lowest_buffered_ancestor, &block)?.is_some() {
            return Ok(None)
        }

        if !self.backfill_sync_state.is_idle() {
            return Ok(None)
        }

        // try to append the block
        match self.insert_block(block) {
            Ok(InsertPayloadOk::Inserted(BlockStatus::Valid)) => {
                // check if we just inserted a block that's part of sync targets,
                // i.e. head, safe, or finalized
                if let Some(sync_target) = self.state.forkchoice_state_tracker.sync_target_state() &&
                    sync_target.contains(block_num_hash.hash)
                {
                    debug!(target: "engine::tree", ?sync_target, "appended downloaded sync target block");

                    // we just inserted a block that we know is part of the canonical chain, so we
                    // can make it canonical
                    return Ok(Some(TreeEvent::TreeAction(TreeAction::MakeCanonical {
                        sync_target_head: block_num_hash.hash,
                    })))
                }
                trace!(target: "engine::tree", "appended downloaded block");
                self.try_connect_buffered_blocks(block_num_hash)?;
            }
            Ok(InsertPayloadOk::Inserted(BlockStatus::Disconnected { head, missing_ancestor })) => {
                // block is not connected to the canonical head, we need to download
                // its missing branch first
                return Ok(self.on_disconnected_downloaded_block(
                    block_num_hash,
                    missing_ancestor,
                    head,
                ))
            }
            Ok(InsertPayloadOk::AlreadySeen(_)) => {
                trace!(target: "engine::tree", "downloaded block already executed");
            }
            Err(err) => {
                if let InsertPayloadError::Block(err) = err {
                    debug!(target: "engine::tree", err=%err.kind(), "failed to insert downloaded block");
                    if let Err(fatal) = self.on_insert_block_error(err) {
                        warn!(target: "engine::tree", %fatal, "fatal error occurred while inserting downloaded block");
                        return Err(fatal)
                    }
                }
            }
        }
        Ok(None)
    }

    /// Inserts a payload into the tree and executes it.
    ///
    /// This function validates the payload's basic structure, then executes it using the
    /// payload validator. The execution includes running all transactions in the payload
    /// and validating the resulting state transitions.
    ///
    /// Returns `InsertPayloadOk` if the payload was successfully inserted and executed,
    /// or `InsertPayloadError` if validation or execution failed.
    fn insert_payload(
        &mut self,
        payload: T::ExecutionData,
    ) -> Result<InsertPayloadOk, InsertPayloadError<N::Block>> {
        self.insert_block_or_payload(
            payload.block_with_parent(),
            payload,
            |validator, payload, ctx| validator.validate_payload(payload, ctx),
            |this, payload| Ok(this.payload_validator.convert_payload_to_block(payload)?),
        )
    }

    fn insert_block(
        &mut self,
        block: SealedBlock<N::Block>,
    ) -> Result<InsertPayloadOk, InsertPayloadError<N::Block>> {
        self.insert_block_or_payload(
            block.block_with_parent(),
            block,
            |validator, block, ctx| validator.validate_block(block, ctx),
            |_, block| Ok(block),
        )
    }

    /// Inserts a block or payload into the blockchain tree with full execution.
    ///
    /// This is a generic function that handles both blocks and payloads by accepting
    /// a block identifier, input data, and execution/validation functions. It performs
    /// comprehensive checks and execution:
    ///
    /// - Validates that the block doesn't already exist in the tree
    /// - Ensures parent state is available, buffering if necessary
    /// - Executes the block/payload using the provided execute function
    /// - Handles both canonical and fork chain insertions
    /// - Updates pending block state when appropriate
    /// - Emits consensus engine events and records metrics
    ///
    /// Returns `InsertPayloadOk::Inserted(BlockStatus::Valid)` on successful execution,
    /// `InsertPayloadOk::AlreadySeen` if the block already exists, or
    /// `InsertPayloadOk::Inserted(BlockStatus::Disconnected)` if parent state is missing.
    #[instrument(level = "debug", target = "engine::tree", skip_all, fields(block_id))]
    fn insert_block_or_payload<Input, Err>(
        &mut self,
        block_id: BlockWithParent,
        input: Input,
        execute: impl FnOnce(&mut V, Input, TreeCtx<'_, N>) -> Result<ExecutedBlock<N>, Err>,
        convert_to_block: impl FnOnce(&mut Self, Input) -> Result<SealedBlock<N::Block>, Err>,
    ) -> Result<InsertPayloadOk, Err>
    where
        Err: From<InsertBlockError<N::Block>>,
    {
        let block_insert_start = Instant::now();
        let block_num_hash = block_id.block;
        debug!(target: "engine::tree", block=?block_num_hash, parent = ?block_id.parent, "Inserting new block into tree");

        match self.sealed_header_by_hash(block_num_hash.hash) {
            Err(err) => {
                let block = convert_to_block(self, input)?;
                return Err(InsertBlockError::new(block, err.into()).into());
            }
            Ok(Some(_)) => {
                // We now assume that we already have this block in the tree. However, we need to
                // run the conversion to ensure that the block hash is valid.
                convert_to_block(self, input)?;
                return Ok(InsertPayloadOk::AlreadySeen(BlockStatus::Valid))
            }
            _ => {}
        };

        // Ensure that the parent state is available.
        match self.state_provider_builder(block_id.parent) {
            Err(err) => {
                let block = convert_to_block(self, input)?;
                return Err(InsertBlockError::new(block, err.into()).into());
            }
            Ok(None) => {
                let block = convert_to_block(self, input)?;

                // we don't have the state required to execute this block, buffering it and find the
                // missing parent block
                let missing_ancestor = self
                    .state
                    .buffer
                    .lowest_ancestor(&block.parent_hash())
                    .map(|block| block.parent_num_hash())
                    .unwrap_or_else(|| block.parent_num_hash());

                self.state.buffer.insert_block(block);

                return Ok(InsertPayloadOk::Inserted(BlockStatus::Disconnected {
                    head: self.state.tree_state.current_canonical_head,
                    missing_ancestor,
                }))
            }
            Ok(Some(_)) => {}
        }

        // determine whether we are on a fork chain by comparing the block number with the
        // canonical head. This is a simple check that is sufficient for the event emission below.
        // A block is considered a fork if its number is less than or equal to the canonical head,
        // as this indicates there's already a canonical block at that height.
        let is_fork = block_id.block.number <= self.state.tree_state.current_canonical_head.number;

        let ctx = TreeCtx::new(&mut self.state, &self.canonical_in_memory_state);

        let start = Instant::now();

        let executed = execute(&mut self.payload_validator, input, ctx)?;

        // if the parent is the canonical head, we can insert the block as the pending block
        if self.state.tree_state.canonical_block_hash() == executed.recovered_block().parent_hash()
        {
            debug!(target: "engine::tree", pending=?block_num_hash, "updating pending block");
            self.canonical_in_memory_state.set_pending_block(executed.clone());
        }

        self.state.tree_state.insert_executed(executed.clone());
        self.metrics.engine.executed_blocks.set(self.state.tree_state.block_count() as f64);

        // emit insert event
        let elapsed = start.elapsed();
        let engine_event = if is_fork {
            ConsensusEngineEvent::ForkBlockAdded(executed, elapsed)
        } else {
            ConsensusEngineEvent::CanonicalBlockAdded(executed, elapsed)
        };
        self.emit_event(EngineApiEvent::BeaconConsensus(engine_event));

        self.metrics
            .engine
            .block_insert_total_duration
            .record(block_insert_start.elapsed().as_secs_f64());
        debug!(target: "engine::tree", block=?block_num_hash, "Finished inserting block");
        Ok(InsertPayloadOk::Inserted(BlockStatus::Valid))
    }

    /// Handles an error that occurred while inserting a block.
    ///
    /// If this is a validation error this will mark the block as invalid.
    ///
    /// Returns the proper payload status response if the block is invalid.
    fn on_insert_block_error(
        &mut self,
        error: InsertBlockError<N::Block>,
    ) -> Result<PayloadStatus, InsertBlockFatalError> {
        let (block, error) = error.split();

        // if invalid block, we check the validation error. Otherwise return the fatal
        // error.
        let validation_err = error.ensure_validation_error()?;

        // If the error was due to an invalid payload, the payload is added to the
        // invalid headers cache and `Ok` with [PayloadStatusEnum::Invalid] is
        // returned.
        warn!(
            target: "engine::tree",
            invalid_hash=%block.hash(),
            invalid_number=block.number(),
            %validation_err,
            "Invalid block error on new payload",
        );
        let latest_valid_hash = self.latest_valid_hash_for_invalid_payload(block.parent_hash())?;

        // keep track of the invalid header
        self.state.invalid_headers.insert(block.block_with_parent());
        self.emit_event(EngineApiEvent::BeaconConsensus(ConsensusEngineEvent::InvalidBlock(
            Box::new(block),
        )));

        Ok(PayloadStatus::new(
            PayloadStatusEnum::Invalid { validation_error: validation_err.to_string() },
            latest_valid_hash,
        ))
    }

    /// Handles a [`NewPayloadError`] by converting it to a [`PayloadStatus`].
    fn on_new_payload_error(
        &mut self,
        error: NewPayloadError,
        payload_num_hash: NumHash,
        parent_hash: B256,
    ) -> ProviderResult<PayloadStatus> {
        error!(target: "engine::tree", payload=?payload_num_hash, %error, "Invalid payload");
        // we need to convert the error to a payload status (response to the CL)

        let latest_valid_hash =
            if error.is_block_hash_mismatch() || error.is_invalid_versioned_hashes() {
                // Engine-API rules:
                // > `latestValidHash: null` if the blockHash validation has failed (<https://github.com/ethereum/execution-apis/blob/fe8e13c288c592ec154ce25c534e26cb7ce0530d/src/engine/shanghai.md?plain=1#L113>)
                // > `latestValidHash: null` if the expected and the actual arrays don't match (<https://github.com/ethereum/execution-apis/blob/fe8e13c288c592ec154ce25c534e26cb7ce0530d/src/engine/cancun.md?plain=1#L103>)
                None
            } else {
                self.latest_valid_hash_for_invalid_payload(parent_hash)?
            };

        let status = PayloadStatusEnum::from(error);
        Ok(PayloadStatus::new(status, latest_valid_hash))
    }

    /// Attempts to find the header for the given block hash if it is canonical.
    pub fn find_canonical_header(
        &self,
        hash: B256,
    ) -> Result<Option<SealedHeader<N::BlockHeader>>, ProviderError> {
        let mut canonical = self.canonical_in_memory_state.header_by_hash(hash);

        if canonical.is_none() {
            canonical = self.provider.header(hash)?.map(|header| SealedHeader::new(header, hash));
        }

        Ok(canonical)
    }

    /// Updates the tracked finalized block if we have it.
    fn update_finalized_block(
        &self,
        finalized_block_hash: B256,
    ) -> Result<(), OnForkChoiceUpdated> {
        if finalized_block_hash.is_zero() {
            return Ok(())
        }

        match self.find_canonical_header(finalized_block_hash) {
            Ok(None) => {
                debug!(target: "engine::tree", "Finalized block not found in canonical chain");
                // if the finalized block is not known, we can't update the finalized block
                return Err(OnForkChoiceUpdated::invalid_state())
            }
            Ok(Some(finalized)) => {
                if Some(finalized.num_hash()) !=
                    self.canonical_in_memory_state.get_finalized_num_hash()
                {
                    // we're also persisting the finalized block on disk so we can reload it on
                    // restart this is required by optimism which queries the finalized block: <https://github.com/ethereum-optimism/optimism/blob/c383eb880f307caa3ca41010ec10f30f08396b2e/op-node/rollup/sync/start.go#L65-L65>
                    let _ = self.persistence.save_finalized_block_number(finalized.number());
                    self.canonical_in_memory_state.set_finalized(finalized.clone());
                    // Update finalized block height metric
                    self.metrics.tree.finalized_block_height.set(finalized.number() as f64);
                }
            }
            Err(err) => {
                error!(target: "engine::tree", %err, "Failed to fetch finalized block header");
            }
        }

        Ok(())
    }

    /// Updates the tracked safe block if we have it
    fn update_safe_block(&self, safe_block_hash: B256) -> Result<(), OnForkChoiceUpdated> {
        if safe_block_hash.is_zero() {
            return Ok(())
        }

        match self.find_canonical_header(safe_block_hash) {
            Ok(None) => {
                debug!(target: "engine::tree", "Safe block not found in canonical chain");
                // if the safe block is not known, we can't update the safe block
                return Err(OnForkChoiceUpdated::invalid_state())
            }
            Ok(Some(safe)) => {
                if Some(safe.num_hash()) != self.canonical_in_memory_state.get_safe_num_hash() {
                    // we're also persisting the safe block on disk so we can reload it on
                    // restart this is required by optimism which queries the safe block: <https://github.com/ethereum-optimism/optimism/blob/c383eb880f307caa3ca41010ec10f30f08396b2e/op-node/rollup/sync/start.go#L65-L65>
                    let _ = self.persistence.save_safe_block_number(safe.number());
                    self.canonical_in_memory_state.set_safe(safe.clone());
                    // Update safe block height metric
                    self.metrics.tree.safe_block_height.set(safe.number() as f64);
                }
            }
            Err(err) => {
                error!(target: "engine::tree", %err, "Failed to fetch safe block header");
            }
        }

        Ok(())
    }

    /// Ensures that the given forkchoice state is consistent, assuming the head block has been
    /// made canonical.
    ///
    /// If the forkchoice state is consistent, this will return Ok(()). Otherwise, this will
    /// return an instance of [`OnForkChoiceUpdated`] that is INVALID.
    ///
    /// This also updates the safe and finalized blocks in the [`CanonicalInMemoryState`], if they
    /// are consistent with the head block.
    fn ensure_consistent_forkchoice_state(
        &self,
        state: ForkchoiceState,
    ) -> Result<(), OnForkChoiceUpdated> {
        // Ensure that the finalized block, if not zero, is known and in the canonical chain
        // after the head block is canonicalized.
        //
        // This ensures that the finalized block is consistent with the head block, i.e. the
        // finalized block is an ancestor of the head block.
        self.update_finalized_block(state.finalized_block_hash)?;

        // Also ensure that the safe block, if not zero, is known and in the canonical chain
        // after the head block is canonicalized.
        //
        // This ensures that the safe block is consistent with the head block, i.e. the safe
        // block is an ancestor of the head block.
        self.update_safe_block(state.safe_block_hash)
    }

    /// Validates the payload attributes with respect to the header and fork choice state.
    ///
    /// Note: At this point, the fork choice update is considered to be VALID, however, we can still
    /// return an error if the payload attributes are invalid.
    fn process_payload_attributes(
        &self,
        attrs: T::PayloadAttributes,
        head: &N::BlockHeader,
        state: ForkchoiceState,
        version: EngineApiMessageVersion,
    ) -> OnForkChoiceUpdated {
        if let Err(err) =
            self.payload_validator.validate_payload_attributes_against_header(&attrs, head)
        {
            warn!(target: "engine::tree", %err, ?head, "Invalid payload attributes");
            return OnForkChoiceUpdated::invalid_payload_attributes()
        }

        // 8. Client software MUST begin a payload build process building on top of
        //    forkchoiceState.headBlockHash and identified via buildProcessId value if
        //    payloadAttributes is not null and the forkchoice state has been updated successfully.
        //    The build process is specified in the Payload building section.
        match <T::PayloadBuilderAttributes as PayloadBuilderAttributes>::try_new(
            state.head_block_hash,
            attrs,
            version as u8,
        ) {
            Ok(attributes) => {
                // send the payload to the builder and return the receiver for the pending payload
                // id, initiating payload job is handled asynchronously
                let pending_payload_id = self.payload_builder.send_new_payload(attributes);

                // Client software MUST respond to this method call in the following way:
                // {
                //      payloadStatus: {
                //          status: VALID,
                //          latestValidHash: forkchoiceState.headBlockHash,
                //          validationError: null
                //      },
                //      payloadId: buildProcessId
                // }
                //
                // if the payload is deemed VALID and the build process has begun.
                OnForkChoiceUpdated::updated_with_pending_payload_id(
                    PayloadStatus::new(PayloadStatusEnum::Valid, Some(state.head_block_hash)),
                    pending_payload_id,
                )
            }
            Err(_) => OnForkChoiceUpdated::invalid_payload_attributes(),
        }
    }

    /// Remove all blocks up to __and including__ the given block number.
    ///
    /// If a finalized hash is provided, the only non-canonical blocks which will be removed are
    /// those which have a fork point at or below the finalized hash.
    ///
    /// Canonical blocks below the upper bound will still be removed.
    pub(crate) fn remove_before(
        &mut self,
        upper_bound: BlockNumHash,
        finalized_hash: Option<B256>,
    ) -> ProviderResult<()> {
        // first fetch the finalized block number and then call the remove_before method on
        // tree_state
        let num = if let Some(hash) = finalized_hash {
            self.provider.block_number(hash)?.map(|number| BlockNumHash { number, hash })
        } else {
            None
        };

        self.state.tree_state.remove_until(
            upper_bound,
            self.persistence_state.last_persisted_block.hash,
            num,
        );
        Ok(())
    }

    /// Returns a builder for creating state providers for the given hash.
    ///
    /// This is an optimization for parallel execution contexts where we want to avoid
    /// creating state providers in the critical path.
    pub fn state_provider_builder(
        &self,
        hash: B256,
    ) -> ProviderResult<Option<StateProviderBuilder<N, P>>>
    where
        P: BlockReader + StateProviderFactory + StateReader + Clone,
    {
        if let Some((historical, blocks)) = self.state.tree_state.blocks_by_hash(hash) {
            debug!(target: "engine::tree", %hash, %historical, "found canonical state for block in memory, creating provider builder");
            // the block leads back to the canonical chain
            return Ok(Some(StateProviderBuilder::new(
                self.provider.clone(),
                historical,
                Some(blocks),
            )))
        }

        // Check if the block is persisted
        if let Some(header) = self.provider.header(hash)? {
            debug!(target: "engine::tree", %hash, number = %header.number(), "found canonical state for block in database, creating provider builder");
            // For persisted blocks, we create a builder that will fetch state directly from the
            // database
            return Ok(Some(StateProviderBuilder::new(self.provider.clone(), hash, None)))
        }

        debug!(target: "engine::tree", %hash, "no canonical state found for block");
        Ok(None)
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
        /// The result of the persistence operation.
        result: Option<BlockNumHash>,
        /// When the persistence operation started.
        start_time: Instant,
    },
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
    /// The block is valid and block extends canonical chain.
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

/// Target for block persistence.
#[derive(Debug, Clone, Copy)]
enum PersistTarget {
    /// Persist up to `canonical_head - memory_block_buffer_target`.
    Threshold,
    /// Persist all blocks up to and including the canonical head.
    Head,
}
