use crate::{
    backfill::BackfillAction,
    chain::FromOrchestrator,
    engine::{DownloadRequest, EngineApiEvent, FromEngine},
    persistence::PersistenceHandle,
};
pub use memory_overlay::MemoryOverlayStateProvider;
use reth_beacon_consensus::{
    BeaconConsensusEngineEvent, BeaconEngineMessage, ForkchoiceStateTracker, InvalidHeaderCache,
    OnForkChoiceUpdated, MIN_BLOCKS_FOR_PIPELINE_RUN,
};
use reth_blockchain_tree::{
    error::InsertBlockErrorKind, BlockAttachment, BlockBuffer, BlockStatus,
};
use reth_blockchain_tree_api::{error::InsertBlockError, InsertPayloadOk};
use reth_chain_state::{CanonicalInMemoryState, ExecutedBlock};
use reth_consensus::{Consensus, PostExecutionInput};
use reth_engine_primitives::EngineTypes;
use reth_errors::{ConsensusError, ProviderResult};
use reth_evm::execute::{BlockExecutorProvider, Executor};
use reth_payload_builder::PayloadBuilderHandle;
use reth_payload_primitives::{PayloadAttributes, PayloadBuilderAttributes, PayloadTypes};
use reth_payload_validator::ExecutionPayloadValidator;
use reth_primitives::{
    Block, BlockNumHash, BlockNumber, GotExpected, Header, Receipts, Requests, SealedBlock,
    SealedBlockWithSenders, SealedHeader, B256, U256,
};
use reth_provider::{
    BlockReader, CanonStateNotification, Chain, ExecutionOutcome, StateProvider,
    StateProviderFactory, StateRootProvider,
};
use reth_revm::database::StateProviderDatabase;
use reth_rpc_types::{
    engine::{
        CancunPayloadFields, ForkchoiceState, PayloadStatus, PayloadStatusEnum,
        PayloadValidationError,
    },
    ExecutionPayload,
};
use reth_stages_api::ControlFlow;
use reth_trie::HashedPostState;
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    ops::Deref,
    sync::{mpsc::Receiver, Arc},
};
use tokio::sync::{
    mpsc::{UnboundedReceiver, UnboundedSender},
    oneshot,
};
use tracing::*;

mod memory_overlay;

/// Maximum number of blocks to be kept only in memory without triggering persistence.
const PERSISTENCE_THRESHOLD: u64 = 256;
/// Number of pending blocks that cannot be executed due to missing parent and
/// are kept in cache.
const DEFAULT_BLOCK_BUFFER_LIMIT: u32 = 256;
/// Number of invalid headers to keep in cache.
const DEFAULT_MAX_INVALID_HEADER_CACHE_LENGTH: u32 = 256;

/// Keeps track of the state of the tree.
///
/// ## Invariants
///
/// - This only stores blocks that are connected to the canonical chain.
/// - All executed blocks are valid and have been executed.
#[derive(Debug, Default)]
pub struct TreeState {
    /// __All__ executed blocks by block hash.
    ///
    /// This includes blocks of all forks.
    blocks_by_hash: HashMap<B256, ExecutedBlock>,
    /// Executed blocks grouped by their respective block number.
    ///
    /// This maps unique block number to all known blocks for that height.
    blocks_by_number: BTreeMap<BlockNumber, Vec<ExecutedBlock>>,
    /// Currently tracked canonical head of the chain.
    current_canonical_head: BlockNumHash,
}

impl TreeState {
    /// Returns a new, empty tree state that points to the given canonical head.
    fn new(current_canonical_head: BlockNumHash) -> Self {
        Self {
            blocks_by_hash: HashMap::new(),
            blocks_by_number: BTreeMap::new(),
            current_canonical_head,
        }
    }

    /// Returns the block by hash.
    fn block_by_hash(&self, hash: B256) -> Option<Arc<SealedBlock>> {
        self.blocks_by_hash.get(&hash).map(|b| b.block.clone())
    }

    fn block_by_number(&self, number: BlockNumber) -> Option<Arc<SealedBlock>> {
        self.blocks_by_number
            .get(&number)
            .and_then(|blocks| blocks.last())
            .map(|executed_block| executed_block.block.clone())
    }

    /// Insert executed block into the state.
    fn insert_executed(&mut self, executed: ExecutedBlock) {
        self.blocks_by_number.entry(executed.block.number).or_default().push(executed.clone());
        let existing = self.blocks_by_hash.insert(executed.block.hash(), executed);
        debug_assert!(existing.is_none(), "inserted duplicate block");
    }

    /// Remove blocks before specified block number.
    pub(crate) fn remove_before(&mut self, block_number: BlockNumber) {
        while self
            .blocks_by_number
            .first_key_value()
            .map(|entry| entry.0 < &block_number)
            .unwrap_or_default()
        {
            let (_, to_remove) = self.blocks_by_number.pop_first().unwrap();
            for block in to_remove {
                let block_hash = block.block.hash();
                let removed = self.blocks_by_hash.remove(&block_hash);
                debug_assert!(
                    removed.is_some(),
                    "attempted to remove non-existing block {block_hash}"
                );
            }
        }
    }

    /// Returns the maximum block number stored.
    pub(crate) fn max_block_number(&self) -> BlockNumber {
        *self.blocks_by_number.last_key_value().unwrap_or((&BlockNumber::default(), &vec![])).0
    }

    /// Returns the block number of the pending block: `head + 1`
    const fn pending_block_number(&self) -> BlockNumber {
        self.current_canonical_head.number + 1
    }

    /// Updates the canonical head to the given block.
    fn set_canonical_head(&mut self, new_head: BlockNumHash) {
        self.current_canonical_head = new_head;
    }

    /// Returns the tracked canonical head.
    const fn canonical_head(&self) -> &BlockNumHash {
        &self.current_canonical_head
    }

    /// Returns the block hash of the canonical head.
    const fn canonical_block_hash(&self) -> B256 {
        self.canonical_head().hash
    }

    /// Returns the new chain for the given head.
    ///
    /// This also handles reorgs.
    fn on_new_head(&self, new_head: B256) -> Option<NewCanonicalChain> {
        let new_head_block = self.blocks_by_hash.get(&new_head).cloned()?;
        let mut parent = new_head_block.block.num_hash();
        let mut new_chain = vec![new_head_block];
        let mut reorged = vec![];

        // walk back the chain until we reach the canonical block
        while parent.hash != self.canonical_block_hash() {
            if parent.number == self.canonical_head().number {
                // we have a reorg
                todo!("handle reorg")
            }
            let parent_block = self.blocks_by_hash.get(&new_head).cloned()?;
            parent = parent_block.block.num_hash();
            new_chain.push(parent_block);
        }

        // reverse the chains
        new_chain.reverse();
        reorged.reverse();

        let chain = if reorged.is_empty() {
            NewCanonicalChain::Commit { new: new_chain }
        } else {
            NewCanonicalChain::Reorg { new: new_chain, old: reorged }
        };

        Some(chain)
    }
}

/// Non-empty chain of blocks.
enum NewCanonicalChain {
    /// A simple append to the current canonical head
    Commit {
        /// all blocks that lead back to the canonical head
        new: Vec<ExecutedBlock>,
    },
    /// A reorged chain consists of two chains that trace back to a shared ancestor block at which
    /// point they diverge.
    Reorg {
        /// All blocks of the _new_ chain
        new: Vec<ExecutedBlock>,
        /// All blocks of the _old_ chain
        old: Vec<ExecutedBlock>,
    },
}

impl NewCanonicalChain {
    /// Converts the new chain into a notification that will be emitted to listeners
    fn to_chain_notification(&self) -> CanonStateNotification {
        // TODO: do we need to merge execution outcome for multiblock commit or reorg?
        //  implement this properly
        match self {
            Self::Commit { new } => CanonStateNotification::Commit {
                new: Arc::new(Chain::new(
                    new.iter().map(ExecutedBlock::sealed_block_with_senders),
                    new.last().unwrap().execution_output.deref().clone(),
                    None,
                )),
            },
            Self::Reorg { new, old } => CanonStateNotification::Reorg {
                new: Arc::new(Chain::new(
                    new.iter().map(ExecutedBlock::sealed_block_with_senders),
                    new.last().unwrap().execution_output.deref().clone(),
                    None,
                )),
                old: Arc::new(Chain::new(
                    old.iter().map(ExecutedBlock::sealed_block_with_senders),
                    old.last().unwrap().execution_output.deref().clone(),
                    None,
                )),
            },
        }
    }

    /// Returns the new tip of the chain.
    ///
    /// Returns the new tip for [`Self::Reorg`] and [`Self::Commit`] variants which commit at least
    /// 1 new block.
    fn tip(&self) -> &SealedBlock {
        match self {
            Self::Commit { new } | Self::Reorg { new, .. } => new.last().unwrap().block(),
        }
    }
}

/// Tracks the state of the engine api internals.
///
/// This type is not shareable.
#[derive(Debug)]
pub struct EngineApiTreeState {
    /// Tracks the state of the blockchain tree.
    tree_state: TreeState,
    /// Tracks the forkchoice state updates received by the CL.
    forkchoice_state_tracker: ForkchoiceStateTracker,
    /// Buffer of detached blocks.
    buffer: BlockBuffer,
    /// Tracks the header of invalid payloads that were rejected by the engine because they're
    /// invalid.
    invalid_headers: InvalidHeaderCache,
}

impl EngineApiTreeState {
    fn new(
        block_buffer_limit: u32,
        max_invalid_header_cache_length: u32,
        canonical_block: BlockNumHash,
    ) -> Self {
        Self {
            invalid_headers: InvalidHeaderCache::new(max_invalid_header_cache_length),
            buffer: BlockBuffer::new(block_buffer_limit),
            tree_state: TreeState::new(canonical_block),
            forkchoice_state_tracker: ForkchoiceStateTracker::default(),
        }
    }
}

/// The type responsible for processing engine API requests.
///
/// TODO: design: should the engine handler functions also accept the response channel or return the
/// result and the caller redirects the response
pub trait EngineApiTreeHandler {
    /// The engine type that this handler is for.
    type Engine: EngineTypes;

    /// Invoked when previously requested blocks were downloaded.
    fn on_downloaded(&mut self, blocks: Vec<SealedBlockWithSenders>) -> Option<TreeEvent>;

    /// When the Consensus layer receives a new block via the consensus gossip protocol,
    /// the transactions in the block are sent to the execution layer in the form of a
    /// [`ExecutionPayload`]. The Execution layer executes the transactions and validates the
    /// state in the block header, then passes validation data back to Consensus layer, that
    /// adds the block to the head of its own blockchain and attests to it. The block is then
    /// broadcast over the consensus p2p network in the form of a "Beacon block".
    ///
    /// These responses should adhere to the [Engine API Spec for
    /// `engine_newPayload`](https://github.com/ethereum/execution-apis/blob/main/src/engine/paris.md#specification).
    ///
    /// This returns a [`PayloadStatus`] that represents the outcome of a processed new payload and
    /// returns an error if an internal error occurred.
    fn on_new_payload(
        &mut self,
        payload: ExecutionPayload,
        cancun_fields: Option<CancunPayloadFields>,
    ) -> ProviderResult<TreeOutcome<PayloadStatus>>;

    /// Invoked when we receive a new forkchoice update message. Calls into the blockchain tree
    /// to resolve chain forks and ensure that the Execution Layer is working with the latest valid
    /// chain.
    ///
    /// These responses should adhere to the [Engine API Spec for
    /// `engine_forkchoiceUpdated`](https://github.com/ethereum/execution-apis/blob/main/src/engine/paris.md#specification-1).
    ///
    /// Returns an error if an internal error occurred like a database error.
    fn on_forkchoice_updated(
        &mut self,
        state: ForkchoiceState,
        attrs: Option<<Self::Engine as PayloadTypes>::PayloadAttributes>,
    ) -> ProviderResult<TreeOutcome<OnForkChoiceUpdated>>;
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

/// Events that can be emitted by the [`EngineApiTreeHandler`].
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
    MakeCanonical(B256),
}

/// The engine API tree handler implementation.
///
/// This type is responsible for processing engine API requests, maintaining the canonical state and
/// emitting events.
#[derive(Debug)]
pub struct EngineApiTreeHandlerImpl<P, E, T: EngineTypes> {
    provider: P,
    executor_provider: E,
    consensus: Arc<dyn Consensus>,
    payload_validator: ExecutionPayloadValidator,
    /// Keeps track of internals such as executed and buffered blocks.
    state: EngineApiTreeState,
    /// Incoming engine API requests.
    incoming: Receiver<FromEngine<BeaconEngineMessage<T>>>,
    /// Outgoing events that are emitted to the handler.
    outgoing: UnboundedSender<EngineApiEvent>,
    /// Channels to the persistence layer.
    persistence: PersistenceHandle,
    /// Tracks the state changes of the persistence task.
    persistence_state: PersistenceState,
    /// Flag indicating whether the node is currently syncing via backfill.
    is_backfill_active: bool,
    /// Keeps track of the state of the canonical chain that isn't persisted yet.
    /// This is intended to be accessed from external sources, such as rpc.
    canonical_in_memory_state: CanonicalInMemoryState,
    /// Handle to the payload builder that will receive payload attributes for valid forkchoice
    /// updates
    payload_builder: PayloadBuilderHandle<T>,
}

impl<P, E, T> EngineApiTreeHandlerImpl<P, E, T>
where
    P: BlockReader + StateProviderFactory + Clone + 'static,
    E: BlockExecutorProvider,
    T: EngineTypes,
{
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        provider: P,
        executor_provider: E,
        consensus: Arc<dyn Consensus>,
        payload_validator: ExecutionPayloadValidator,
        incoming: Receiver<FromEngine<BeaconEngineMessage<T>>>,
        outgoing: UnboundedSender<EngineApiEvent>,
        state: EngineApiTreeState,
        header: SealedHeader,
        persistence: PersistenceHandle,
        payload_builder: PayloadBuilderHandle<T>,
    ) -> Self {
        Self {
            provider,
            executor_provider,
            consensus,
            payload_validator,
            incoming,
            outgoing,
            persistence,
            persistence_state: PersistenceState::default(),
            is_backfill_active: false,
            state,
            canonical_in_memory_state: CanonicalInMemoryState::with_head(header),
            payload_builder,
        }
    }

    /// Creates a new `EngineApiTreeHandlerImpl` instance and spawns it in its
    /// own thread. Returns the receiver end of a `EngineApiEvent` unbounded
    /// channel to receive events from the engine.
    #[allow(clippy::too_many_arguments)]
    pub fn spawn_new(
        provider: P,
        executor_provider: E,
        consensus: Arc<dyn Consensus>,
        payload_validator: ExecutionPayloadValidator,
        incoming: Receiver<FromEngine<BeaconEngineMessage<T>>>,
        persistence: PersistenceHandle,
        payload_builder: PayloadBuilderHandle<T>,
    ) -> UnboundedReceiver<EngineApiEvent> {
        let best_block_number = provider.best_block_number().unwrap_or(0);
        let header = provider.sealed_header(best_block_number).ok().flatten().unwrap_or_default();

        let (tx, outgoing) = tokio::sync::mpsc::unbounded_channel();
        let state = EngineApiTreeState::new(
            DEFAULT_BLOCK_BUFFER_LIMIT,
            DEFAULT_MAX_INVALID_HEADER_CACHE_LENGTH,
            header.num_hash(),
        );

        let task = Self::new(
            provider,
            executor_provider,
            consensus,
            payload_validator,
            incoming,
            tx,
            state,
            header,
            persistence,
            payload_builder,
        );
        std::thread::Builder::new().name("Tree Task".to_string()).spawn(|| task.run()).unwrap();
        outgoing
    }

    /// Run the engine API handler.
    ///
    /// This will block the current thread and process incoming messages.
    pub fn run(mut self) {
        while let Ok(msg) = self.incoming.recv() {
            self.run_once(msg);
        }
    }

    /// Run the engine API handler once.
    fn run_once(&mut self, msg: FromEngine<BeaconEngineMessage<T>>) {
        self.on_engine_message(msg);

        if self.should_persist() && !self.persistence_state.in_progress() {
            let blocks_to_persist = self.get_blocks_to_persist();
            let (tx, rx) = oneshot::channel();
            self.persistence.save_blocks(blocks_to_persist, tx);
            self.persistence_state.start(rx);
        }

        if self.persistence_state.in_progress() {
            let rx = self
                .persistence_state
                .rx
                .as_mut()
                .expect("if a persistence task is in progress Receiver must be Some");
            // Check if persistence has completed
            if let Ok(last_persisted_block_hash) = rx.try_recv() {
                if let Some(block) = self.state.tree_state.block_by_hash(last_persisted_block_hash)
                {
                    self.persistence_state.finish(last_persisted_block_hash, block.number);
                    self.remove_persisted_blocks_from_memory();
                } else {
                    error!("could not find persisted block with hash {last_persisted_block_hash} in memory");
                }
            }
        }
    }

    /// Handles a message from the engine.
    fn on_engine_message(&mut self, msg: FromEngine<BeaconEngineMessage<T>>) {
        match msg {
            FromEngine::Event(event) => match event {
                FromOrchestrator::BackfillSyncStarted => {
                    debug!(target: "consensus::engine", "received backfill sync started event");
                    self.is_backfill_active = true;
                }
                FromOrchestrator::BackfillSyncFinished(ctrl) => {
                    self.on_backfill_sync_finished(ctrl);
                }
            },
            FromEngine::Request(request) => match request {
                BeaconEngineMessage::ForkchoiceUpdated { state, payload_attrs, tx } => {
                    let mut output = self.on_forkchoice_updated(state, payload_attrs);

                    if let Ok(res) = &mut output {
                        // emit an event about the handled FCU
                        self.emit_event(BeaconConsensusEngineEvent::ForkchoiceUpdated(
                            state,
                            res.outcome.forkchoice_status(),
                        ));

                        // handle the event if any
                        self.on_maybe_tree_event(res.event.take());
                    }

                    if let Err(err) = tx.send(output.map(|o| o.outcome).map_err(Into::into)) {
                        error!("Failed to send event: {err:?}");
                    }
                }
                BeaconEngineMessage::NewPayload { payload, cancun_fields, tx } => {
                    let output = self.on_new_payload(payload, cancun_fields);
                    if let Err(err) = tx.send(output.map(|o| o.outcome).map_err(|e| {
                        reth_beacon_consensus::BeaconOnNewPayloadError::Internal(Box::new(e))
                    })) {
                        error!("Failed to send event: {err:?}");
                    }
                }
                BeaconEngineMessage::TransitionConfigurationExchanged => {
                    // this is a reporting no-op because the engine API impl does not need
                    // additional input to handle this request
                }
            },
            FromEngine::DownloadedBlocks(blocks) => {
                if let Some(event) = self.on_downloaded(blocks) {
                    self.on_tree_event(event);
                }
            }
        }
    }

    /// Invoked if the backfill sync has finished to target.
    ///
    /// Checks the tracked finalized block against the block on disk and restarts backfill if
    /// needed.
    ///
    /// This will also try to connect the buffered blocks.
    fn on_backfill_sync_finished(&mut self, ctrl: ControlFlow) {
        debug!(target: "consensus::engine", "received backfill sync finished event");
        self.is_backfill_active = false;

        // Pipeline unwound, memorize the invalid block and wait for CL for next sync target.
        if let ControlFlow::Unwind { bad_block, .. } = ctrl {
            warn!(target: "consensus::engine", invalid_hash=?bad_block.hash(), invalid_number=?bad_block.number, "Bad block detected in unwind");
            // update the `invalid_headers` cache with the new invalid header
            self.state.invalid_headers.insert(*bad_block);
            return
        }

        let Some(sync_target_state) = self.state.forkchoice_state_tracker.sync_target_state()
        else {
            return
        };

        if sync_target_state.finalized_block_hash.is_zero() {
            return
        }

        // get the block number of the finalized block, if we have it
        let newest_finalized = self
            .state
            .buffer
            .block(&sync_target_state.finalized_block_hash)
            .map(|block| block.number);

        // TODO(mattsse): state housekeeping, this needs to update the tracked canonical state and
        // attempt to make the current target canonical if we have all the blocks buffered

        // The block number that the backfill finished at - if the progress or newest
        // finalized is None then we can't check the distance anyways.
        //
        // If both are Some, we perform another distance check and return the desired
        // backfill target
        let Some(backfill_target) =
            ctrl.block_number().zip(newest_finalized).and_then(|(progress, finalized_number)| {
                // Determines whether or not we should run backfill again, in case
                // the new gap is still large enough and requires running backfill again
                self.backfill_sync_target(progress, finalized_number, None)
            })
        else {
            return
        };

        // request another backfill run
        self.emit_event(EngineApiEvent::BackfillAction(BackfillAction::Start(
            backfill_target.into(),
        )));
    }

    /// Attempts to make the given target canonical.
    ///
    /// This will update the tracked canonical in memory state and do the necessary housekeeping.
    const fn make_canonical(&self, target: B256) {
        // TODO: implement state updates and shift canonical state
    }

    /// Convenience function to handle an optional tree event.
    fn on_maybe_tree_event(&self, event: Option<TreeEvent>) {
        if let Some(event) = event {
            self.on_tree_event(event);
        }
    }

    /// Handles a tree event.
    fn on_tree_event(&self, event: TreeEvent) {
        match event {
            TreeEvent::TreeAction(action) => match action {
                TreeAction::MakeCanonical(target) => {
                    self.make_canonical(target);
                }
            },
            TreeEvent::BackfillAction(action) => {
                self.emit_event(EngineApiEvent::BackfillAction(action));
            }
            TreeEvent::Download(action) => {
                self.emit_event(EngineApiEvent::Download(action));
            }
        }
    }

    /// Emits an outgoing event to the engine.
    fn emit_event(&self, event: impl Into<EngineApiEvent>) {
        let _ = self
            .outgoing
            .send(event.into())
            .inspect_err(|err| error!("Failed to send internal event: {err:?}"));
    }

    /// Returns true if the canonical chain length minus the last persisted
    /// block is greater than or equal to the persistence threshold.
    fn should_persist(&self) -> bool {
        self.state.tree_state.max_block_number() -
            self.persistence_state.last_persisted_block_number >=
            PERSISTENCE_THRESHOLD
    }

    fn get_blocks_to_persist(&self) -> Vec<ExecutedBlock> {
        let start = self.persistence_state.last_persisted_block_number;
        let end = start + PERSISTENCE_THRESHOLD;

        // NOTE: this is an exclusive range, to try to include exactly PERSISTENCE_THRESHOLD blocks
        self.state
            .tree_state
            .blocks_by_number
            .range(start..end)
            .flat_map(|(_, blocks)| blocks.iter().cloned())
            .collect()
    }

    fn remove_persisted_blocks_from_memory(&mut self) {
        let keys_to_remove: Vec<BlockNumber> = self
            .state
            .tree_state
            .blocks_by_number
            .range(..=self.persistence_state.last_persisted_block_number)
            .map(|(&k, _)| k)
            .collect();

        for key in keys_to_remove {
            if let Some(blocks) = self.state.tree_state.blocks_by_number.remove(&key) {
                // Remove corresponding blocks from blocks_by_hash
                for block in blocks {
                    self.state.tree_state.blocks_by_hash.remove(&block.block().hash());
                }
            }
        }
    }

    /// Return block from database or in-memory state by hash.
    fn block_by_hash(&self, hash: B256) -> ProviderResult<Option<Block>> {
        // check database first
        let mut block = self.provider.block_by_hash(hash)?;
        if block.is_none() {
            // Note: it's fine to return the unsealed block because the caller already has
            // the hash
            block = self
                .state
                .tree_state
                .block_by_hash(hash)
                // TODO: clone for compatibility. should we return an Arc here?
                .map(|block| block.as_ref().clone().unseal());
        }
        Ok(block)
    }

    /// Return state provider with reference to in-memory blocks that overlay database state.
    ///
    /// This merges the state of all blocks that are part of the chain that the requested block is
    /// the head of. This includes all blocks that connect back to the canonical block on disk.
    // TODO: return error if the chain has gaps
    fn state_provider(
        &self,
        hash: B256,
    ) -> ProviderResult<MemoryOverlayStateProvider<Box<dyn StateProvider>>> {
        let mut in_memory = Vec::new();
        let mut parent_hash = hash;
        while let Some(executed) = self.state.tree_state.blocks_by_hash.get(&parent_hash) {
            parent_hash = executed.block.parent_hash;
            in_memory.insert(0, executed.clone());
        }

        let historical = self.provider.state_by_block_hash(parent_hash)?;
        Ok(MemoryOverlayStateProvider::new(in_memory, historical))
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
            .map(|block| block.parent_hash)
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
        if self.block_by_hash(parent_hash)?.is_some() {
            return Ok(Some(parent_hash))
        }

        // iterate over ancestors in the invalid cache
        // until we encounter the first valid ancestor
        let mut current_hash = parent_hash;
        let mut current_header = self.state.invalid_headers.get(&current_hash);
        while let Some(header) = current_header {
            current_hash = header.parent_hash;
            current_header = self.state.invalid_headers.get(&current_hash);

            // If current_header is None, then the current_hash does not have an invalid
            // ancestor in the cache, check its presence in blockchain tree
            if current_header.is_none() && self.block_by_hash(current_hash)?.is_some() {
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
        if let Some(parent) = self.block_by_hash(parent_hash)? {
            if !parent.is_zero_difficulty() {
                parent_hash = B256::ZERO;
            }
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

    /// Checks if the given `check` hash points to an invalid header, inserting the given `head`
    /// block into the invalid header cache if the `check` hash has a known invalid ancestor.
    ///
    /// Returns a payload status response according to the engine API spec if the block is known to
    /// be invalid.
    fn check_invalid_ancestor_with_head(
        &mut self,
        check: B256,
        head: B256,
    ) -> ProviderResult<Option<PayloadStatus>> {
        // check if the check hash was previously marked as invalid
        let Some(header) = self.state.invalid_headers.get(&check) else { return Ok(None) };

        // populate the latest valid hash field
        let status = self.prepare_invalid_response(header.parent_hash)?;

        // insert the head block into the invalid header cache
        self.state.invalid_headers.insert_with_invalid_ancestor(head, header);

        Ok(Some(status))
    }

    /// Checks if the given `head` points to an invalid header, which requires a specific response
    /// to a forkchoice update.
    fn check_invalid_ancestor(&mut self, head: B256) -> ProviderResult<Option<PayloadStatus>> {
        // check if the head was previously marked as invalid
        let Some(header) = self.state.invalid_headers.get(&head) else { return Ok(None) };
        // populate the latest valid hash field
        Ok(Some(self.prepare_invalid_response(header.parent_hash)?))
    }

    /// Validate if block is correct and satisfies all the consensus rules that concern the header
    /// and block body itself.
    fn validate_block(&self, block: &SealedBlockWithSenders) -> Result<(), ConsensusError> {
        if let Err(e) = self.consensus.validate_header_with_total_difficulty(block, U256::MAX) {
            error!(
                ?block,
                "Failed to validate total difficulty for block {}: {e}",
                block.header.hash()
            );
            return Err(e)
        }

        if let Err(e) = self.consensus.validate_header(block) {
            error!(?block, "Failed to validate header {}: {e}", block.header.hash());
            return Err(e)
        }

        if let Err(e) = self.consensus.validate_block_pre_execution(block) {
            error!(?block, "Failed to validate block {}: {e}", block.header.hash());
            return Err(e)
        }

        Ok(())
    }

    fn buffer_block_without_senders(&mut self, block: SealedBlock) -> Result<(), InsertBlockError> {
        match block.try_seal_with_senders() {
            Ok(block) => self.buffer_block(block),
            Err(block) => Err(InsertBlockError::sender_recovery_error(block)),
        }
    }

    fn buffer_block(&mut self, block: SealedBlockWithSenders) -> Result<(), InsertBlockError> {
        if let Err(err) = self.validate_block(&block) {
            return Err(InsertBlockError::consensus_error(err, block.block))
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

        // check if the distance exceeds the threshold for backfill sync
        let mut exceeds_backfill_threshold =
            self.exceeds_backfill_run_threshold(canonical_tip_num, target_block_number);

        // check if the downloaded block is the tracked finalized block
        if let Some(buffered_finalized) = sync_target_state
            .as_ref()
            .and_then(|state| self.state.buffer.block(&state.finalized_block_hash))
        {
            // if we have buffered the finalized block, we should check how far
            // we're off
            exceeds_backfill_threshold =
                self.exceeds_backfill_run_threshold(canonical_tip_num, buffered_finalized.number);
        }

        // If this is invoked after we downloaded a block we can check if this block is the
        // finalized block
        if let (Some(downloaded_block), Some(ref state)) = (downloaded_block, sync_target_state) {
            if downloaded_block.hash == state.finalized_block_hash {
                // we downloaded the finalized block and can now check how far we're off
                exceeds_backfill_threshold =
                    self.exceeds_backfill_run_threshold(canonical_tip_num, downloaded_block.number);
            }
        }

        // if the number of missing blocks is greater than the max, trigger backfill
        if exceeds_backfill_threshold {
            if let Some(state) = sync_target_state {
                // if we have already canonicalized the finalized block, we should skip backfill
                match self.provider.header_by_hash_or_number(state.finalized_block_hash.into()) {
                    Err(err) => {
                        warn!(target: "consensus::engine", %err, "Failed to get finalized block header");
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
                        debug!(target: "consensus::engine", hash=?state.head_block_hash, "Setting head hash as an optimistic backfill target.");
                        return Some(state.head_block_hash)
                    }
                    Ok(Some(_)) => {
                        // we're fully synced to the finalized block
                    }
                }
            }
        }

        None
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
            DownloadRequest::BlockRange(missing_parent.hash, distance)
        } else {
            // This happens when the missing parent is on an outdated
            // sidechain and we can only download the missing block itself
            DownloadRequest::BlockSet(HashSet::from_iter([missing_parent.hash]))
        };

        Some(TreeEvent::Download(request))
    }

    /// Invoked with a block downloaded from the network
    ///
    /// Returns an event with the appropriate action to take, such as:
    ///  - download more missing blocks
    ///  - try to canonicalize the target if the `block` is the tracked target (head) block.
    fn on_downloaded_block(&mut self, block: SealedBlockWithSenders) -> Option<TreeEvent> {
        let block_num_hash = block.num_hash();
        let lowest_buffered_ancestor = self.lowest_buffered_ancestor_or(block_num_hash.hash);
        if self
            .check_invalid_ancestor_with_head(lowest_buffered_ancestor, block_num_hash.hash)
            .ok()?
            .is_some()
        {
            return None
        }

        if self.is_backfill_active {
            return None
        }

        // try to append the block
        match self.insert_block(block) {
            Ok(InsertPayloadOk::Inserted(BlockStatus::Valid(_))) => {
                if self.is_sync_target_head(block_num_hash.hash) {
                    return Some(TreeEvent::TreeAction(TreeAction::MakeCanonical(
                        block_num_hash.hash,
                    )))
                }
            }
            Ok(InsertPayloadOk::Inserted(BlockStatus::Disconnected { head, missing_ancestor })) => {
                // block is not connected to the canonical head, we need to download
                // its missing branch first
                return self.on_disconnected_downloaded_block(block_num_hash, missing_ancestor, head)
            }
            _ => {}
        }
        None
    }

    fn insert_block_without_senders(
        &mut self,
        block: SealedBlock,
    ) -> Result<InsertPayloadOk, InsertBlockError> {
        match block.try_seal_with_senders() {
            Ok(block) => self.insert_block(block),
            Err(block) => Err(InsertBlockError::sender_recovery_error(block)),
        }
    }

    fn insert_block(
        &mut self,
        block: SealedBlockWithSenders,
    ) -> Result<InsertPayloadOk, InsertBlockError> {
        self.insert_block_inner(block.clone())
            .map_err(|kind| InsertBlockError::new(block.block, kind))
    }

    fn insert_block_inner(
        &mut self,
        block: SealedBlockWithSenders,
    ) -> Result<InsertPayloadOk, InsertBlockErrorKind> {
        if self.block_by_hash(block.hash())?.is_some() {
            let attachment = BlockAttachment::Canonical; // TODO: remove or revise attachment
            return Ok(InsertPayloadOk::AlreadySeen(BlockStatus::Valid(attachment)))
        }

        // validate block consensus rules
        self.validate_block(&block)?;

        let state_provider = self.state_provider(block.parent_hash).unwrap();
        let executor = self.executor_provider.executor(StateProviderDatabase::new(&state_provider));

        let block_number = block.number;
        let block_hash = block.hash();
        let block = block.unseal();
        let output = executor.execute((&block, U256::MAX).into()).unwrap();
        self.consensus.validate_block_post_execution(
            &block,
            PostExecutionInput::new(&output.receipts, &output.requests),
        )?;

        // TODO: change StateRootProvider API to accept hashed post state
        let hashed_state = HashedPostState::from_bundle_state(&output.state.state);

        let (state_root, trie_output) = state_provider.state_root_with_updates(&output.state)?;
        if state_root != block.state_root {
            return Err(ConsensusError::BodyStateRootDiff(
                GotExpected { got: state_root, expected: block.state_root }.into(),
            )
            .into())
        }

        let executed = ExecutedBlock {
            block: Arc::new(block.block.seal(block_hash)),
            senders: Arc::new(block.senders),
            execution_output: Arc::new(ExecutionOutcome::new(
                output.state,
                Receipts::from(output.receipts),
                block_number,
                vec![Requests::from(output.requests)],
            )),
            hashed_state: Arc::new(hashed_state),
            trie: Arc::new(trie_output),
        };
        self.state.tree_state.insert_executed(executed);

        let attachment = BlockAttachment::Canonical; // TODO: remove or revise attachment
        Ok(InsertPayloadOk::Inserted(BlockStatus::Valid(attachment)))
    }

    /// Pre-validate forkchoice update and check whether it can be processed.
    ///
    /// This method returns the update outcome if validation fails or
    /// the node is syncing and the update cannot be processed at the moment.
    fn pre_validate_forkchoice_update(
        &mut self,
        state: ForkchoiceState,
    ) -> ProviderResult<Option<OnForkChoiceUpdated>> {
        if state.head_block_hash.is_zero() {
            return Ok(Some(OnForkChoiceUpdated::invalid_state()))
        }

        // check if the new head hash is connected to any ancestor that we previously marked as
        // invalid
        let lowest_buffered_ancestor_fcu = self.lowest_buffered_ancestor_or(state.head_block_hash);
        if let Some(status) = self.check_invalid_ancestor(lowest_buffered_ancestor_fcu)? {
            return Ok(Some(OnForkChoiceUpdated::with_invalid(status)))
        }

        if self.is_backfill_active {
            // We can only process new forkchoice updates if the pipeline is idle, since it requires
            // exclusive access to the database
            trace!(target: "consensus::engine", "Pipeline is syncing, skipping forkchoice update");
            return Ok(Some(OnForkChoiceUpdated::syncing()))
        }

        Ok(None)
    }

    /// Validates the payload attributes with respect to the header and fork choice state.
    ///
    /// Note: At this point, the fork choice update is considered to be VALID, however, we can still
    /// return an error if the payload attributes are invalid.
    fn process_payload_attributes(
        &self,
        attrs: T::PayloadAttributes,
        head: &Header,
        state: ForkchoiceState,
    ) -> OnForkChoiceUpdated {
        // 7. Client software MUST ensure that payloadAttributes.timestamp is greater than timestamp
        //    of a block referenced by forkchoiceState.headBlockHash. If this condition isn't held
        //    client software MUST respond with -38003: `Invalid payload attributes` and MUST NOT
        //    begin a payload build process. In such an event, the forkchoiceState update MUST NOT
        //    be rolled back.
        if attrs.timestamp() <= head.timestamp {
            return OnForkChoiceUpdated::invalid_payload_attributes()
        }

        // 8. Client software MUST begin a payload build process building on top of
        //    forkchoiceState.headBlockHash and identified via buildProcessId value if
        //    payloadAttributes is not null and the forkchoice state has been updated successfully.
        //    The build process is specified in the Payload building section.
        match <T::PayloadBuilderAttributes as PayloadBuilderAttributes>::try_new(
            state.head_block_hash,
            attrs,
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
}

impl<P, E, T> EngineApiTreeHandler for EngineApiTreeHandlerImpl<P, E, T>
where
    P: BlockReader + StateProviderFactory + Clone + 'static,
    E: BlockExecutorProvider,
    T: EngineTypes,
{
    type Engine = T;

    fn on_downloaded(&mut self, blocks: Vec<SealedBlockWithSenders>) -> Option<TreeEvent> {
        for block in blocks {
            if let Some(event) = self.on_downloaded_block(block) {
                let needs_backfill = event.is_backfill_action();
                self.on_tree_event(event);
                if needs_backfill {
                    // can exit early if backfill is needed
                    break
                }
            }
        }
        None
    }

    fn on_new_payload(
        &mut self,
        payload: ExecutionPayload,
        cancun_fields: Option<CancunPayloadFields>,
    ) -> ProviderResult<TreeOutcome<PayloadStatus>> {
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
        let parent_hash = payload.parent_hash();
        let block = match self
            .payload_validator
            .ensure_well_formed_payload(payload, cancun_fields.into())
        {
            Ok(block) => block,
            Err(error) => {
                error!(target: "engine::tree", %error, "Invalid payload");
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
                return Ok(TreeOutcome::new(PayloadStatus::new(status, latest_valid_hash)))
            }
        };

        let block_hash = block.hash();
        let mut lowest_buffered_ancestor = self.lowest_buffered_ancestor_or(block_hash);
        if lowest_buffered_ancestor == block_hash {
            lowest_buffered_ancestor = block.parent_hash;
        }

        // now check the block itself
        if let Some(status) =
            self.check_invalid_ancestor_with_head(lowest_buffered_ancestor, block_hash)?
        {
            return Ok(TreeOutcome::new(status))
        }

        let status = if self.is_backfill_active {
            self.buffer_block_without_senders(block).unwrap();
            PayloadStatus::from_status(PayloadStatusEnum::Syncing)
        } else {
            let mut latest_valid_hash = None;
            let status = match self.insert_block_without_senders(block).unwrap() {
                InsertPayloadOk::Inserted(BlockStatus::Valid(_)) |
                InsertPayloadOk::AlreadySeen(BlockStatus::Valid(_)) => {
                    latest_valid_hash = Some(block_hash);
                    PayloadStatusEnum::Valid
                }
                InsertPayloadOk::Inserted(BlockStatus::Disconnected { .. }) |
                InsertPayloadOk::AlreadySeen(BlockStatus::Disconnected { .. }) => {
                    // not known to be invalid, but we don't know anything else
                    PayloadStatusEnum::Syncing
                }
            };
            PayloadStatus::new(status, latest_valid_hash)
        };

        let mut outcome = TreeOutcome::new(status);
        if outcome.outcome.is_valid() && self.is_sync_target_head(block_hash) {
            // if the block is valid and it is the sync target head, make it canonical
            outcome =
                outcome.with_event(TreeEvent::TreeAction(TreeAction::MakeCanonical(block_hash)));
        }

        Ok(outcome)
    }

    fn on_forkchoice_updated(
        &mut self,
        state: ForkchoiceState,
        attrs: Option<<Self::Engine as PayloadTypes>::PayloadAttributes>,
    ) -> ProviderResult<TreeOutcome<OnForkChoiceUpdated>> {
        if let Some(on_updated) = self.pre_validate_forkchoice_update(state)? {
            self.state.forkchoice_state_tracker.set_latest(state, on_updated.forkchoice_status());
            return Ok(TreeOutcome::new(on_updated))
        }

        let valid_outcome = |head| {
            TreeOutcome::new(OnForkChoiceUpdated::valid(PayloadStatus::new(
                PayloadStatusEnum::Valid,
                Some(head),
            )))
        };

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

        // 1. ensure we have a new head block
        if self.state.tree_state.canonical_block_hash() == state.head_block_hash {
            // the head block is already canonical
            return Ok(valid_outcome(state.head_block_hash))
        }

        // 2. ensure we can apply a new chain update for the head block
        if let Some(update) = self.state.tree_state.on_new_head(state.head_block_hash) {
            // update the tracked canonical head
            self.state.tree_state.set_canonical_head(update.tip().num_hash());
            // TODO
            //  update inmemory state
            //  update trackers

            // sends an event to all active listeners about the new canonical chain
            self.canonical_in_memory_state.notify_canon_state(update.to_chain_notification());

            if let Some(attr) = attrs {
                let updated = self.process_payload_attributes(attr, &update.tip().header, state);
                return Ok(TreeOutcome::new(updated))
            }

            return Ok(valid_outcome(state.head_block_hash))
        }

        // 3. we don't have the block to perform the update
        let target = self.lowest_buffered_ancestor_or(state.head_block_hash);

        Ok(TreeOutcome::new(OnForkChoiceUpdated::valid(PayloadStatus::from_status(
            PayloadStatusEnum::Syncing,
        )))
        .with_event(TreeEvent::Download(DownloadRequest::single_block(target))))
    }
}

/// The state of the persistence task.
#[derive(Default, Debug)]
struct PersistenceState {
    /// Hash of the last block persisted.
    last_persisted_block_hash: B256,
    /// Receiver end of channel where the result of the persistence task will be
    /// sent when done. A None value means there's no persistence task in progress.
    rx: Option<oneshot::Receiver<B256>>,
    /// The last persisted block number.
    last_persisted_block_number: u64,
}

impl PersistenceState {
    /// Determines if there is a persistence task in progress by checking if the
    /// receiver is set.
    const fn in_progress(&self) -> bool {
        self.rx.is_some()
    }

    /// Sets state for a started persistence task.
    fn start(&mut self, rx: oneshot::Receiver<B256>) {
        self.rx = Some(rx);
    }

    /// Sets state for a finished persistence task.
    fn finish(&mut self, last_persisted_block_hash: B256, last_persisted_block_number: u64) {
        self.rx = None;
        self.last_persisted_block_number = last_persisted_block_number;
        self.last_persisted_block_hash = last_persisted_block_hash;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persistence::PersistenceAction;
    use alloy_rlp::Decodable;
    use reth_beacon_consensus::EthBeaconConsensus;
    use reth_chain_state::{test_utils::get_executed_blocks, BlockState};
    use reth_chainspec::{ChainSpecBuilder, HOLESKY, MAINNET};
    use reth_ethereum_engine_primitives::EthEngineTypes;
    use reth_evm::test_utils::MockExecutorProvider;
    use reth_payload_builder::PayloadServiceCommand;
    use reth_primitives::Bytes;
    use reth_provider::test_utils::MockEthProvider;
    use reth_rpc_types_compat::engine::block_to_payload_v1;
    use std::{
        str::FromStr,
        sync::mpsc::{channel, Sender},
    };
    use tokio::sync::mpsc::unbounded_channel;

    struct TestHarness {
        tree: EngineApiTreeHandlerImpl<MockEthProvider, MockExecutorProvider, EthEngineTypes>,
        to_tree_tx: Sender<FromEngine<BeaconEngineMessage<EthEngineTypes>>>,
        blocks: Vec<ExecutedBlock>,
        action_rx: Receiver<PersistenceAction>,
        payload_command_rx: UnboundedReceiver<PayloadServiceCommand<EthEngineTypes>>,
    }

    impl TestHarness {
        fn holesky() -> Self {
            let (action_tx, action_rx) = channel();
            let persistence_handle = PersistenceHandle::new(action_tx);

            let chain_spec = HOLESKY.clone();
            let consensus = Arc::new(EthBeaconConsensus::new(chain_spec.clone()));

            let provider = MockEthProvider::default();
            let executor_factory = MockExecutorProvider::default();

            let payload_validator = ExecutionPayloadValidator::new(chain_spec.clone());

            let (to_tree_tx, to_tree_rx) = channel();
            let (from_tree_tx, from_tree_rx) = unbounded_channel();

            let header = chain_spec.genesis_header().seal_slow();
            let engine_api_tree_state = EngineApiTreeState::new(10, 10, header.num_hash());

            let (to_payload_service, payload_command_rx) = unbounded_channel();
            let payload_builder = PayloadBuilderHandle::new(to_payload_service);
            let tree = EngineApiTreeHandlerImpl::new(
                provider,
                executor_factory,
                consensus,
                payload_validator,
                to_tree_rx,
                from_tree_tx,
                engine_api_tree_state,
                header,
                persistence_handle,
                payload_builder,
            );

            Self { tree, to_tree_tx, blocks: vec![], action_rx, payload_command_rx }
        }
    }

    fn get_default_test_harness(number_of_blocks: u64) -> TestHarness {
        let blocks: Vec<_> = get_executed_blocks(0..number_of_blocks).collect();

        let mut blocks_by_hash = HashMap::new();
        let mut blocks_by_number = BTreeMap::new();
        let mut state_by_hash = HashMap::new();
        let mut hash_by_number = HashMap::new();
        for block in &blocks {
            let sealed_block = block.block();
            let hash = sealed_block.hash();
            let number = sealed_block.number;
            blocks_by_hash.insert(hash, block.clone());
            blocks_by_number.entry(number).or_insert_with(Vec::new).push(block.clone());
            state_by_hash.insert(hash, Arc::new(BlockState::new(block.clone())));
            hash_by_number.insert(number, hash);
        }
        let tree_state = TreeState { blocks_by_hash, blocks_by_number, ..Default::default() };

        let (action_tx, action_rx) = channel();
        let persistence_handle = PersistenceHandle::new(action_tx);

        let chain_spec = Arc::new(
            ChainSpecBuilder::default()
                .chain(MAINNET.chain)
                .genesis(MAINNET.genesis.clone())
                .paris_activated()
                .build(),
        );
        let consensus = Arc::new(EthBeaconConsensus::new(chain_spec.clone()));

        let provider = MockEthProvider::default();
        let executor_factory = MockExecutorProvider::default();
        executor_factory.extend(vec![ExecutionOutcome::default()]);

        let payload_validator = ExecutionPayloadValidator::new(chain_spec);

        let (to_tree_tx, to_tree_rx) = channel();
        let (from_tree_tx, from_tree_rx) = unbounded_channel();

        let engine_api_tree_state = EngineApiTreeState {
            invalid_headers: InvalidHeaderCache::new(10),
            buffer: BlockBuffer::new(10),
            tree_state,
            forkchoice_state_tracker: ForkchoiceStateTracker::default(),
        };

        let header = blocks.first().unwrap().block().header.clone();

        let (to_payload_service, payload_command_rx) = unbounded_channel();
        let payload_builder = PayloadBuilderHandle::new(to_payload_service);
        let mut tree = EngineApiTreeHandlerImpl::new(
            provider,
            executor_factory,
            consensus,
            payload_validator,
            to_tree_rx,
            from_tree_tx,
            engine_api_tree_state,
            header,
            persistence_handle,
            payload_builder,
        );
        let last_executed_block = blocks.last().unwrap().clone();
        let pending = Some(BlockState::new(last_executed_block));
        tree.canonical_in_memory_state =
            CanonicalInMemoryState::new(state_by_hash, hash_by_number, pending);

        TestHarness { tree, to_tree_tx, blocks, action_rx, payload_command_rx }
    }

    #[tokio::test]
    async fn test_tree_persist_blocks() {
        // we need more than PERSISTENCE_THRESHOLD blocks to trigger the
        // persistence task.
        let TestHarness { tree, to_tree_tx, action_rx, mut blocks, payload_command_rx } =
            get_default_test_harness(PERSISTENCE_THRESHOLD + 1);
        std::thread::Builder::new().name("Tree Task".to_string()).spawn(|| tree.run()).unwrap();

        // send a message to the tree to enter the main loop.
        to_tree_tx.send(FromEngine::DownloadedBlocks(vec![])).unwrap();

        let received_action = action_rx.recv().expect("Failed to receive saved blocks");
        if let PersistenceAction::SaveBlocks((saved_blocks, _)) = received_action {
            // only PERSISTENCE_THRESHOLD will be persisted
            blocks.pop();
            assert_eq!(saved_blocks.len() as u64, PERSISTENCE_THRESHOLD);
            assert_eq!(saved_blocks, blocks);
        } else {
            panic!("unexpected action received {received_action:?}");
        }
    }

    #[tokio::test]
    async fn test_in_memory_state_trait_impl() {
        let TestHarness { tree, to_tree_tx, action_rx, blocks, payload_command_rx } =
            get_default_test_harness(10);

        let head_block = blocks.last().unwrap().block();
        let first_block = blocks.first().unwrap().block();

        for executed_block in blocks {
            let sealed_block = executed_block.block();

            let expected_state = BlockState::new(executed_block.clone());

            let actual_state_by_hash =
                tree.canonical_in_memory_state.state_by_hash(sealed_block.hash()).unwrap();
            assert_eq!(expected_state, *actual_state_by_hash);

            let actual_state_by_number =
                tree.canonical_in_memory_state.state_by_number(sealed_block.number).unwrap();
            assert_eq!(expected_state, *actual_state_by_number);
        }
    }

    #[tokio::test]
    async fn test_engine_request_during_backfill() {
        let TestHarness { mut tree, to_tree_tx, action_rx, blocks, payload_command_rx } =
            get_default_test_harness(PERSISTENCE_THRESHOLD);

        // set backfill active
        tree.is_backfill_active = true;

        let (tx, rx) = oneshot::channel();
        tree.on_engine_message(FromEngine::Request(BeaconEngineMessage::ForkchoiceUpdated {
            state: ForkchoiceState {
                head_block_hash: B256::random(),
                safe_block_hash: B256::random(),
                finalized_block_hash: B256::random(),
            },
            payload_attrs: None,
            tx,
        }));

        let resp = rx.await.unwrap().unwrap().await.unwrap();
        assert!(resp.payload_status.is_syncing());
    }

    #[tokio::test]
    async fn test_holesky_payload() {
        let s = include_str!("../../test-data/holesky/1.rlp");
        let data = Bytes::from_str(s).unwrap();
        let block = Block::decode(&mut data.as_ref()).unwrap();
        let sealed = block.seal_slow();
        let payload = block_to_payload_v1(sealed);

        let TestHarness { mut tree, to_tree_tx, action_rx, blocks, payload_command_rx } =
            TestHarness::holesky();

        // set backfill active
        tree.is_backfill_active = true;

        let (tx, rx) = oneshot::channel();
        tree.on_engine_message(FromEngine::Request(BeaconEngineMessage::NewPayload {
            payload: payload.clone().into(),
            cancun_fields: None,
            tx,
        }));

        let resp = rx.await.unwrap().unwrap();
        assert!(resp.is_syncing());
    }
}
