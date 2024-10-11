use crate::{
    backfill::{BackfillAction, BackfillSyncState},
    chain::FromOrchestrator,
    engine::{DownloadRequest, EngineApiEvent, FromEngine},
    persistence::PersistenceHandle,
};
use alloy_eips::BlockNumHash;
use alloy_primitives::{
    map::{HashMap, HashSet},
    BlockNumber, B256, U256,
};
use alloy_rpc_types_engine::{
    CancunPayloadFields, ExecutionPayload, ForkchoiceState, PayloadStatus, PayloadStatusEnum,
    PayloadValidationError,
};
use reth_beacon_consensus::{
    BeaconConsensusEngineEvent, BeaconEngineMessage, ForkchoiceStateTracker, InvalidHeaderCache,
    OnForkChoiceUpdated, MIN_BLOCKS_FOR_PIPELINE_RUN,
};
use reth_blockchain_tree::{
    error::{InsertBlockErrorKindTwo, InsertBlockErrorTwo, InsertBlockFatalError},
    BlockBuffer, BlockStatus2, InsertPayloadOk2,
};
use reth_chain_state::{
    CanonicalInMemoryState, ExecutedBlock, MemoryOverlayStateProvider, NewCanonicalChain,
};
use reth_chainspec::EthereumHardforks;
use reth_consensus::{Consensus, PostExecutionInput};
use reth_engine_primitives::EngineTypes;
use reth_errors::{ConsensusError, ProviderResult};
use reth_evm::execute::BlockExecutorProvider;
use reth_payload_builder::PayloadBuilderHandle;
use reth_payload_primitives::{PayloadAttributes, PayloadBuilder, PayloadBuilderAttributes};
use reth_payload_validator::ExecutionPayloadValidator;
use reth_primitives::{
    Block, GotExpected, Header, SealedBlock, SealedBlockWithSenders, SealedHeader,
};
use reth_provider::{
    providers::ConsistentDbView, BlockReader, DatabaseProviderFactory, ExecutionOutcome,
    ProviderError, StateProviderBox, StateProviderFactory, StateReader, StateRootProvider,
    TransactionVariant,
};
use reth_revm::database::StateProviderDatabase;
use reth_stages_api::ControlFlow;
use reth_trie::{updates::TrieUpdates, HashedPostState, TrieInput};
use reth_trie_parallel::parallel_root::{ParallelStateRoot, ParallelStateRootError};
use std::{
    cmp::Ordering,
    collections::{btree_map, hash_map, BTreeMap, VecDeque},
    fmt::Debug,
    ops::Bound,
    sync::{
        mpsc::{Receiver, RecvError, RecvTimeoutError, Sender},
        Arc,
    },
    time::Instant,
};
use tokio::sync::{
    mpsc::{UnboundedReceiver, UnboundedSender},
    oneshot,
    oneshot::error::TryRecvError,
};
use tracing::*;

pub mod config;
mod invalid_block_hook;
mod metrics;
mod persistence_state;
use crate::{
    engine::{EngineApiKind, EngineApiRequest},
    tree::metrics::EngineApiMetrics,
};
pub use config::TreeConfig;
pub use invalid_block_hook::{InvalidBlockHooks, NoopInvalidBlockHook};
pub use persistence_state::PersistenceState;
pub use reth_engine_primitives::InvalidBlockHook;

/// Keeps track of the state of the tree.
///
/// ## Invariants
///
/// - This only stores blocks that are connected to the canonical chain.
/// - All executed blocks are valid and have been executed.
#[derive(Debug, Default)]
pub struct TreeState {
    /// __All__ unique executed blocks by block hash that are connected to the canonical chain.
    ///
    /// This includes blocks of all forks.
    blocks_by_hash: HashMap<B256, ExecutedBlock>,
    /// Executed blocks grouped by their respective block number.
    ///
    /// This maps unique block number to all known blocks for that height.
    ///
    /// Note: there can be multiple blocks at the same height due to forks.
    blocks_by_number: BTreeMap<BlockNumber, Vec<ExecutedBlock>>,
    /// Map of any parent block hash to its children.
    parent_to_child: HashMap<B256, HashSet<B256>>,
    /// Map of hash to trie updates for canonical blocks that are persisted but not finalized.
    ///
    /// Contains the block number for easy removal.
    persisted_trie_updates: HashMap<B256, (BlockNumber, Arc<TrieUpdates>)>,
    /// Currently tracked canonical head of the chain.
    current_canonical_head: BlockNumHash,
}

impl TreeState {
    /// Returns a new, empty tree state that points to the given canonical head.
    fn new(current_canonical_head: BlockNumHash) -> Self {
        Self {
            blocks_by_hash: HashMap::default(),
            blocks_by_number: BTreeMap::new(),
            current_canonical_head,
            parent_to_child: HashMap::default(),
            persisted_trie_updates: HashMap::default(),
        }
    }

    /// Returns the number of executed blocks stored.
    fn block_count(&self) -> usize {
        self.blocks_by_hash.len()
    }

    /// Returns the [`ExecutedBlock`] by hash.
    fn executed_block_by_hash(&self, hash: B256) -> Option<&ExecutedBlock> {
        self.blocks_by_hash.get(&hash)
    }

    /// Returns the block by hash.
    fn block_by_hash(&self, hash: B256) -> Option<Arc<SealedBlock>> {
        self.blocks_by_hash.get(&hash).map(|b| b.block.clone())
    }

    /// Returns all available blocks for the given hash that lead back to the canonical chain, from
    /// newest to oldest. And the parent hash of the oldest block that is missing from the buffer.
    ///
    /// Returns `None` if the block for the given hash is not found.
    fn blocks_by_hash(&self, hash: B256) -> Option<(B256, Vec<ExecutedBlock>)> {
        let block = self.blocks_by_hash.get(&hash).cloned()?;
        let mut parent_hash = block.block().parent_hash;
        let mut blocks = vec![block];
        while let Some(executed) = self.blocks_by_hash.get(&parent_hash) {
            parent_hash = executed.block.parent_hash;
            blocks.push(executed.clone());
        }

        Some((parent_hash, blocks))
    }

    /// Insert executed block into the state.
    fn insert_executed(&mut self, executed: ExecutedBlock) {
        let hash = executed.block.hash();
        let parent_hash = executed.block.parent_hash;
        let block_number = executed.block.number;

        if self.blocks_by_hash.contains_key(&hash) {
            return;
        }

        self.blocks_by_hash.insert(hash, executed.clone());

        self.blocks_by_number.entry(block_number).or_default().push(executed);

        self.parent_to_child.entry(parent_hash).or_default().insert(hash);

        if let Some(existing_blocks) = self.blocks_by_number.get(&block_number) {
            if existing_blocks.len() > 1 {
                self.parent_to_child.entry(parent_hash).or_default().insert(hash);
            }
        }

        for children in self.parent_to_child.values_mut() {
            children.retain(|child| self.blocks_by_hash.contains_key(child));
        }
    }

    /// Remove single executed block by its hash.
    ///
    /// ## Returns
    ///
    /// The removed block and the block hashes of its children.
    fn remove_by_hash(&mut self, hash: B256) -> Option<(ExecutedBlock, HashSet<B256>)> {
        let executed = self.blocks_by_hash.remove(&hash)?;

        // Remove this block from collection of children of its parent block.
        let parent_entry = self.parent_to_child.entry(executed.block.parent_hash);
        if let hash_map::Entry::Occupied(mut entry) = parent_entry {
            entry.get_mut().remove(&hash);

            if entry.get().is_empty() {
                entry.remove();
            }
        }

        // Remove point to children of this block.
        let children = self.parent_to_child.remove(&hash).unwrap_or_default();

        // Remove this block from `blocks_by_number`.
        let block_number_entry = self.blocks_by_number.entry(executed.block.number);
        if let btree_map::Entry::Occupied(mut entry) = block_number_entry {
            // We have to find the index of the block since it exists in a vec
            if let Some(index) = entry.get().iter().position(|b| b.block.hash() == hash) {
                entry.get_mut().swap_remove(index);

                // If there are no blocks left then remove the entry for this block
                if entry.get().is_empty() {
                    entry.remove();
                }
            }
        }

        Some((executed, children))
    }

    /// Returns whether or not the hash is part of the canonical chain.
    pub(crate) fn is_canonical(&self, hash: B256) -> bool {
        let mut current_block = self.current_canonical_head.hash;
        if current_block == hash {
            return true
        }

        while let Some(executed) = self.blocks_by_hash.get(&current_block) {
            current_block = executed.block.parent_hash;
            if current_block == hash {
                return true
            }
        }

        false
    }

    /// Removes canonical blocks below the upper bound, only if the last persisted hash is
    /// part of the canonical chain.
    pub(crate) fn remove_canonical_until(
        &mut self,
        upper_bound: BlockNumber,
        last_persisted_hash: B256,
    ) {
        debug!(target: "engine::tree", ?upper_bound, ?last_persisted_hash, "Removing canonical blocks from the tree");

        // If the last persisted hash is not canonical, then we don't want to remove any canonical
        // blocks yet.
        if !self.is_canonical(last_persisted_hash) {
            return
        }

        // First, let's walk back the canonical chain and remove canonical blocks lower than the
        // upper bound
        let mut current_block = self.current_canonical_head.hash;
        while let Some(executed) = self.blocks_by_hash.get(&current_block) {
            current_block = executed.block.parent_hash;
            if executed.block.number <= upper_bound {
                debug!(target: "engine::tree", num_hash=?executed.block.num_hash(), "Attempting to remove block walking back from the head");
                if let Some((removed, _)) = self.remove_by_hash(executed.block.hash()) {
                    debug!(target: "engine::tree", num_hash=?removed.block.num_hash(), "Removed block walking back from the head");
                    // finally, move the trie updates
                    self.persisted_trie_updates
                        .insert(removed.block.hash(), (removed.block.number, removed.trie));
                }
            }
        }
    }

    /// Removes all blocks that are below the finalized block, as well as removing non-canonical
    /// sidechains that fork from below the finalized block.
    pub(crate) fn prune_finalized_sidechains(&mut self, finalized_num_hash: BlockNumHash) {
        let BlockNumHash { number: finalized_num, hash: finalized_hash } = finalized_num_hash;

        // We remove disconnected sidechains in three steps:
        // * first, remove everything with a block number __below__ the finalized block.
        // * next, we populate a vec with parents __at__ the finalized block.
        // * finally, we iterate through the vec, removing children until the vec is empty
        // (BFS).

        // We _exclude_ the finalized block because we will be dealing with the blocks __at__
        // the finalized block later.
        let blocks_to_remove = self
            .blocks_by_number
            .range((Bound::Unbounded, Bound::Excluded(finalized_num)))
            .flat_map(|(_, blocks)| blocks.iter().map(|b| b.block.hash()))
            .collect::<Vec<_>>();
        for hash in blocks_to_remove {
            if let Some((removed, _)) = self.remove_by_hash(hash) {
                debug!(target: "engine::tree", num_hash=?removed.block.num_hash(), "Removed finalized sidechain block");
            }
        }

        // remove trie updates that are below the finalized block
        self.persisted_trie_updates.retain(|_, (block_num, _)| *block_num > finalized_num);

        // The only block that should remain at the `finalized` number now, is the finalized
        // block, if it exists.
        //
        // For all other blocks, we  first put their children into this vec.
        // Then, we will iterate over them, removing them, adding their children, etc etc,
        // until the vec is empty.
        let mut blocks_to_remove = self.blocks_by_number.remove(&finalized_num).unwrap_or_default();

        // re-insert the finalized hash if we removed it
        if let Some(position) =
            blocks_to_remove.iter().position(|b| b.block.hash() == finalized_hash)
        {
            let finalized_block = blocks_to_remove.swap_remove(position);
            self.blocks_by_number.insert(finalized_num, vec![finalized_block]);
        }

        let mut blocks_to_remove =
            blocks_to_remove.into_iter().map(|e| e.block.hash()).collect::<VecDeque<_>>();
        while let Some(block) = blocks_to_remove.pop_front() {
            if let Some((removed, children)) = self.remove_by_hash(block) {
                debug!(target: "engine::tree", num_hash=?removed.block.num_hash(), "Removed finalized sidechain child block");
                blocks_to_remove.extend(children);
            }
        }
    }

    /// Remove all blocks up to __and including__ the given block number.
    ///
    /// If a finalized hash is provided, the only non-canonical blocks which will be removed are
    /// those which have a fork point at or below the finalized hash.
    ///
    /// Canonical blocks below the upper bound will still be removed.
    ///
    /// NOTE: if the finalized block is greater than the upper bound, the only blocks that will be
    /// removed are canonical blocks and sidechains that fork below the `upper_bound`. This is the
    /// same behavior as if the `finalized_num` were `Some(upper_bound)`.
    pub(crate) fn remove_until(
        &mut self,
        upper_bound: BlockNumHash,
        last_persisted_hash: B256,
        finalized_num_hash: Option<BlockNumHash>,
    ) {
        debug!(target: "engine::tree", ?upper_bound, ?finalized_num_hash, "Removing blocks from the tree");

        // If the finalized num is ahead of the upper bound, and exists, we need to instead ensure
        // that the only blocks removed, are canonical blocks less than the upper bound
        let finalized_num_hash = finalized_num_hash.map(|mut finalized| {
            if upper_bound.number < finalized.number {
                finalized = upper_bound;
                debug!(target: "engine::tree", ?finalized, "Adjusted upper bound");
            }
            finalized
        });

        // We want to do two things:
        // * remove canonical blocks that are persisted
        // * remove forks whose root are below the finalized block
        // We can do this in 2 steps:
        // * remove all canonical blocks below the upper bound
        // * fetch the number of the finalized hash, removing any sidechains that are __below__ the
        // finalized block
        self.remove_canonical_until(upper_bound.number, last_persisted_hash);

        // Now, we have removed canonical blocks (assuming the upper bound is above the finalized
        // block) and only have sidechains below the finalized block.
        if let Some(finalized_num_hash) = finalized_num_hash {
            self.prune_finalized_sidechains(finalized_num_hash);
        }
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

    /// Returns the block number of the canonical head.
    const fn canonical_block_number(&self) -> BlockNumber {
        self.canonical_head().number
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

/// The engine API tree handler implementation.
///
/// This type is responsible for processing engine API requests, maintaining the canonical state and
/// emitting events.
pub struct EngineApiTreeHandler<P, E, T: EngineTypes, Spec> {
    provider: P,
    executor_provider: E,
    consensus: Arc<dyn Consensus>,
    payload_validator: ExecutionPayloadValidator<Spec>,
    /// Keeps track of internals such as executed and buffered blocks.
    state: EngineApiTreeState,
    /// The half for sending messages to the engine.
    ///
    /// This is kept so that we can queue in messages to ourself that we can process later, for
    /// example distributing workload across multiple messages that would otherwise take too long
    /// to process. E.g. we might receive a range of downloaded blocks and we want to process
    /// them one by one so that we can handle incoming engine API in between and don't become
    /// unresponsive. This can happen during live sync transition where we're trying to close the
    /// gap (up to 3 epochs of blocks in the worst case).
    incoming_tx: Sender<FromEngine<EngineApiRequest<T>>>,
    /// Incoming engine API requests.
    incoming: Receiver<FromEngine<EngineApiRequest<T>>>,
    /// Outgoing events that are emitted to the handler.
    outgoing: UnboundedSender<EngineApiEvent>,
    /// Channels to the persistence layer.
    persistence: PersistenceHandle,
    /// Tracks the state changes of the persistence task.
    persistence_state: PersistenceState,
    /// Flag indicating the state of the node's backfill synchronization process.
    backfill_sync_state: BackfillSyncState,
    /// Keeps track of the state of the canonical chain that isn't persisted yet.
    /// This is intended to be accessed from external sources, such as rpc.
    canonical_in_memory_state: CanonicalInMemoryState,
    /// Handle to the payload builder that will receive payload attributes for valid forkchoice
    /// updates
    payload_builder: PayloadBuilderHandle<T>,
    /// Configuration settings.
    config: TreeConfig,
    /// Metrics for the engine api.
    metrics: EngineApiMetrics,
    /// An invalid block hook.
    invalid_block_hook: Box<dyn InvalidBlockHook>,
    /// The engine API variant of this handler
    engine_kind: EngineApiKind,
}

impl<P: Debug, E: Debug, T: EngineTypes + Debug, Spec: Debug> std::fmt::Debug
    for EngineApiTreeHandler<P, E, T, Spec>
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EngineApiTreeHandler")
            .field("provider", &self.provider)
            .field("executor_provider", &self.executor_provider)
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
            .field("invalid_block_hook", &format!("{:p}", self.invalid_block_hook))
            .field("engine_kind", &self.engine_kind)
            .finish()
    }
}

impl<P, E, T, Spec> EngineApiTreeHandler<P, E, T, Spec>
where
    P: DatabaseProviderFactory + BlockReader + StateProviderFactory + StateReader + Clone + 'static,
    <P as DatabaseProviderFactory>::Provider: BlockReader,
    E: BlockExecutorProvider,
    T: EngineTypes,
    Spec: Send + Sync + EthereumHardforks + 'static,
{
    /// Creates a new [`EngineApiTreeHandler`].
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        provider: P,
        executor_provider: E,
        consensus: Arc<dyn Consensus>,
        payload_validator: ExecutionPayloadValidator<Spec>,
        outgoing: UnboundedSender<EngineApiEvent>,
        state: EngineApiTreeState,
        canonical_in_memory_state: CanonicalInMemoryState,
        persistence: PersistenceHandle,
        persistence_state: PersistenceState,
        payload_builder: PayloadBuilderHandle<T>,
        config: TreeConfig,
        engine_kind: EngineApiKind,
    ) -> Self {
        let (incoming_tx, incoming) = std::sync::mpsc::channel();

        Self {
            provider,
            executor_provider,
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
            invalid_block_hook: Box::new(NoopInvalidBlockHook),
            engine_kind,
        }
    }

    /// Sets the invalid block hook.
    fn set_invalid_block_hook(&mut self, invalid_block_hook: Box<dyn InvalidBlockHook>) {
        self.invalid_block_hook = invalid_block_hook;
    }

    /// Creates a new [`EngineApiTreeHandler`] instance and spawns it in its
    /// own thread.
    ///
    /// Returns the sender through which incoming requests can be sent to the task and the receiver
    /// end of a [`EngineApiEvent`] unbounded channel to receive events from the engine.
    #[allow(clippy::too_many_arguments)]
    pub fn spawn_new(
        provider: P,
        executor_provider: E,
        consensus: Arc<dyn Consensus>,
        payload_validator: ExecutionPayloadValidator<Spec>,
        persistence: PersistenceHandle,
        payload_builder: PayloadBuilderHandle<T>,
        canonical_in_memory_state: CanonicalInMemoryState,
        config: TreeConfig,
        invalid_block_hook: Box<dyn InvalidBlockHook>,
        kind: EngineApiKind,
    ) -> (Sender<FromEngine<EngineApiRequest<T>>>, UnboundedReceiver<EngineApiEvent>) {
        let best_block_number = provider.best_block_number().unwrap_or(0);
        let header = provider.sealed_header(best_block_number).ok().flatten().unwrap_or_default();

        let persistence_state = PersistenceState {
            last_persisted_block: BlockNumHash::new(best_block_number, header.hash()),
            rx: None,
            remove_above_state: VecDeque::new(),
        };

        let (tx, outgoing) = tokio::sync::mpsc::unbounded_channel();
        let state = EngineApiTreeState::new(
            config.block_buffer_limit(),
            config.max_invalid_header_cache_length(),
            header.num_hash(),
        );

        let mut task = Self::new(
            provider,
            executor_provider,
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
        );
        task.set_invalid_block_hook(invalid_block_hook);
        let incoming = task.incoming_tx.clone();
        std::thread::Builder::new().name("Tree Task".to_string()).spawn(|| task.run()).unwrap();
        (incoming, outgoing)
    }

    /// Returns a new [`Sender`] to send messages to this type.
    pub fn sender(&self) -> Sender<FromEngine<EngineApiRequest<T>>> {
        self.incoming_tx.clone()
    }

    /// Run the engine API handler.
    ///
    /// This will block the current thread and process incoming messages.
    pub fn run(mut self) {
        loop {
            match self.try_recv_engine_message() {
                Ok(Some(msg)) => {
                    debug!(target: "engine::tree", %msg, "received new engine message");
                    if let Err(fatal) = self.on_engine_message(msg) {
                        error!(target: "engine::tree", %fatal, "insert block fatal error");
                        return
                    }
                }
                Ok(None) => {
                    debug!(target: "engine::tree", "received no engine message for some time, while waiting for persistence task to complete");
                }
                Err(_err) => {
                    error!(target: "engine::tree", "Engine channel disconnected");
                    return
                }
            }

            if let Err(err) = self.advance_persistence() {
                error!(target: "engine::tree", %err, "Advancing persistence failed");
                return
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
        mut blocks: Vec<SealedBlockWithSenders>,
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
    #[instrument(level = "trace", skip_all, fields(block_hash = %payload.block_hash(), block_num = %payload.block_number(),), target = "engine::tree")]
    fn on_new_payload(
        &mut self,
        payload: ExecutionPayload,
        cancun_fields: Option<CancunPayloadFields>,
    ) -> Result<TreeOutcome<PayloadStatus>, InsertBlockFatalError> {
        trace!(target: "engine::tree", "invoked new payload");
        self.metrics.engine.new_payload_messages.increment(1);

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

        let status = if self.backfill_sync_state.is_idle() {
            let mut latest_valid_hash = None;
            let num_hash = block.num_hash();
            match self.insert_block_without_senders(block) {
                Ok(status) => {
                    let status = match status {
                        InsertPayloadOk2::Inserted(BlockStatus2::Valid) => {
                            latest_valid_hash = Some(block_hash);
                            self.try_connect_buffered_blocks(num_hash)?;
                            PayloadStatusEnum::Valid
                        }
                        InsertPayloadOk2::AlreadySeen(BlockStatus2::Valid) => {
                            latest_valid_hash = Some(block_hash);
                            PayloadStatusEnum::Valid
                        }
                        InsertPayloadOk2::Inserted(BlockStatus2::Disconnected { .. }) |
                        InsertPayloadOk2::AlreadySeen(BlockStatus2::Disconnected { .. }) => {
                            // not known to be invalid, but we don't know anything else
                            PayloadStatusEnum::Syncing
                        }
                    };

                    PayloadStatus::new(status, latest_valid_hash)
                }
                Err(error) => self.on_insert_block_error(error)?,
            }
        } else if let Err(error) = self.buffer_block_without_senders(block) {
            self.on_insert_block_error(error)?
        } else {
            PayloadStatus::from_status(PayloadStatusEnum::Syncing)
        };

        let mut outcome = TreeOutcome::new(status);
        if outcome.outcome.is_valid() && self.is_sync_target_head(block_hash) {
            // if the block is valid and it is the sync target head, make it canonical
            outcome = outcome.with_event(TreeEvent::TreeAction(TreeAction::MakeCanonical {
                sync_target_head: block_hash,
            }));
        }

        Ok(outcome)
    }

    /// Returns the new chain for the given head.
    ///
    /// This also handles reorgs.
    ///
    /// Note: This does not update the tracked state and instead returns the new chain based on the
    /// given head.
    fn on_new_head(&self, new_head: B256) -> ProviderResult<Option<NewCanonicalChain>> {
        // get the executed new head block
        let Some(new_head_block) = self.state.tree_state.blocks_by_hash.get(&new_head) else {
            return Ok(None)
        };

        let new_head_number = new_head_block.block.number;
        let mut current_canonical_number = self.state.tree_state.current_canonical_head.number;

        let mut new_chain = vec![new_head_block.clone()];
        let mut current_hash = new_head_block.block.parent_hash;
        let mut current_number = new_head_number - 1;

        // Walk back the new chain until we reach a block we know about
        //
        // This is only done for in-memory blocks, because we should not have persisted any blocks
        // that are _above_ the current canonical head.
        while current_number > current_canonical_number {
            if let Some(block) = self.executed_block_by_hash(current_hash)? {
                current_hash = block.block.parent_hash;
                current_number -= 1;
                new_chain.push(block);
            } else {
                warn!(target: "engine::tree", current_hash=?current_hash, "Sidechain block not found in TreeState");
                // This should never happen as we're walking back a chain that should connect to
                // the canonical chain
                return Ok(None);
            }
        }

        // If we have reached the current canonical head by walking back from the target, then we
        // know this represents an extension of the canonical chain.
        if current_hash == self.state.tree_state.current_canonical_head.hash {
            new_chain.reverse();

            // Simple extension of the current chain
            return Ok(Some(NewCanonicalChain::Commit { new: new_chain }));
        }

        // We have a reorg. Walk back both chains to find the fork point.
        let mut old_chain = Vec::new();
        let mut old_hash = self.state.tree_state.current_canonical_head.hash;

        // If the canonical chain is ahead of the new chain,
        // gather all blocks until new head number.
        while current_canonical_number > current_number {
            if let Some(block) = self.executed_block_by_hash(old_hash)? {
                old_chain.push(block.clone());
                old_hash = block.block.header.parent_hash;
                current_canonical_number -= 1;
            } else {
                // This shouldn't happen as we're walking back the canonical chain
                warn!(target: "engine::tree", current_hash=?old_hash, "Canonical block not found in TreeState");
                return Ok(None);
            }
        }

        // Both new and old chain pointers are now at the same height.
        debug_assert_eq!(current_number, current_canonical_number);

        // Walk both chains from specified hashes at same height until
        // a common ancestor (fork block) is reached.
        while old_hash != current_hash {
            if let Some(block) = self.executed_block_by_hash(old_hash)? {
                old_hash = block.block.header.parent_hash;
                old_chain.push(block);
            } else {
                // This shouldn't happen as we're walking back the canonical chain
                warn!(target: "engine::tree", current_hash=?old_hash, "Canonical block not found in TreeState");
                return Ok(None);
            }

            if let Some(block) = self.executed_block_by_hash(current_hash)? {
                current_hash = block.block.parent_hash;
                new_chain.push(block);
            } else {
                // This shouldn't happen as we've already walked this path
                warn!(target: "engine::tree", invalid_hash=?current_hash, "New chain block not found in TreeState");
                return Ok(None);
            }
        }
        new_chain.reverse();
        old_chain.reverse();

        Ok(Some(NewCanonicalChain::Reorg { new: new_chain, old: old_chain }))
    }

    /// Determines if the given block is part of a fork by checking that these
    /// conditions are true:
    /// * walking back from the target hash to verify that the target hash is not part of an
    ///   extension of the canonical chain.
    /// * walking back from the current head to verify that the target hash is not already part of
    ///   the canonical chain.
    fn is_fork(&self, target_hash: B256) -> ProviderResult<bool> {
        // verify that the given hash is not part of an extension of the canon chain.
        let canonical_head = self.state.tree_state.canonical_head();
        let mut current_hash = target_hash;
        while let Some(current_block) = self.sealed_header_by_hash(current_hash)? {
            if current_block.hash() == canonical_head.hash {
                return Ok(false)
            }
            // We already passed the canonical head
            if current_block.number <= canonical_head.number {
                break
            }
            current_hash = current_block.parent_hash;
        }

        // verify that the given hash is not already part of canonical chain stored in memory
        if self.canonical_in_memory_state.header_by_hash(target_hash).is_some() {
            return Ok(false)
        }

        // verify that the given hash is not already part of persisted canonical chain
        if self.provider.block_number(target_hash)?.is_some() {
            return Ok(false)
        }

        Ok(true)
    }

    /// Invoked when we receive a new forkchoice update message. Calls into the blockchain tree
    /// to resolve chain forks and ensure that the Execution Layer is working with the latest valid
    /// chain.
    ///
    /// These responses should adhere to the [Engine API Spec for
    /// `engine_forkchoiceUpdated`](https://github.com/ethereum/execution-apis/blob/main/src/engine/paris.md#specification-1).
    ///
    /// Returns an error if an internal error occurred like a database error.
    #[instrument(level = "trace", skip_all, fields(head = % state.head_block_hash, safe = % state.safe_block_hash,finalized = % state.finalized_block_hash), target = "engine::tree")]
    fn on_forkchoice_updated(
        &mut self,
        state: ForkchoiceState,
        attrs: Option<T::PayloadAttributes>,
    ) -> ProviderResult<TreeOutcome<OnForkChoiceUpdated>> {
        trace!(target: "engine::tree", ?attrs, "invoked forkchoice update");
        self.metrics.engine.forkchoice_updated_messages.increment(1);
        self.canonical_in_memory_state.on_forkchoice_update_received();

        if let Some(on_updated) = self.pre_validate_forkchoice_update(state)? {
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
            trace!(target: "engine::tree", "fcu head hash is already canonical");

            // update the safe and finalized blocks and ensure their values are valid
            if let Err(outcome) = self.ensure_consistent_forkchoice_state(state) {
                // safe or finalized hashes are invalid
                return Ok(TreeOutcome::new(outcome))
            }

            // we still need to process payload attributes if the head is already canonical
            if let Some(attr) = attrs {
                let tip = self
                    .block_by_hash(self.state.tree_state.canonical_block_hash())?
                    .ok_or_else(|| {
                        // If we can't find the canonical block, then something is wrong and we need
                        // to return an error
                        ProviderError::HeaderNotFound(state.head_block_hash.into())
                    })?;
                let updated = self.process_payload_attributes(attr, &tip, state);
                return Ok(TreeOutcome::new(updated))
            }

            // the head block is already canonical
            return Ok(valid_outcome(state.head_block_hash))
        }

        // 2. ensure we can apply a new chain update for the head block
        if let Some(chain_update) = self.on_new_head(state.head_block_hash)? {
            let tip = chain_update.tip().header.clone();
            self.on_canonical_chain_update(chain_update);

            // update the safe and finalized blocks and ensure their values are valid
            if let Err(outcome) = self.ensure_consistent_forkchoice_state(state) {
                // safe or finalized hashes are invalid
                return Ok(TreeOutcome::new(outcome))
            }

            if let Some(attr) = attrs {
                let updated = self.process_payload_attributes(attr, &tip, state);
                return Ok(TreeOutcome::new(updated))
            }

            return Ok(valid_outcome(state.head_block_hash))
        }

        // 3. check if the head is already part of the canonical chain
        if let Ok(Some(canonical_header)) = self.find_canonical_header(state.head_block_hash) {
            debug!(target: "engine::tree", head = canonical_header.number, "fcu head block is already canonical");

            // For OpStack the proposers are allowed to reorg their own chain at will, so we need to
            // always trigger a new payload job if requested.
            if self.engine_kind.is_opstack() {
                if let Some(attr) = attrs {
                    debug!(target: "engine::tree", head = canonical_header.number, "handling payload attributes for canonical head");
                    let updated = self.process_payload_attributes(attr, &canonical_header, state);
                    return Ok(TreeOutcome::new(updated))
                }
            }

            // 2. Client software MAY skip an update of the forkchoice state and MUST NOT begin a
            //    payload build process if `forkchoiceState.headBlockHash` references a `VALID`
            //    ancestor of the head of canonical chain, i.e. the ancestor passed payload
            //    validation process and deemed `VALID`. In the case of such an event, client
            //    software MUST return `{payloadStatus: {status: VALID, latestValidHash:
            //    forkchoiceState.headBlockHash, validationError: null}, payloadId: null}`

            // the head block is already canonical, so we're not triggering a payload job and can
            // return right away
            return Ok(valid_outcome(state.head_block_hash))
        }

        // 4. we don't have the block to perform the update
        // we assume the FCU is valid and at least the head is missing,
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

    /// Attempts to receive the next engine request.
    ///
    /// If there's currently no persistence action in progress, this will block until a new request
    /// is received. If there's a persistence action in progress, this will try to receive the
    /// next request with a timeout to not block indefinitely and return `Ok(None)` if no request is
    /// received in time.
    ///
    /// Returns an error if the engine channel is disconnected.
    fn try_recv_engine_message(
        &self,
    ) -> Result<Option<FromEngine<EngineApiRequest<T>>>, RecvError> {
        if self.persistence_state.in_progress() {
            // try to receive the next request with a timeout to not block indefinitely
            match self.incoming.recv_timeout(std::time::Duration::from_millis(500)) {
                Ok(msg) => Ok(Some(msg)),
                Err(err) => match err {
                    RecvTimeoutError::Timeout => Ok(None),
                    RecvTimeoutError::Disconnected => Err(RecvError),
                },
            }
        } else {
            self.incoming.recv().map(Some)
        }
    }

    /// Attempts to advance the persistence state.
    ///
    /// If we're currently awaiting a response this will try to receive the response (non-blocking)
    /// or send a new persistence action if necessary.
    fn advance_persistence(&mut self) -> Result<(), AdvancePersistenceError> {
        if !self.persistence_state.in_progress() {
            if let Some(new_tip_num) = self.persistence_state.remove_above_state.pop_front() {
                debug!(target: "engine::tree", ?new_tip_num, remove_state=?self.persistence_state.remove_above_state, last_persisted_block_number=?self.persistence_state.last_persisted_block.number, "Removing blocks using persistence task");
                if new_tip_num < self.persistence_state.last_persisted_block.number {
                    debug!(target: "engine::tree", ?new_tip_num, "Starting remove blocks job");
                    let (tx, rx) = oneshot::channel();
                    let _ = self.persistence.remove_blocks_above(new_tip_num, tx);
                    self.persistence_state.start(rx);
                }
            } else if self.should_persist() {
                let blocks_to_persist = self.get_canonical_blocks_to_persist();
                if blocks_to_persist.is_empty() {
                    debug!(target: "engine::tree", "Returned empty set of blocks to persist");
                } else {
                    let (tx, rx) = oneshot::channel();
                    let _ = self.persistence.save_blocks(blocks_to_persist, tx);
                    self.persistence_state.start(rx);
                }
            }
        }

        if self.persistence_state.in_progress() {
            let (mut rx, start_time) = self
                .persistence_state
                .rx
                .take()
                .expect("if a persistence task is in progress Receiver must be Some");
            // Check if persistence has complete
            match rx.try_recv() {
                Ok(last_persisted_hash_num) => {
                    self.metrics.engine.persistence_duration.record(start_time.elapsed());
                    let Some(BlockNumHash {
                        hash: last_persisted_block_hash,
                        number: last_persisted_block_number,
                    }) = last_persisted_hash_num
                    else {
                        // if this happened, then we persisted no blocks because we sent an
                        // empty vec of blocks
                        warn!(target: "engine::tree", "Persistence task completed but did not persist any blocks");
                        return Ok(())
                    };

                    trace!(target: "engine::tree", ?last_persisted_block_hash, ?last_persisted_block_number, "Finished persisting, calling finish");
                    self.persistence_state
                        .finish(last_persisted_block_hash, last_persisted_block_number);
                    self.on_new_persisted_block()?;
                }
                Err(TryRecvError::Closed) => return Err(TryRecvError::Closed.into()),
                Err(TryRecvError::Empty) => self.persistence_state.rx = Some((rx, start_time)),
            }
        }
        Ok(())
    }

    /// Handles a message from the engine.
    fn on_engine_message(
        &mut self,
        msg: FromEngine<EngineApiRequest<T>>,
    ) -> Result<(), InsertBlockFatalError> {
        match msg {
            FromEngine::Event(event) => match event {
                FromOrchestrator::BackfillSyncStarted => {
                    debug!(target: "engine::tree", "received backfill sync started event");
                    self.backfill_sync_state = BackfillSyncState::Active;
                }
                FromOrchestrator::BackfillSyncFinished(ctrl) => {
                    self.on_backfill_sync_finished(ctrl)?;
                }
            },
            FromEngine::Request(request) => {
                match request {
                    EngineApiRequest::InsertExecutedBlock(block) => {
                        debug!(target: "engine::tree", block=?block.block().num_hash(), "inserting already executed block");
                        self.state.tree_state.insert_executed(block);
                        self.metrics.engine.inserted_already_executed_blocks.increment(1);
                    }
                    EngineApiRequest::Beacon(request) => {
                        match request {
                            BeaconEngineMessage::ForkchoiceUpdated { state, payload_attrs, tx } => {
                                let mut output = self.on_forkchoice_updated(state, payload_attrs);

                                if let Ok(res) = &mut output {
                                    // track last received forkchoice state
                                    self.state
                                        .forkchoice_state_tracker
                                        .set_latest(state, res.outcome.forkchoice_status());

                                    // emit an event about the handled FCU
                                    self.emit_event(BeaconConsensusEngineEvent::ForkchoiceUpdated(
                                        state,
                                        res.outcome.forkchoice_status(),
                                    ));

                                    // handle the event if any
                                    self.on_maybe_tree_event(res.event.take())?;
                                }

                                if let Err(err) =
                                    tx.send(output.map(|o| o.outcome).map_err(Into::into))
                                {
                                    self.metrics
                                        .engine
                                        .failed_forkchoice_updated_response_deliveries
                                        .increment(1);
                                    error!(target: "engine::tree", "Failed to send event: {err:?}");
                                }
                            }
                            BeaconEngineMessage::NewPayload { payload, cancun_fields, tx } => {
                                let output = self.on_new_payload(payload, cancun_fields);
                                if let Err(err) = tx.send(output.map(|o| o.outcome).map_err(|e| {
                                    reth_beacon_consensus::BeaconOnNewPayloadError::Internal(
                                        Box::new(e),
                                    )
                                })) {
                                    error!(target: "engine::tree", "Failed to send event: {err:?}");
                                    self.metrics
                                        .engine
                                        .failed_new_payload_response_deliveries
                                        .increment(1);
                                }
                            }
                            BeaconEngineMessage::TransitionConfigurationExchanged => {
                                // triggering this hook will record that we received a request from
                                // the CL
                                self.canonical_in_memory_state
                                    .on_transition_configuration_exchanged();
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
        Ok(())
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
    fn on_backfill_sync_finished(
        &mut self,
        ctrl: ControlFlow,
    ) -> Result<(), InsertBlockFatalError> {
        debug!(target: "engine::tree", "received backfill sync finished event");
        self.backfill_sync_state = BackfillSyncState::Idle;

        // Pipeline unwound, memorize the invalid block and wait for CL for next sync target.
        if let ControlFlow::Unwind { bad_block, .. } = ctrl {
            warn!(target: "engine::tree", invalid_hash=?bad_block.hash(), invalid_number=?bad_block.number, "Bad block detected in unwind");
            // update the `invalid_headers` cache with the new invalid header
            self.state.invalid_headers.insert(*bad_block);
            return Ok(())
        }

        // backfill height is the block number that the backfill finished at
        let Some(backfill_height) = ctrl.block_number() else { return Ok(()) };

        // state house keeping after backfill sync
        // remove all executed blocks below the backfill height
        //
        // We set the `finalized_num` to `Some(backfill_height)` to ensure we remove all state
        // before that
        let backfill_num_hash = self
            .provider
            .block_hash(backfill_height)?
            .map(|hash| BlockNumHash { hash, number: backfill_height });

        self.state.tree_state.remove_until(
            backfill_num_hash
                .expect("after backfill the block target hash should be present in the db"),
            self.persistence_state.last_persisted_block.hash,
            backfill_num_hash,
        );
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
            self.persistence_state.finish(new_head.hash(), new_head.number);

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
            .map(|block| block.number);

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
    fn emit_event(&mut self, event: impl Into<EngineApiEvent>) {
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
    const fn should_persist(&self) -> bool {
        if !self.backfill_sync_state.is_idle() {
            // can't persist if backfill is running
            return false
        }

        let min_block = self.persistence_state.last_persisted_block.number;
        self.state.tree_state.canonical_block_number().saturating_sub(min_block) >
            self.config.persistence_threshold()
    }

    /// Returns a batch of consecutive canonical blocks to persist in the range
    /// `(last_persisted_number .. canonical_head - threshold]` . The expected
    /// order is oldest -> newest.
    fn get_canonical_blocks_to_persist(&self) -> Vec<ExecutedBlock> {
        let mut blocks_to_persist = Vec::new();
        let mut current_hash = self.state.tree_state.canonical_block_hash();
        let last_persisted_number = self.persistence_state.last_persisted_block.number;

        let canonical_head_number = self.state.tree_state.canonical_block_number();

        let target_number =
            canonical_head_number.saturating_sub(self.config.memory_block_buffer_target());

        debug!(target: "engine::tree", ?last_persisted_number, ?canonical_head_number, ?target_number, ?current_hash, "Returning canonical blocks to persist");
        while let Some(block) = self.state.tree_state.blocks_by_hash.get(&current_hash) {
            if block.block.number <= last_persisted_number {
                break;
            }

            if block.block.number <= target_number {
                blocks_to_persist.push(block.clone());
            }

            current_hash = block.block.parent_hash;
        }

        // reverse the order so that the oldest block comes first
        blocks_to_persist.reverse();

        blocks_to_persist
    }

    /// This clears the blocks from the in-memory tree state that have been persisted to the
    /// database.
    ///
    /// This also updates the canonical in-memory state to reflect the newest persisted block
    /// height.
    ///
    /// Assumes that `finish` has been called on the `persistence_state` at least once
    fn on_new_persisted_block(&mut self) -> ProviderResult<()> {
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
    /// NOTE: This cannot fetch [`ExecutedBlock`]s for _finalized_ blocks, instead it can only
    /// fetch [`ExecutedBlock`]s for _canonical_ blocks, or blocks from sidechains that the node
    /// has in memory.
    ///
    /// For finalized blocks, this will return `None`.
    fn executed_block_by_hash(&self, hash: B256) -> ProviderResult<Option<ExecutedBlock>> {
        trace!(target: "engine::tree", ?hash, "Fetching executed block by hash");
        // check memory first
        let block = self.state.tree_state.executed_block_by_hash(hash).cloned();

        if block.is_some() {
            return Ok(block)
        }

        let Some((_, updates)) = self.state.tree_state.persisted_trie_updates.get(&hash) else {
            return Ok(None)
        };

        let SealedBlockWithSenders { block, senders } = self
            .provider
            .sealed_block_with_senders(hash.into(), TransactionVariant::WithHash)?
            .ok_or_else(|| ProviderError::HeaderNotFound(hash.into()))?;
        let execution_output = self
            .provider
            .get_state(block.number)?
            .ok_or_else(|| ProviderError::StateForNumberNotFound(block.number))?;
        let hashed_state = execution_output.hash_state_slow();

        Ok(Some(ExecutedBlock {
            block: Arc::new(block),
            senders: Arc::new(senders),
            trie: updates.clone(),
            execution_output: Arc::new(execution_output),
            hashed_state: Arc::new(hashed_state),
        }))
    }

    /// Return sealed block from database or in-memory state by hash.
    fn sealed_header_by_hash(&self, hash: B256) -> ProviderResult<Option<SealedHeader>> {
        // check memory first
        let block =
            self.state.tree_state.block_by_hash(hash).map(|block| block.as_ref().clone().header);

        if block.is_some() {
            Ok(block)
        } else {
            self.provider.sealed_header_by_hash(hash)
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

    /// Returns the state provider for the requested block hash.
    ///
    /// This merges the state of all blocks that are part of the chain that the requested block is
    /// the head of and are not yet persisted on disk. This includes all blocks that connect back to
    /// a canonical block on disk.
    ///
    /// Returns `None` if the state for the requested hash is not found, this happens if the
    /// requested state belongs to a block that is not connected to the canonical chain.
    ///
    /// Returns an error if we failed to fetch the state from the database.
    fn state_provider(&self, hash: B256) -> ProviderResult<Option<StateProviderBox>> {
        if let Some((historical, blocks)) = self.state.tree_state.blocks_by_hash(hash) {
            trace!(target: "engine::tree", %hash, "found canonical state for block in memory");
            // the block leads back to the canonical chain
            let historical = self.provider.state_by_block_hash(historical)?;
            return Ok(Some(Box::new(MemoryOverlayStateProvider::new(historical, blocks))))
        }

        // the hash could belong to an unknown block or a persisted block
        if let Some(header) = self.provider.header(&hash)? {
            trace!(target: "engine::tree", %hash, number = %header.number, "found canonical state for block in database");
            // the block is known and persisted
            let historical = self.provider.state_by_block_hash(hash)?;
            return Ok(Some(historical))
        }

        trace!(target: "engine::tree", %hash, "no canonical state found for block");

        Ok(None)
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
                target: "engine::tree",
                ?block,
                "Failed to validate total difficulty for block {}: {e}",
                block.header.hash()
            );
            return Err(e)
        }

        if let Err(e) = self.consensus.validate_header(block) {
            error!(target: "engine::tree", ?block, "Failed to validate header {}: {e}", block.header.hash());
            return Err(e)
        }

        if let Err(e) = self.consensus.validate_block_pre_execution(block) {
            error!(target: "engine::tree", ?block, "Failed to validate block {}: {e}", block.header.hash());
            return Err(e)
        }

        Ok(())
    }

    /// Attempts to connect any buffered blocks that are connected to the given parent hash.
    #[instrument(level = "trace", skip(self), target = "engine::tree")]
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
                    if self.is_sync_target_head(child_num_hash.hash) &&
                        matches!(res, InsertPayloadOk2::Inserted(BlockStatus2::Valid))
                    {
                        self.make_canonical(child_num_hash.hash)?;
                    }
                }
                Err(err) => {
                    debug!(target: "engine::tree", ?err, "failed to connect buffered block to tree");
                    if let Err(fatal) = self.on_insert_block_error(err) {
                        warn!(target: "engine::tree", %fatal, "fatal error occurred while connecting buffered blocks");
                        return Err(fatal)
                    }
                }
            }
        }

        debug!(target: "engine::tree", elapsed = ?now.elapsed(), %block_count, "connected buffered blocks");
        Ok(())
    }

    /// Attempts to recover the block's senders and then buffers it.
    ///
    /// Returns an error if sender recovery failed or inserting into the buffer failed.
    fn buffer_block_without_senders(
        &mut self,
        block: SealedBlock,
    ) -> Result<(), InsertBlockErrorTwo> {
        match block.try_seal_with_senders() {
            Ok(block) => self.buffer_block(block),
            Err(block) => Err(InsertBlockErrorTwo::sender_recovery_error(block)),
        }
    }

    /// Pre-validates the block and inserts it into the buffer.
    fn buffer_block(&mut self, block: SealedBlockWithSenders) -> Result<(), InsertBlockErrorTwo> {
        if let Err(err) = self.validate_block(&block) {
            return Err(InsertBlockErrorTwo::consensus_error(err, block.block))
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
        }

        None
    }

    /// This determines whether or not we should remove blocks from the chain, based on a canonical
    /// chain update.
    ///
    /// If the chain update is a reorg:
    /// * is the new chain behind the last persisted block, or
    /// * if the root of the new chain is at the same height as the last persisted block, is it a
    ///   different block
    ///
    /// If either of these are true, then this returns the height of the first block. Otherwise,
    /// this returns [`None`]. This should be used to check whether or not we should be sending a
    /// remove command to the persistence task.
    fn find_disk_reorg(&self, chain_update: &NewCanonicalChain) -> Option<u64> {
        let NewCanonicalChain::Reorg { new, old: _ } = chain_update else { return None };

        let BlockNumHash { number: new_num, hash: new_hash } =
            new.first().map(|block| block.block.num_hash())?;

        match new_num.cmp(&self.persistence_state.last_persisted_block.number) {
            Ordering::Greater => {
                // new number is above the last persisted block so the reorg can be performed
                // entirely in memory
                None
            }
            Ordering::Equal => {
                // new number is the same, if the hash is the same then we should not need to remove
                // any blocks
                (self.persistence_state.last_persisted_block.hash != new_hash).then_some(new_num)
            }
            Ordering::Less => {
                // this means we are below the last persisted block and must remove on disk blocks
                Some(new_num)
            }
        }
    }

    /// Invoked when we the canonical chain has been updated.
    ///
    /// This is invoked on a valid forkchoice update, or if we can make the target block canonical.
    fn on_canonical_chain_update(&mut self, chain_update: NewCanonicalChain) {
        trace!(target: "engine::tree", new_blocks = %chain_update.new_block_count(), reorged_blocks =  %chain_update.reorged_block_count(), "applying new chain update");
        let start = Instant::now();

        // schedule a remove_above call if we have an on-disk reorg
        if let Some(height) = self.find_disk_reorg(&chain_update) {
            // calculate the new tip by subtracting one from the lowest part of the chain
            let new_tip_num = height.saturating_sub(1);
            self.persistence_state.schedule_removal(new_tip_num);
        }

        // update the tracked canonical head
        self.state.tree_state.set_canonical_head(chain_update.tip().num_hash());

        let tip = chain_update.tip().header.clone();
        let notification = chain_update.to_chain_notification();

        // reinsert any missing reorged blocks
        if let NewCanonicalChain::Reorg { new, old } = &chain_update {
            let new_first = new.first().map(|first| first.block.num_hash());
            let old_first = old.first().map(|first| first.block.num_hash());
            trace!(target: "engine::tree", ?new_first, ?old_first, "Reorg detected, new and old first blocks");

            self.update_reorg_metrics(old.len());
            self.reinsert_reorged_blocks(new.clone());
            self.reinsert_reorged_blocks(old.clone());
        }

        // update the tracked in-memory state with the new chain
        self.canonical_in_memory_state.update_chain(chain_update);
        self.canonical_in_memory_state.set_canonical_head(tip.clone());

        // Update metrics based on new tip
        self.metrics.tree.canonical_chain_height.set(tip.number as f64);

        // sends an event to all active listeners about the new canonical chain
        self.canonical_in_memory_state.notify_canon_state(notification);

        // emit event
        self.emit_event(BeaconConsensusEngineEvent::CanonicalChainCommitted(
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
    fn reinsert_reorged_blocks(&mut self, new_chain: Vec<ExecutedBlock>) {
        for block in new_chain {
            if self.state.tree_state.executed_block_by_hash(block.block.hash()).is_none() {
                trace!(target: "engine::tree", num=?block.block.number, hash=?block.block.hash(), "Reinserting block into tree state");
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
    #[instrument(level = "trace", skip_all, fields(block_hash = %block.hash(), block_num = %block.number,), target = "engine::tree")]
    fn on_downloaded_block(
        &mut self,
        block: SealedBlockWithSenders,
    ) -> Result<Option<TreeEvent>, InsertBlockFatalError> {
        let block_num_hash = block.num_hash();
        let lowest_buffered_ancestor = self.lowest_buffered_ancestor_or(block_num_hash.hash);
        if self
            .check_invalid_ancestor_with_head(lowest_buffered_ancestor, block_num_hash.hash)?
            .is_some()
        {
            return Ok(None)
        }

        if !self.backfill_sync_state.is_idle() {
            return Ok(None)
        }

        // try to append the block
        match self.insert_block(block) {
            Ok(InsertPayloadOk2::Inserted(BlockStatus2::Valid)) => {
                if self.is_sync_target_head(block_num_hash.hash) {
                    trace!(target: "engine::tree", "appended downloaded sync target block");

                    // we just inserted the current sync target block, we can try to make it
                    // canonical
                    return Ok(Some(TreeEvent::TreeAction(TreeAction::MakeCanonical {
                        sync_target_head: block_num_hash.hash,
                    })))
                }
                trace!(target: "engine::tree", "appended downloaded block");
                self.try_connect_buffered_blocks(block_num_hash)?;
            }
            Ok(InsertPayloadOk2::Inserted(BlockStatus2::Disconnected {
                head,
                missing_ancestor,
            })) => {
                // block is not connected to the canonical head, we need to download
                // its missing branch first
                return Ok(self.on_disconnected_downloaded_block(
                    block_num_hash,
                    missing_ancestor,
                    head,
                ))
            }
            Ok(InsertPayloadOk2::AlreadySeen(_)) => {
                trace!(target: "engine::tree", "downloaded block already executed");
            }
            Err(err) => {
                debug!(target: "engine::tree", err=%err.kind(), "failed to insert downloaded block");
                if let Err(fatal) = self.on_insert_block_error(err) {
                    warn!(target: "engine::tree", %fatal, "fatal error occurred while inserting downloaded block");
                    return Err(fatal)
                }
            }
        }
        Ok(None)
    }

    fn insert_block_without_senders(
        &mut self,
        block: SealedBlock,
    ) -> Result<InsertPayloadOk2, InsertBlockErrorTwo> {
        match block.try_seal_with_senders() {
            Ok(block) => self.insert_block(block),
            Err(block) => Err(InsertBlockErrorTwo::sender_recovery_error(block)),
        }
    }

    fn insert_block(
        &mut self,
        block: SealedBlockWithSenders,
    ) -> Result<InsertPayloadOk2, InsertBlockErrorTwo> {
        self.insert_block_inner(block.clone())
            .map_err(|kind| InsertBlockErrorTwo::new(block.block, kind))
    }

    fn insert_block_inner(
        &mut self,
        block: SealedBlockWithSenders,
    ) -> Result<InsertPayloadOk2, InsertBlockErrorKindTwo> {
        debug!(target: "engine::tree", block=?block.num_hash(), "Inserting new block into tree");
        if self.block_by_hash(block.hash())?.is_some() {
            return Ok(InsertPayloadOk2::AlreadySeen(BlockStatus2::Valid))
        }

        let start = Instant::now();

        trace!(target: "engine::tree", block=?block.num_hash(), "Validating block consensus");
        // validate block consensus rules
        self.validate_block(&block)?;

        trace!(target: "engine::tree", block=?block.num_hash(), parent=?block.parent_hash, "Fetching block state provider");
        let Some(state_provider) = self.state_provider(block.parent_hash)? else {
            // we don't have the state required to execute this block, buffering it and find the
            // missing parent block
            let missing_ancestor = self
                .state
                .buffer
                .lowest_ancestor(&block.parent_hash)
                .map(|block| block.parent_num_hash())
                .unwrap_or_else(|| block.parent_num_hash());

            self.state.buffer.insert_block(block);

            return Ok(InsertPayloadOk2::Inserted(BlockStatus2::Disconnected {
                head: self.state.tree_state.current_canonical_head,
                missing_ancestor,
            }))
        };

        // now validate against the parent
        let parent_block = self.sealed_header_by_hash(block.parent_hash)?.ok_or_else(|| {
            InsertBlockErrorKindTwo::Provider(ProviderError::HeaderNotFound(
                block.parent_hash.into(),
            ))
        })?;
        if let Err(e) = self.consensus.validate_header_against_parent(&block, &parent_block) {
            warn!(target: "engine::tree", ?block, "Failed to validate header {} against parent: {e}", block.header.hash());
            return Err(e.into())
        }

        trace!(target: "engine::tree", block=?block.num_hash(), "Executing block");
        let executor = self.executor_provider.executor(StateProviderDatabase::new(&state_provider));

        let block_number = block.number;
        let block_hash = block.hash();
        let sealed_block = Arc::new(block.block.clone());
        let block = block.unseal();

        let exec_time = Instant::now();
        let output = self.metrics.executor.execute_metered(executor, (&block, U256::MAX).into())?;

        trace!(target: "engine::tree", elapsed=?exec_time.elapsed(), ?block_number, "Executed block");
        if let Err(err) = self.consensus.validate_block_post_execution(
            &block,
            PostExecutionInput::new(&output.receipts, &output.requests),
        ) {
            // call post-block hook
            self.invalid_block_hook.on_invalid_block(
                &parent_block,
                &block.seal_slow(),
                &output,
                None,
            );
            return Err(err.into())
        }

        let hashed_state = HashedPostState::from_bundle_state(&output.state.state);

        trace!(target: "engine::tree", block=?BlockNumHash::new(block_number, block_hash), "Calculating block state root");
        let root_time = Instant::now();
        let mut state_root_result = None;

        // We attempt to compute state root in parallel if we are currently not persisting anything
        // to database. This is safe, because the database state cannot change until we
        // finish parallel computation. It is important that nothing is being persisted as
        // we are computing in parallel, because we initialize a different database transaction
        // per thread and it might end up with a different view of the database.
        let persistence_in_progress = self.persistence_state.in_progress();
        if !persistence_in_progress {
            state_root_result = match self
                .compute_state_root_parallel(block.parent_hash, &hashed_state)
            {
                Ok((state_root, trie_output)) => Some((state_root, trie_output)),
                Err(ParallelStateRootError::Provider(ProviderError::ConsistentView(error))) => {
                    debug!(target: "engine", %error, "Parallel state root computation failed consistency check, falling back");
                    None
                }
                Err(error) => return Err(InsertBlockErrorKindTwo::Other(Box::new(error))),
            };
        }

        let (state_root, trie_output) = if let Some(result) = state_root_result {
            result
        } else {
            debug!(target: "engine::tree", persistence_in_progress, "Failed to compute state root in parallel");
            state_provider.state_root_with_updates(hashed_state.clone())?
        };

        if state_root != block.state_root {
            // call post-block hook
            self.invalid_block_hook.on_invalid_block(
                &parent_block,
                &block.clone().seal_slow(),
                &output,
                Some((&trie_output, state_root)),
            );
            return Err(ConsensusError::BodyStateRootDiff(
                GotExpected { got: state_root, expected: block.state_root }.into(),
            )
            .into())
        }

        let root_elapsed = root_time.elapsed();
        self.metrics.block_validation.record_state_root(&trie_output, root_elapsed.as_secs_f64());
        debug!(target: "engine::tree", ?root_elapsed, ?block_number, "Calculated state root");

        let executed = ExecutedBlock {
            block: sealed_block.clone(),
            senders: Arc::new(block.senders),
            execution_output: Arc::new(ExecutionOutcome::from((output, block_number))),
            hashed_state: Arc::new(hashed_state),
            trie: Arc::new(trie_output),
        };

        if self.state.tree_state.canonical_block_hash() == executed.block().parent_hash {
            debug!(target: "engine::tree", pending = ?executed.block().num_hash() ,"updating pending block");
            // if the parent is the canonical head, we can insert the block as the pending block
            self.canonical_in_memory_state.set_pending_block(executed.clone());
        }

        self.state.tree_state.insert_executed(executed);
        self.metrics.engine.executed_blocks.set(self.state.tree_state.block_count() as f64);

        // emit insert event
        let elapsed = start.elapsed();
        let engine_event = if self.is_fork(block_hash)? {
            BeaconConsensusEngineEvent::ForkBlockAdded(sealed_block, elapsed)
        } else {
            BeaconConsensusEngineEvent::CanonicalBlockAdded(sealed_block, elapsed)
        };
        self.emit_event(EngineApiEvent::BeaconConsensus(engine_event));

        debug!(target: "engine::tree", block=?BlockNumHash::new(block_number, block_hash), "Finished inserting block");
        Ok(InsertPayloadOk2::Inserted(BlockStatus2::Valid))
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
        parent_hash: B256,
        hashed_state: &HashedPostState,
    ) -> Result<(B256, TrieUpdates), ParallelStateRootError> {
        let consistent_view = ConsistentDbView::new_with_latest_tip(self.provider.clone())?;
        let mut input = TrieInput::default();

        if let Some((historical, blocks)) = self.state.tree_state.blocks_by_hash(parent_hash) {
            // Retrieve revert state for historical block.
            let revert_state = consistent_view.revert_state(historical)?;
            input.append(revert_state);

            // Extend with contents of parent in-memory blocks.
            for block in blocks.iter().rev() {
                input.append_cached_ref(block.trie_updates(), block.hashed_state())
            }
        } else {
            // The block attaches to canonical persisted parent.
            let revert_state = consistent_view.revert_state(parent_hash)?;
            input.append(revert_state);
        }

        // Extend with block we are validating root for.
        input.append_ref(hashed_state);

        ParallelStateRoot::new(consistent_view, input).incremental_root_with_updates()
    }

    /// Handles an error that occurred while inserting a block.
    ///
    /// If this is a validation error this will mark the block as invalid.
    ///
    /// Returns the proper payload status response if the block is invalid.
    fn on_insert_block_error(
        &mut self,
        error: InsertBlockErrorTwo,
    ) -> Result<PayloadStatus, InsertBlockFatalError> {
        let (block, error) = error.split();

        // if invalid block, we check the validation error. Otherwise return the fatal
        // error.
        let validation_err = error.ensure_validation_error()?;

        // If the error was due to an invalid payload, the payload is added to the
        // invalid headers cache and `Ok` with [PayloadStatusEnum::Invalid] is
        // returned.
        warn!(target: "engine::tree", invalid_hash=?block.hash(), invalid_number=?block.number, %validation_err, "Invalid block error on new payload");
        let latest_valid_hash = if validation_err.is_block_pre_merge() {
            // zero hash must be returned if block is pre-merge
            Some(B256::ZERO)
        } else {
            self.latest_valid_hash_for_invalid_payload(block.parent_hash)?
        };

        // keep track of the invalid header
        self.state.invalid_headers.insert(block.header);
        Ok(PayloadStatus::new(
            PayloadStatusEnum::Invalid { validation_error: validation_err.to_string() },
            latest_valid_hash,
        ))
    }

    /// Attempts to find the header for the given block hash if it is canonical.
    pub fn find_canonical_header(&self, hash: B256) -> Result<Option<SealedHeader>, ProviderError> {
        let mut canonical = self.canonical_in_memory_state.header_by_hash(hash);

        if canonical.is_none() {
            canonical = self.provider.header(&hash)?.map(|header| SealedHeader::new(header, hash));
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
                    let _ = self.persistence.save_finalized_block_number(finalized.number);
                    self.canonical_in_memory_state.set_finalized(finalized);
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
                    let _ = self.persistence.save_safe_block_number(safe.number);
                    self.canonical_in_memory_state.set_safe(safe);
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

        if !self.backfill_sync_state.is_idle() {
            // We can only process new forkchoice updates if the pipeline is idle, since it requires
            // exclusive access to the database
            trace!(target: "engine::tree", "Pipeline is syncing, skipping forkchoice update");
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
}

/// This is an error that can come from advancing persistence. Either this can be a
/// [`TryRecvError`], or this can be a [`ProviderError`]
#[derive(Debug, thiserror::Error)]
pub enum AdvancePersistenceError {
    /// An error that can be from failing to receive a value from persistence
    #[error(transparent)]
    RecvError(#[from] TryRecvError),
    /// A provider error
    #[error(transparent)]
    Provider(#[from] ProviderError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persistence::PersistenceAction;
    use alloy_primitives::{Bytes, Sealable};
    use alloy_rlp::Decodable;
    use assert_matches::assert_matches;
    use reth_beacon_consensus::{EthBeaconConsensus, ForkchoiceStatus};
    use reth_chain_state::{test_utils::TestBlockBuilder, BlockState};
    use reth_chainspec::{ChainSpec, HOLESKY, MAINNET};
    use reth_ethereum_engine_primitives::EthEngineTypes;
    use reth_evm::test_utils::MockExecutorProvider;
    use reth_provider::test_utils::MockEthProvider;
    use reth_rpc_types_compat::engine::{block_to_payload_v1, payload::block_to_payload_v3};
    use reth_trie::updates::TrieUpdates;
    use std::{
        str::FromStr,
        sync::mpsc::{channel, Sender},
    };
    use tokio::sync::mpsc::unbounded_channel;

    /// This is a test channel that allows you to `release` any value that is in the channel.
    ///
    /// If nothing has been sent, then the next value will be immediately sent.
    #[allow(dead_code)]
    struct TestChannel<T> {
        /// If an item is sent to this channel, an item will be released in the wrapped channel
        release: Receiver<()>,
        /// The sender channel
        tx: Sender<T>,
        /// The receiver channel
        rx: Receiver<T>,
    }

    impl<T: Send + 'static> TestChannel<T> {
        /// Creates a new test channel
        #[allow(dead_code)]
        fn spawn_channel() -> (Sender<T>, Receiver<T>, TestChannelHandle) {
            let (original_tx, original_rx) = channel();
            let (wrapped_tx, wrapped_rx) = channel();
            let (release_tx, release_rx) = channel();
            let handle = TestChannelHandle::new(release_tx);
            let test_channel = Self { release: release_rx, tx: wrapped_tx, rx: original_rx };
            // spawn the task that listens and releases stuff
            std::thread::spawn(move || test_channel.intercept_loop());
            (original_tx, wrapped_rx, handle)
        }

        /// Runs the intercept loop, waiting for the handle to release a value
        fn intercept_loop(&self) {
            while self.release.recv() == Ok(()) {
                let Ok(value) = self.rx.recv() else { return };

                let _ = self.tx.send(value);
            }
        }
    }

    struct TestChannelHandle {
        /// The sender to use for releasing values
        release: Sender<()>,
    }

    impl TestChannelHandle {
        /// Returns a [`TestChannelHandle`]
        const fn new(release: Sender<()>) -> Self {
            Self { release }
        }

        /// Signals to the channel task that a value should be released
        #[allow(dead_code)]
        fn release(&self) {
            let _ = self.release.send(());
        }
    }

    struct TestHarness {
        tree:
            EngineApiTreeHandler<MockEthProvider, MockExecutorProvider, EthEngineTypes, ChainSpec>,
        to_tree_tx: Sender<FromEngine<EngineApiRequest<EthEngineTypes>>>,
        from_tree_rx: UnboundedReceiver<EngineApiEvent>,
        blocks: Vec<ExecutedBlock>,
        action_rx: Receiver<PersistenceAction>,
        executor_provider: MockExecutorProvider,
        block_builder: TestBlockBuilder,
        provider: MockEthProvider,
    }

    impl TestHarness {
        fn new(chain_spec: Arc<ChainSpec>) -> Self {
            let (action_tx, action_rx) = channel();
            Self::with_persistence_channel(chain_spec, action_tx, action_rx)
        }

        #[allow(dead_code)]
        fn with_test_channel(chain_spec: Arc<ChainSpec>) -> (Self, TestChannelHandle) {
            let (action_tx, action_rx, handle) = TestChannel::spawn_channel();
            (Self::with_persistence_channel(chain_spec, action_tx, action_rx), handle)
        }

        fn with_persistence_channel(
            chain_spec: Arc<ChainSpec>,
            action_tx: Sender<PersistenceAction>,
            action_rx: Receiver<PersistenceAction>,
        ) -> Self {
            let persistence_handle = PersistenceHandle::new(action_tx);

            let consensus = Arc::new(EthBeaconConsensus::new(chain_spec.clone()));

            let provider = MockEthProvider::default();
            let executor_provider = MockExecutorProvider::default();

            let payload_validator = ExecutionPayloadValidator::new(chain_spec.clone());

            let (from_tree_tx, from_tree_rx) = unbounded_channel();

            let sealed = chain_spec.genesis_header().clone().seal_slow();
            let (header, seal) = sealed.into_parts();
            let header = SealedHeader::new(header, seal);
            let engine_api_tree_state = EngineApiTreeState::new(10, 10, header.num_hash());
            let canonical_in_memory_state = CanonicalInMemoryState::with_head(header, None, None);

            let (to_payload_service, _payload_command_rx) = unbounded_channel();
            let payload_builder = PayloadBuilderHandle::new(to_payload_service);

            let tree = EngineApiTreeHandler::new(
                provider.clone(),
                executor_provider.clone(),
                consensus,
                payload_validator,
                from_tree_tx,
                engine_api_tree_state,
                canonical_in_memory_state,
                persistence_handle,
                PersistenceState::default(),
                payload_builder,
                TreeConfig::default(),
                EngineApiKind::Ethereum,
            );

            let block_builder = TestBlockBuilder::default().with_chain_spec((*chain_spec).clone());
            Self {
                to_tree_tx: tree.incoming_tx.clone(),
                tree,
                from_tree_rx,
                blocks: vec![],
                action_rx,
                executor_provider,
                block_builder,
                provider,
            }
        }

        fn with_blocks(mut self, blocks: Vec<ExecutedBlock>) -> Self {
            let mut blocks_by_hash = HashMap::default();
            let mut blocks_by_number = BTreeMap::new();
            let mut state_by_hash = HashMap::default();
            let mut hash_by_number = BTreeMap::new();
            let mut parent_to_child: HashMap<B256, HashSet<B256>> = HashMap::default();
            let mut parent_hash = B256::ZERO;

            for block in &blocks {
                let sealed_block = block.block();
                let hash = sealed_block.hash();
                let number = sealed_block.number;
                blocks_by_hash.insert(hash, block.clone());
                blocks_by_number.entry(number).or_insert_with(Vec::new).push(block.clone());
                state_by_hash.insert(hash, Arc::new(BlockState::new(block.clone())));
                hash_by_number.insert(number, hash);
                parent_to_child.entry(parent_hash).or_default().insert(hash);
                parent_hash = hash;
            }

            self.tree.state.tree_state = TreeState {
                blocks_by_hash,
                blocks_by_number,
                current_canonical_head: blocks.last().unwrap().block().num_hash(),
                parent_to_child,
                persisted_trie_updates: HashMap::default(),
            };

            let last_executed_block = blocks.last().unwrap().clone();
            let pending = Some(BlockState::new(last_executed_block));
            self.tree.canonical_in_memory_state =
                CanonicalInMemoryState::new(state_by_hash, hash_by_number, pending, None, None);

            self.blocks = blocks.clone();
            self.persist_blocks(
                blocks
                    .into_iter()
                    .map(|b| SealedBlockWithSenders {
                        block: (*b.block).clone(),
                        senders: b.senders.to_vec(),
                    })
                    .collect(),
            );

            self
        }

        const fn with_backfill_state(mut self, state: BackfillSyncState) -> Self {
            self.tree.backfill_sync_state = state;
            self
        }

        fn extend_execution_outcome(
            &self,
            execution_outcomes: impl IntoIterator<Item = impl Into<ExecutionOutcome>>,
        ) {
            self.executor_provider.extend(execution_outcomes);
        }

        fn insert_block(
            &mut self,
            block: SealedBlockWithSenders,
        ) -> Result<InsertPayloadOk2, InsertBlockErrorTwo> {
            let execution_outcome = self.block_builder.get_execution_outcome(block.clone());
            self.extend_execution_outcome([execution_outcome]);
            self.tree.provider.add_state_root(block.state_root);
            self.tree.insert_block(block)
        }

        async fn fcu_to(&mut self, block_hash: B256, fcu_status: impl Into<ForkchoiceStatus>) {
            let fcu_status = fcu_status.into();

            self.send_fcu(block_hash, fcu_status).await;

            self.check_fcu(block_hash, fcu_status).await;
        }

        async fn send_fcu(&mut self, block_hash: B256, fcu_status: impl Into<ForkchoiceStatus>) {
            let fcu_state = self.fcu_state(block_hash);

            let (tx, rx) = oneshot::channel();
            self.tree
                .on_engine_message(FromEngine::Request(
                    BeaconEngineMessage::ForkchoiceUpdated {
                        state: fcu_state,
                        payload_attrs: None,
                        tx,
                    }
                    .into(),
                ))
                .unwrap();

            let response = rx.await.unwrap().unwrap().await.unwrap();
            match fcu_status.into() {
                ForkchoiceStatus::Valid => assert!(response.payload_status.is_valid()),
                ForkchoiceStatus::Syncing => assert!(response.payload_status.is_syncing()),
                ForkchoiceStatus::Invalid => assert!(response.payload_status.is_invalid()),
            }
        }

        async fn check_fcu(&mut self, block_hash: B256, fcu_status: impl Into<ForkchoiceStatus>) {
            let fcu_state = self.fcu_state(block_hash);

            // check for ForkchoiceUpdated event
            let event = self.from_tree_rx.recv().await.unwrap();
            match event {
                EngineApiEvent::BeaconConsensus(BeaconConsensusEngineEvent::ForkchoiceUpdated(
                    state,
                    status,
                )) => {
                    assert_eq!(state, fcu_state);
                    assert_eq!(status, fcu_status.into());
                }
                _ => panic!("Unexpected event: {:#?}", event),
            }
        }

        const fn fcu_state(&self, block_hash: B256) -> ForkchoiceState {
            ForkchoiceState {
                head_block_hash: block_hash,
                safe_block_hash: block_hash,
                finalized_block_hash: block_hash,
            }
        }

        async fn send_new_payload(&mut self, block: SealedBlockWithSenders) {
            let payload = block_to_payload_v3(block.block.clone());
            self.tree
                .on_new_payload(
                    payload.into(),
                    Some(CancunPayloadFields {
                        parent_beacon_block_root: block.parent_beacon_block_root.unwrap(),
                        versioned_hashes: vec![],
                    }),
                )
                .unwrap();
        }

        async fn insert_chain(
            &mut self,
            chain: impl IntoIterator<Item = SealedBlockWithSenders> + Clone,
        ) {
            for block in chain.clone() {
                self.insert_block(block.clone()).unwrap();
            }
            self.check_canon_chain_insertion(chain).await;
        }

        async fn check_canon_commit(&mut self, hash: B256) {
            let event = self.from_tree_rx.recv().await.unwrap();
            match event {
                EngineApiEvent::BeaconConsensus(
                    BeaconConsensusEngineEvent::CanonicalChainCommitted(header, _),
                ) => {
                    assert_eq!(header.hash(), hash);
                }
                _ => panic!("Unexpected event: {:#?}", event),
            }
        }

        async fn check_fork_chain_insertion(
            &mut self,
            chain: impl IntoIterator<Item = SealedBlockWithSenders> + Clone,
        ) {
            for block in chain {
                self.check_fork_block_added(block.block.hash()).await;
            }
        }

        async fn check_canon_chain_insertion(
            &mut self,
            chain: impl IntoIterator<Item = SealedBlockWithSenders> + Clone,
        ) {
            for block in chain.clone() {
                self.check_canon_block_added(block.hash()).await;
            }
        }

        async fn check_canon_block_added(&mut self, expected_hash: B256) {
            let event = self.from_tree_rx.recv().await.unwrap();
            match event {
                EngineApiEvent::BeaconConsensus(
                    BeaconConsensusEngineEvent::CanonicalBlockAdded(block, _),
                ) => {
                    assert!(block.hash() == expected_hash);
                }
                _ => panic!("Unexpected event: {:#?}", event),
            }
        }

        async fn check_fork_block_added(&mut self, expected_hash: B256) {
            let event = self.from_tree_rx.recv().await.unwrap();
            match event {
                EngineApiEvent::BeaconConsensus(BeaconConsensusEngineEvent::ForkBlockAdded(
                    block,
                    _,
                )) => {
                    assert!(block.hash() == expected_hash);
                }
                _ => panic!("Unexpected event: {:#?}", event),
            }
        }

        fn persist_blocks(&self, blocks: Vec<SealedBlockWithSenders>) {
            let mut block_data: Vec<(B256, Block)> = Vec::with_capacity(blocks.len());
            let mut headers_data: Vec<(B256, Header)> = Vec::with_capacity(blocks.len());

            for block in &blocks {
                let unsealed_block = block.clone().unseal();
                block_data.push((block.hash(), unsealed_block.clone().block));
                headers_data.push((block.hash(), unsealed_block.header.clone()));
            }

            self.provider.extend_blocks(block_data);
            self.provider.extend_headers(headers_data);
        }

        fn setup_range_insertion_for_valid_chain(&mut self, chain: Vec<SealedBlockWithSenders>) {
            self.setup_range_insertion_for_chain(chain, None)
        }

        fn setup_range_insertion_for_invalid_chain(
            &mut self,
            chain: Vec<SealedBlockWithSenders>,
            index: usize,
        ) {
            self.setup_range_insertion_for_chain(chain, Some(index))
        }

        fn setup_range_insertion_for_chain(
            &mut self,
            chain: Vec<SealedBlockWithSenders>,
            invalid_index: Option<usize>,
        ) {
            // setting up execution outcomes for the chain, the blocks will be
            // executed starting from the oldest, so we need to reverse.
            let mut chain_rev = chain;
            chain_rev.reverse();

            let mut execution_outcomes = Vec::with_capacity(chain_rev.len());
            for (index, block) in chain_rev.iter().enumerate() {
                let execution_outcome = self.block_builder.get_execution_outcome(block.clone());
                let state_root = if invalid_index.is_some() && invalid_index.unwrap() == index {
                    B256::random()
                } else {
                    block.state_root
                };
                self.tree.provider.add_state_root(state_root);
                execution_outcomes.push(execution_outcome);
            }
            self.extend_execution_outcome(execution_outcomes);
        }

        fn check_canon_head(&self, head_hash: B256) {
            assert_eq!(self.tree.state.tree_state.canonical_head().hash, head_hash);
        }
    }

    #[test]
    fn test_tree_persist_block_batch() {
        let tree_config = TreeConfig::default();
        let chain_spec = MAINNET.clone();
        let mut test_block_builder =
            TestBlockBuilder::default().with_chain_spec((*chain_spec).clone());

        // we need more than tree_config.persistence_threshold() +1 blocks to
        // trigger the persistence task.
        let blocks: Vec<_> = test_block_builder
            .get_executed_blocks(1..tree_config.persistence_threshold() + 2)
            .collect();
        let mut test_harness = TestHarness::new(chain_spec).with_blocks(blocks);

        let mut blocks = vec![];
        for idx in 0..tree_config.max_execute_block_batch_size() * 2 {
            blocks.push(test_block_builder.generate_random_block(idx as u64, B256::random()));
        }

        test_harness.to_tree_tx.send(FromEngine::DownloadedBlocks(blocks)).unwrap();

        // process the message
        let msg = test_harness.tree.try_recv_engine_message().unwrap().unwrap();
        test_harness.tree.on_engine_message(msg).unwrap();

        // we now should receive the other batch
        let msg = test_harness.tree.try_recv_engine_message().unwrap().unwrap();
        match msg {
            FromEngine::DownloadedBlocks(blocks) => {
                assert_eq!(blocks.len(), tree_config.max_execute_block_batch_size());
            }
            _ => panic!("unexpected message: {:#?}", msg),
        }
    }

    #[tokio::test]
    async fn test_tree_persist_blocks() {
        let tree_config = TreeConfig::default();
        let chain_spec = MAINNET.clone();
        let mut test_block_builder =
            TestBlockBuilder::default().with_chain_spec((*chain_spec).clone());

        // we need more than tree_config.persistence_threshold() +1 blocks to
        // trigger the persistence task.
        let blocks: Vec<_> = test_block_builder
            .get_executed_blocks(1..tree_config.persistence_threshold() + 2)
            .collect();
        let test_harness = TestHarness::new(chain_spec).with_blocks(blocks.clone());
        std::thread::Builder::new()
            .name("Tree Task".to_string())
            .spawn(|| test_harness.tree.run())
            .unwrap();

        // send a message to the tree to enter the main loop.
        test_harness.to_tree_tx.send(FromEngine::DownloadedBlocks(vec![])).unwrap();

        let received_action =
            test_harness.action_rx.recv().expect("Failed to receive save blocks action");
        if let PersistenceAction::SaveBlocks(saved_blocks, _) = received_action {
            // only blocks.len() - tree_config.memory_block_buffer_target() will be
            // persisted
            let expected_persist_len =
                blocks.len() - tree_config.memory_block_buffer_target() as usize;
            assert_eq!(saved_blocks.len(), expected_persist_len);
            assert_eq!(saved_blocks, blocks[..expected_persist_len]);
        } else {
            panic!("unexpected action received {received_action:?}");
        }
    }

    #[tokio::test]
    async fn test_in_memory_state_trait_impl() {
        let blocks: Vec<_> = TestBlockBuilder::default().get_executed_blocks(0..10).collect();
        let test_harness = TestHarness::new(MAINNET.clone()).with_blocks(blocks.clone());

        for executed_block in blocks {
            let sealed_block = executed_block.block();

            let expected_state = BlockState::new(executed_block.clone());

            let actual_state_by_hash = test_harness
                .tree
                .canonical_in_memory_state
                .state_by_hash(sealed_block.hash())
                .unwrap();
            assert_eq!(expected_state, *actual_state_by_hash);

            let actual_state_by_number = test_harness
                .tree
                .canonical_in_memory_state
                .state_by_number(sealed_block.number)
                .unwrap();
            assert_eq!(expected_state, *actual_state_by_number);
        }
    }

    #[tokio::test]
    async fn test_engine_request_during_backfill() {
        let tree_config = TreeConfig::default();
        let blocks: Vec<_> = TestBlockBuilder::default()
            .get_executed_blocks(0..tree_config.persistence_threshold())
            .collect();
        let mut test_harness = TestHarness::new(MAINNET.clone())
            .with_blocks(blocks)
            .with_backfill_state(BackfillSyncState::Active);

        let (tx, rx) = oneshot::channel();
        test_harness
            .tree
            .on_engine_message(FromEngine::Request(
                BeaconEngineMessage::ForkchoiceUpdated {
                    state: ForkchoiceState {
                        head_block_hash: B256::random(),
                        safe_block_hash: B256::random(),
                        finalized_block_hash: B256::random(),
                    },
                    payload_attrs: None,
                    tx,
                }
                .into(),
            ))
            .unwrap();

        let resp = rx.await.unwrap().unwrap().await.unwrap();
        assert!(resp.payload_status.is_syncing());
    }

    #[test]
    fn test_disconnected_payload() {
        let s = include_str!("../../test-data/holesky/2.rlp");
        let data = Bytes::from_str(s).unwrap();
        let block = Block::decode(&mut data.as_ref()).unwrap();
        let sealed = block.seal_slow();
        let hash = sealed.hash();
        let payload = block_to_payload_v1(sealed.clone());

        let mut test_harness = TestHarness::new(HOLESKY.clone());

        let outcome = test_harness.tree.on_new_payload(payload.into(), None).unwrap();
        assert!(outcome.outcome.is_syncing());

        // ensure block is buffered
        let buffered = test_harness.tree.state.buffer.block(&hash).unwrap();
        assert_eq!(buffered.block, sealed);
    }

    #[test]
    fn test_disconnected_block() {
        let s = include_str!("../../test-data/holesky/2.rlp");
        let data = Bytes::from_str(s).unwrap();
        let block = Block::decode(&mut data.as_ref()).unwrap();
        let sealed = block.seal_slow();

        let mut test_harness = TestHarness::new(HOLESKY.clone());

        let outcome = test_harness.tree.insert_block_without_senders(sealed.clone()).unwrap();
        assert_eq!(
            outcome,
            InsertPayloadOk2::Inserted(BlockStatus2::Disconnected {
                head: test_harness.tree.state.tree_state.current_canonical_head,
                missing_ancestor: sealed.parent_num_hash()
            })
        );
    }

    #[tokio::test]
    async fn test_holesky_payload() {
        let s = include_str!("../../test-data/holesky/1.rlp");
        let data = Bytes::from_str(s).unwrap();
        let block = Block::decode(&mut data.as_ref()).unwrap();
        let sealed = block.seal_slow();
        let payload = block_to_payload_v1(sealed);

        let mut test_harness =
            TestHarness::new(HOLESKY.clone()).with_backfill_state(BackfillSyncState::Active);

        let (tx, rx) = oneshot::channel();
        test_harness
            .tree
            .on_engine_message(FromEngine::Request(
                BeaconEngineMessage::NewPayload {
                    payload: payload.clone().into(),
                    cancun_fields: None,
                    tx,
                }
                .into(),
            ))
            .unwrap();

        let resp = rx.await.unwrap().unwrap();
        assert!(resp.is_syncing());
    }

    #[tokio::test]
    async fn test_tree_state_insert_executed() {
        let mut tree_state = TreeState::new(BlockNumHash::default());
        let blocks: Vec<_> = TestBlockBuilder::default().get_executed_blocks(1..4).collect();

        tree_state.insert_executed(blocks[0].clone());
        tree_state.insert_executed(blocks[1].clone());

        assert_eq!(
            tree_state.parent_to_child.get(&blocks[0].block.hash()),
            Some(&HashSet::from_iter([blocks[1].block.hash()]))
        );

        assert!(!tree_state.parent_to_child.contains_key(&blocks[1].block.hash()));

        tree_state.insert_executed(blocks[2].clone());

        assert_eq!(
            tree_state.parent_to_child.get(&blocks[1].block.hash()),
            Some(&HashSet::from_iter([blocks[2].block.hash()]))
        );
        assert!(tree_state.parent_to_child.contains_key(&blocks[1].block.hash()));

        assert!(!tree_state.parent_to_child.contains_key(&blocks[2].block.hash()));
    }

    #[tokio::test]
    async fn test_tree_state_insert_executed_with_reorg() {
        let mut tree_state = TreeState::new(BlockNumHash::default());
        let mut test_block_builder = TestBlockBuilder::default();
        let blocks: Vec<_> = test_block_builder.get_executed_blocks(1..6).collect();

        for block in &blocks {
            tree_state.insert_executed(block.clone());
        }
        assert_eq!(tree_state.blocks_by_hash.len(), 5);

        let fork_block_3 =
            test_block_builder.get_executed_block_with_number(3, blocks[1].block.hash());
        let fork_block_4 =
            test_block_builder.get_executed_block_with_number(4, fork_block_3.block.hash());
        let fork_block_5 =
            test_block_builder.get_executed_block_with_number(5, fork_block_4.block.hash());

        tree_state.insert_executed(fork_block_3.clone());
        tree_state.insert_executed(fork_block_4.clone());
        tree_state.insert_executed(fork_block_5.clone());

        assert_eq!(tree_state.blocks_by_hash.len(), 8);
        assert_eq!(tree_state.blocks_by_number[&3].len(), 2); // two blocks at height 3 (original and fork)
        assert_eq!(tree_state.parent_to_child[&blocks[1].block.hash()].len(), 2); // block 2 should have two children

        // verify that we can insert the same block again without issues
        tree_state.insert_executed(fork_block_4.clone());
        assert_eq!(tree_state.blocks_by_hash.len(), 8);

        assert!(tree_state.parent_to_child[&fork_block_3.block.hash()]
            .contains(&fork_block_4.block.hash()));
        assert!(tree_state.parent_to_child[&fork_block_4.block.hash()]
            .contains(&fork_block_5.block.hash()));

        assert_eq!(tree_state.blocks_by_number[&4].len(), 2);
        assert_eq!(tree_state.blocks_by_number[&5].len(), 2);
    }

    #[tokio::test]
    async fn test_tree_state_remove_before() {
        let start_num_hash = BlockNumHash::default();
        let mut tree_state = TreeState::new(start_num_hash);
        let blocks: Vec<_> = TestBlockBuilder::default().get_executed_blocks(1..6).collect();

        for block in &blocks {
            tree_state.insert_executed(block.clone());
        }

        let last = blocks.last().unwrap();

        // set the canonical head
        tree_state.set_canonical_head(last.block.num_hash());

        // inclusive bound, so we should remove anything up to and including 2
        tree_state.remove_until(
            BlockNumHash::new(2, blocks[1].block.hash()),
            start_num_hash.hash,
            Some(blocks[1].block.num_hash()),
        );

        assert!(!tree_state.blocks_by_hash.contains_key(&blocks[0].block.hash()));
        assert!(!tree_state.blocks_by_hash.contains_key(&blocks[1].block.hash()));
        assert!(!tree_state.blocks_by_number.contains_key(&1));
        assert!(!tree_state.blocks_by_number.contains_key(&2));

        assert!(tree_state.blocks_by_hash.contains_key(&blocks[2].block.hash()));
        assert!(tree_state.blocks_by_hash.contains_key(&blocks[3].block.hash()));
        assert!(tree_state.blocks_by_hash.contains_key(&blocks[4].block.hash()));
        assert!(tree_state.blocks_by_number.contains_key(&3));
        assert!(tree_state.blocks_by_number.contains_key(&4));
        assert!(tree_state.blocks_by_number.contains_key(&5));

        assert!(!tree_state.parent_to_child.contains_key(&blocks[0].block.hash()));
        assert!(!tree_state.parent_to_child.contains_key(&blocks[1].block.hash()));
        assert!(tree_state.parent_to_child.contains_key(&blocks[2].block.hash()));
        assert!(tree_state.parent_to_child.contains_key(&blocks[3].block.hash()));
        assert!(!tree_state.parent_to_child.contains_key(&blocks[4].block.hash()));

        assert_eq!(
            tree_state.parent_to_child.get(&blocks[2].block.hash()),
            Some(&HashSet::from_iter([blocks[3].block.hash()]))
        );
        assert_eq!(
            tree_state.parent_to_child.get(&blocks[3].block.hash()),
            Some(&HashSet::from_iter([blocks[4].block.hash()]))
        );
    }

    #[tokio::test]
    async fn test_tree_state_remove_before_finalized() {
        let start_num_hash = BlockNumHash::default();
        let mut tree_state = TreeState::new(start_num_hash);
        let blocks: Vec<_> = TestBlockBuilder::default().get_executed_blocks(1..6).collect();

        for block in &blocks {
            tree_state.insert_executed(block.clone());
        }

        let last = blocks.last().unwrap();

        // set the canonical head
        tree_state.set_canonical_head(last.block.num_hash());

        // we should still remove everything up to and including 2
        tree_state.remove_until(
            BlockNumHash::new(2, blocks[1].block.hash()),
            start_num_hash.hash,
            None,
        );

        assert!(!tree_state.blocks_by_hash.contains_key(&blocks[0].block.hash()));
        assert!(!tree_state.blocks_by_hash.contains_key(&blocks[1].block.hash()));
        assert!(!tree_state.blocks_by_number.contains_key(&1));
        assert!(!tree_state.blocks_by_number.contains_key(&2));

        assert!(tree_state.blocks_by_hash.contains_key(&blocks[2].block.hash()));
        assert!(tree_state.blocks_by_hash.contains_key(&blocks[3].block.hash()));
        assert!(tree_state.blocks_by_hash.contains_key(&blocks[4].block.hash()));
        assert!(tree_state.blocks_by_number.contains_key(&3));
        assert!(tree_state.blocks_by_number.contains_key(&4));
        assert!(tree_state.blocks_by_number.contains_key(&5));

        assert!(!tree_state.parent_to_child.contains_key(&blocks[0].block.hash()));
        assert!(!tree_state.parent_to_child.contains_key(&blocks[1].block.hash()));
        assert!(tree_state.parent_to_child.contains_key(&blocks[2].block.hash()));
        assert!(tree_state.parent_to_child.contains_key(&blocks[3].block.hash()));
        assert!(!tree_state.parent_to_child.contains_key(&blocks[4].block.hash()));

        assert_eq!(
            tree_state.parent_to_child.get(&blocks[2].block.hash()),
            Some(&HashSet::from_iter([blocks[3].block.hash()]))
        );
        assert_eq!(
            tree_state.parent_to_child.get(&blocks[3].block.hash()),
            Some(&HashSet::from_iter([blocks[4].block.hash()]))
        );
    }

    #[tokio::test]
    async fn test_tree_state_remove_before_lower_finalized() {
        let start_num_hash = BlockNumHash::default();
        let mut tree_state = TreeState::new(start_num_hash);
        let blocks: Vec<_> = TestBlockBuilder::default().get_executed_blocks(1..6).collect();

        for block in &blocks {
            tree_state.insert_executed(block.clone());
        }

        let last = blocks.last().unwrap();

        // set the canonical head
        tree_state.set_canonical_head(last.block.num_hash());

        // we have no forks so we should still remove anything up to and including 2
        tree_state.remove_until(
            BlockNumHash::new(2, blocks[1].block.hash()),
            start_num_hash.hash,
            Some(blocks[0].block.num_hash()),
        );

        assert!(!tree_state.blocks_by_hash.contains_key(&blocks[0].block.hash()));
        assert!(!tree_state.blocks_by_hash.contains_key(&blocks[1].block.hash()));
        assert!(!tree_state.blocks_by_number.contains_key(&1));
        assert!(!tree_state.blocks_by_number.contains_key(&2));

        assert!(tree_state.blocks_by_hash.contains_key(&blocks[2].block.hash()));
        assert!(tree_state.blocks_by_hash.contains_key(&blocks[3].block.hash()));
        assert!(tree_state.blocks_by_hash.contains_key(&blocks[4].block.hash()));
        assert!(tree_state.blocks_by_number.contains_key(&3));
        assert!(tree_state.blocks_by_number.contains_key(&4));
        assert!(tree_state.blocks_by_number.contains_key(&5));

        assert!(!tree_state.parent_to_child.contains_key(&blocks[0].block.hash()));
        assert!(!tree_state.parent_to_child.contains_key(&blocks[1].block.hash()));
        assert!(tree_state.parent_to_child.contains_key(&blocks[2].block.hash()));
        assert!(tree_state.parent_to_child.contains_key(&blocks[3].block.hash()));
        assert!(!tree_state.parent_to_child.contains_key(&blocks[4].block.hash()));

        assert_eq!(
            tree_state.parent_to_child.get(&blocks[2].block.hash()),
            Some(&HashSet::from_iter([blocks[3].block.hash()]))
        );
        assert_eq!(
            tree_state.parent_to_child.get(&blocks[3].block.hash()),
            Some(&HashSet::from_iter([blocks[4].block.hash()]))
        );
    }

    #[tokio::test]
    async fn test_tree_state_on_new_head() {
        let chain_spec = MAINNET.clone();
        let mut test_harness = TestHarness::new(chain_spec);
        let mut test_block_builder = TestBlockBuilder::default();

        let blocks: Vec<_> = test_block_builder.get_executed_blocks(1..6).collect();

        for block in &blocks {
            test_harness.tree.state.tree_state.insert_executed(block.clone());
        }

        // set block 3 as the current canonical head
        test_harness.tree.state.tree_state.set_canonical_head(blocks[2].block.num_hash());

        // create a fork from block 2
        let fork_block_3 =
            test_block_builder.get_executed_block_with_number(3, blocks[1].block.hash());
        let fork_block_4 =
            test_block_builder.get_executed_block_with_number(4, fork_block_3.block.hash());
        let fork_block_5 =
            test_block_builder.get_executed_block_with_number(5, fork_block_4.block.hash());

        test_harness.tree.state.tree_state.insert_executed(fork_block_3.clone());
        test_harness.tree.state.tree_state.insert_executed(fork_block_4.clone());
        test_harness.tree.state.tree_state.insert_executed(fork_block_5.clone());

        // normal (non-reorg) case
        let result = test_harness.tree.on_new_head(blocks[4].block.hash()).unwrap();
        assert!(matches!(result, Some(NewCanonicalChain::Commit { .. })));
        if let Some(NewCanonicalChain::Commit { new }) = result {
            assert_eq!(new.len(), 2);
            assert_eq!(new[0].block.hash(), blocks[3].block.hash());
            assert_eq!(new[1].block.hash(), blocks[4].block.hash());
        }

        // reorg case
        let result = test_harness.tree.on_new_head(fork_block_5.block.hash()).unwrap();
        assert!(matches!(result, Some(NewCanonicalChain::Reorg { .. })));
        if let Some(NewCanonicalChain::Reorg { new, old }) = result {
            assert_eq!(new.len(), 3);
            assert_eq!(new[0].block.hash(), fork_block_3.block.hash());
            assert_eq!(new[1].block.hash(), fork_block_4.block.hash());
            assert_eq!(new[2].block.hash(), fork_block_5.block.hash());

            assert_eq!(old.len(), 1);
            assert_eq!(old[0].block.hash(), blocks[2].block.hash());
        }
    }

    #[tokio::test]
    async fn test_tree_state_on_new_head_deep_fork() {
        reth_tracing::init_test_tracing();

        let chain_spec = MAINNET.clone();
        let mut test_harness = TestHarness::new(chain_spec);
        let mut test_block_builder = TestBlockBuilder::default();

        let blocks: Vec<_> = test_block_builder.get_executed_blocks(0..5).collect();

        for block in &blocks {
            test_harness.tree.state.tree_state.insert_executed(block.clone());
        }

        // set last block as the current canonical head
        let last_block = blocks.last().unwrap().block.clone();

        test_harness.tree.state.tree_state.set_canonical_head(last_block.num_hash());

        // create a fork chain from last_block
        let chain_a = test_block_builder.create_fork(&last_block, 10);
        let chain_b = test_block_builder.create_fork(&last_block, 10);

        for block in &chain_a {
            test_harness.tree.state.tree_state.insert_executed(ExecutedBlock {
                block: Arc::new(block.block.clone()),
                senders: Arc::new(block.senders.clone()),
                execution_output: Arc::new(ExecutionOutcome::default()),
                hashed_state: Arc::new(HashedPostState::default()),
                trie: Arc::new(TrieUpdates::default()),
            });
        }
        test_harness.tree.state.tree_state.set_canonical_head(chain_a.last().unwrap().num_hash());

        for block in &chain_b {
            test_harness.tree.state.tree_state.insert_executed(ExecutedBlock {
                block: Arc::new(block.block.clone()),
                senders: Arc::new(block.senders.clone()),
                execution_output: Arc::new(ExecutionOutcome::default()),
                hashed_state: Arc::new(HashedPostState::default()),
                trie: Arc::new(TrieUpdates::default()),
            });
        }

        // for each block in chain_b, reorg to it and then back to canonical
        let mut expected_new = Vec::new();
        for block in &chain_b {
            // reorg to chain from block b
            let result = test_harness.tree.on_new_head(block.block.hash()).unwrap();
            assert_matches!(result, Some(NewCanonicalChain::Reorg { .. }));

            expected_new.push(block);
            if let Some(NewCanonicalChain::Reorg { new, old }) = result {
                assert_eq!(new.len(), expected_new.len());
                for (index, block) in expected_new.iter().enumerate() {
                    assert_eq!(new[index].block.hash(), block.block.hash());
                }

                assert_eq!(old.len(), chain_a.len());
                for (index, block) in chain_a.iter().enumerate() {
                    assert_eq!(old[index].block.hash(), block.block.hash());
                }
            }

            // set last block of chain a as canonical head
            test_harness.tree.on_new_head(chain_a.last().unwrap().hash()).unwrap();
        }
    }

    #[tokio::test]
    async fn test_get_canonical_blocks_to_persist() {
        let chain_spec = MAINNET.clone();
        let mut test_harness = TestHarness::new(chain_spec);
        let mut test_block_builder = TestBlockBuilder::default();

        let canonical_head_number = 9;
        let blocks: Vec<_> =
            test_block_builder.get_executed_blocks(0..canonical_head_number + 1).collect();
        test_harness = test_harness.with_blocks(blocks.clone());

        let last_persisted_block_number = 3;
        test_harness.tree.persistence_state.last_persisted_block.number =
            last_persisted_block_number;

        let persistence_threshold = 4;
        let memory_block_buffer_target = 3;
        test_harness.tree.config = TreeConfig::default()
            .with_persistence_threshold(persistence_threshold)
            .with_memory_block_buffer_target(memory_block_buffer_target);

        let blocks_to_persist = test_harness.tree.get_canonical_blocks_to_persist();

        let expected_blocks_to_persist_length: usize =
            (canonical_head_number - memory_block_buffer_target - last_persisted_block_number)
                .try_into()
                .unwrap();

        assert_eq!(blocks_to_persist.len(), expected_blocks_to_persist_length);
        for (i, item) in
            blocks_to_persist.iter().enumerate().take(expected_blocks_to_persist_length)
        {
            assert_eq!(item.block.number, last_persisted_block_number + i as u64 + 1);
        }

        // make sure only canonical blocks are included
        let fork_block = test_block_builder.get_executed_block_with_number(4, B256::random());
        let fork_block_hash = fork_block.block.hash();
        test_harness.tree.state.tree_state.insert_executed(fork_block);

        assert!(test_harness.tree.state.tree_state.block_by_hash(fork_block_hash).is_some());

        let blocks_to_persist = test_harness.tree.get_canonical_blocks_to_persist();
        assert_eq!(blocks_to_persist.len(), expected_blocks_to_persist_length);

        // check that the fork block is not included in the blocks to persist
        assert!(!blocks_to_persist.iter().any(|b| b.block.hash() == fork_block_hash));

        // check that the original block 4 is still included
        assert!(blocks_to_persist
            .iter()
            .any(|b| b.block.number == 4 && b.block.hash() == blocks[4].block.hash()));
    }

    #[tokio::test]
    async fn test_engine_tree_fcu_missing_head() {
        let chain_spec = MAINNET.clone();
        let mut test_harness = TestHarness::new(chain_spec.clone());

        let mut test_block_builder =
            TestBlockBuilder::default().with_chain_spec((*chain_spec).clone());

        let blocks: Vec<_> = test_block_builder.get_executed_blocks(0..5).collect();
        test_harness = test_harness.with_blocks(blocks);

        let missing_block = test_block_builder
            .generate_random_block(6, test_harness.blocks.last().unwrap().block().hash());

        test_harness.fcu_to(missing_block.hash(), PayloadStatusEnum::Syncing).await;

        // after FCU we receive an EngineApiEvent::Download event to get the missing block.
        let event = test_harness.from_tree_rx.recv().await.unwrap();
        match event {
            EngineApiEvent::Download(DownloadRequest::BlockSet(actual_block_set)) => {
                let expected_block_set = HashSet::from_iter([missing_block.hash()]);
                assert_eq!(actual_block_set, expected_block_set);
            }
            _ => panic!("Unexpected event: {:#?}", event),
        }
    }

    #[tokio::test]
    async fn test_engine_tree_fcu_canon_chain_insertion() {
        let chain_spec = MAINNET.clone();
        let mut test_harness = TestHarness::new(chain_spec.clone());

        let base_chain: Vec<_> = test_harness.block_builder.get_executed_blocks(0..1).collect();
        test_harness = test_harness.with_blocks(base_chain.clone());

        test_harness
            .fcu_to(base_chain.last().unwrap().block().hash(), ForkchoiceStatus::Valid)
            .await;

        // extend main chain
        let main_chain = test_harness.block_builder.create_fork(base_chain[0].block(), 3);

        test_harness.insert_chain(main_chain).await;
    }

    #[tokio::test]
    async fn test_engine_tree_fcu_reorg_with_all_blocks() {
        let chain_spec = MAINNET.clone();
        let mut test_harness = TestHarness::new(chain_spec.clone());

        let main_chain: Vec<_> = test_harness.block_builder.get_executed_blocks(0..5).collect();
        test_harness = test_harness.with_blocks(main_chain.clone());

        let fork_chain = test_harness.block_builder.create_fork(main_chain[2].block(), 3);
        let fork_chain_last_hash = fork_chain.last().unwrap().hash();

        // add fork blocks to the tree
        for block in &fork_chain {
            test_harness.insert_block(block.clone()).unwrap();
        }

        test_harness.send_fcu(fork_chain_last_hash, ForkchoiceStatus::Valid).await;

        // check for ForkBlockAdded events, we expect fork_chain.len() blocks added
        test_harness.check_fork_chain_insertion(fork_chain.clone()).await;

        // check for CanonicalChainCommitted event
        test_harness.check_canon_commit(fork_chain_last_hash).await;

        test_harness.check_fcu(fork_chain_last_hash, ForkchoiceStatus::Valid).await;

        // new head is the tip of the fork chain
        test_harness.check_canon_head(fork_chain_last_hash);
    }

    #[tokio::test]
    async fn test_engine_tree_live_sync_transition_required_blocks_requested() {
        reth_tracing::init_test_tracing();

        let chain_spec = MAINNET.clone();
        let mut test_harness = TestHarness::new(chain_spec.clone());

        let base_chain: Vec<_> = test_harness.block_builder.get_executed_blocks(0..1).collect();
        test_harness = test_harness.with_blocks(base_chain.clone());

        test_harness
            .fcu_to(base_chain.last().unwrap().block().hash(), ForkchoiceStatus::Valid)
            .await;

        // extend main chain with enough blocks to trigger pipeline run but don't insert them
        let main_chain = test_harness
            .block_builder
            .create_fork(base_chain[0].block(), MIN_BLOCKS_FOR_PIPELINE_RUN + 10);

        let main_chain_last_hash = main_chain.last().unwrap().hash();
        test_harness.send_fcu(main_chain_last_hash, ForkchoiceStatus::Syncing).await;

        test_harness.check_fcu(main_chain_last_hash, ForkchoiceStatus::Syncing).await;

        // create event for backfill finished
        let backfill_finished_block_number = MIN_BLOCKS_FOR_PIPELINE_RUN + 1;
        let backfill_finished = FromOrchestrator::BackfillSyncFinished(ControlFlow::Continue {
            block_number: backfill_finished_block_number,
        });

        let backfill_tip_block = main_chain[(backfill_finished_block_number - 1) as usize].clone();
        // add block to mock provider to enable persistence clean up.
        test_harness
            .provider
            .add_block(backfill_tip_block.hash(), backfill_tip_block.block.unseal());
        test_harness.tree.on_engine_message(FromEngine::Event(backfill_finished)).unwrap();

        let event = test_harness.from_tree_rx.recv().await.unwrap();
        match event {
            EngineApiEvent::Download(DownloadRequest::BlockSet(hash_set)) => {
                assert_eq!(hash_set, HashSet::from_iter([main_chain_last_hash]));
            }
            _ => panic!("Unexpected event: {:#?}", event),
        }

        test_harness
            .tree
            .on_engine_message(FromEngine::DownloadedBlocks(vec![main_chain
                .last()
                .unwrap()
                .clone()]))
            .unwrap();

        let event = test_harness.from_tree_rx.recv().await.unwrap();
        match event {
            EngineApiEvent::Download(DownloadRequest::BlockRange(initial_hash, total_blocks)) => {
                assert_eq!(
                    total_blocks,
                    (main_chain.len() - backfill_finished_block_number as usize - 1) as u64
                );
                assert_eq!(initial_hash, main_chain.last().unwrap().parent_hash);
            }
            _ => panic!("Unexpected event: {:#?}", event),
        }
    }

    #[tokio::test]
    async fn test_engine_tree_live_sync_transition_eventually_canonical() {
        reth_tracing::init_test_tracing();

        let chain_spec = MAINNET.clone();
        let mut test_harness = TestHarness::new(chain_spec.clone());
        test_harness.tree.config = test_harness.tree.config.with_max_execute_block_batch_size(100);

        // create base chain and setup test harness with it
        let base_chain: Vec<_> = test_harness.block_builder.get_executed_blocks(0..1).collect();
        test_harness = test_harness.with_blocks(base_chain.clone());

        // fcu to the tip of base chain
        test_harness
            .fcu_to(base_chain.last().unwrap().block().hash(), ForkchoiceStatus::Valid)
            .await;

        // create main chain, extension of base chain, with enough blocks to
        // trigger backfill sync
        let main_chain = test_harness
            .block_builder
            .create_fork(base_chain[0].block(), MIN_BLOCKS_FOR_PIPELINE_RUN + 10);

        let main_chain_last = main_chain.last().unwrap();
        let main_chain_last_hash = main_chain_last.hash();
        let main_chain_backfill_target =
            main_chain.get(MIN_BLOCKS_FOR_PIPELINE_RUN as usize).unwrap();
        let main_chain_backfill_target_hash = main_chain_backfill_target.hash();

        // fcu to the element of main chain that should trigger backfill sync
        test_harness.send_fcu(main_chain_backfill_target_hash, ForkchoiceStatus::Syncing).await;
        test_harness.check_fcu(main_chain_backfill_target_hash, ForkchoiceStatus::Syncing).await;

        // check download request for target
        let event = test_harness.from_tree_rx.recv().await.unwrap();
        match event {
            EngineApiEvent::Download(DownloadRequest::BlockSet(hash_set)) => {
                assert_eq!(hash_set, HashSet::from_iter([main_chain_backfill_target_hash]));
            }
            _ => panic!("Unexpected event: {:#?}", event),
        }

        // send message to tell the engine the requested block was downloaded
        test_harness
            .tree
            .on_engine_message(FromEngine::DownloadedBlocks(vec![
                main_chain_backfill_target.clone()
            ]))
            .unwrap();

        // check that backfill is triggered
        let event = test_harness.from_tree_rx.recv().await.unwrap();
        match event {
            EngineApiEvent::BackfillAction(BackfillAction::Start(
                reth_stages::PipelineTarget::Sync(target_hash),
            )) => {
                assert_eq!(target_hash, main_chain_backfill_target_hash);
            }
            _ => panic!("Unexpected event: {:#?}", event),
        }

        // persist blocks of main chain, same as the backfill operation would do
        let backfilled_chain: Vec<_> =
            main_chain.clone().drain(0..(MIN_BLOCKS_FOR_PIPELINE_RUN + 1) as usize).collect();
        test_harness.persist_blocks(backfilled_chain.clone());

        test_harness.setup_range_insertion_for_valid_chain(backfilled_chain);

        // send message to mark backfill finished
        test_harness
            .tree
            .on_engine_message(FromEngine::Event(FromOrchestrator::BackfillSyncFinished(
                ControlFlow::Continue { block_number: main_chain_backfill_target.number },
            )))
            .unwrap();

        // send fcu to the tip of main
        test_harness.fcu_to(main_chain_last_hash, ForkchoiceStatus::Syncing).await;

        let event = test_harness.from_tree_rx.recv().await.unwrap();
        match event {
            EngineApiEvent::Download(DownloadRequest::BlockSet(target_hash)) => {
                assert_eq!(target_hash, HashSet::from_iter([main_chain_last_hash]));
            }
            _ => panic!("Unexpected event: {:#?}", event),
        }

        // tell engine main chain tip downloaded
        test_harness
            .tree
            .on_engine_message(FromEngine::DownloadedBlocks(vec![main_chain_last.clone()]))
            .unwrap();

        // check download range request
        let event = test_harness.from_tree_rx.recv().await.unwrap();
        match event {
            EngineApiEvent::Download(DownloadRequest::BlockRange(initial_hash, total_blocks)) => {
                assert_eq!(
                    total_blocks,
                    (main_chain.len() - MIN_BLOCKS_FOR_PIPELINE_RUN as usize - 2) as u64
                );
                assert_eq!(initial_hash, main_chain_last.parent_hash);
            }
            _ => panic!("Unexpected event: {:#?}", event),
        }

        let remaining: Vec<_> = main_chain
            .clone()
            .drain((MIN_BLOCKS_FOR_PIPELINE_RUN + 1) as usize..main_chain.len())
            .collect();

        test_harness.setup_range_insertion_for_valid_chain(remaining.clone());

        // tell engine block range downloaded
        test_harness
            .tree
            .on_engine_message(FromEngine::DownloadedBlocks(remaining.clone()))
            .unwrap();

        test_harness.check_canon_chain_insertion(remaining).await;

        // check canonical chain committed event with the hash of the latest block
        test_harness.check_canon_commit(main_chain_last_hash).await;

        // new head is the tip of the main chain
        test_harness.check_canon_head(main_chain_last_hash);
    }

    #[tokio::test]
    async fn test_engine_tree_live_sync_fcu_extends_canon_chain() {
        reth_tracing::init_test_tracing();

        let chain_spec = MAINNET.clone();
        let mut test_harness = TestHarness::new(chain_spec.clone());

        // create base chain and setup test harness with it
        let base_chain: Vec<_> = test_harness.block_builder.get_executed_blocks(0..1).collect();
        test_harness = test_harness.with_blocks(base_chain.clone());

        // fcu to the tip of base chain
        test_harness
            .fcu_to(base_chain.last().unwrap().block().hash(), ForkchoiceStatus::Valid)
            .await;

        // create main chain, extension of base chain
        let main_chain = test_harness.block_builder.create_fork(base_chain[0].block(), 10);
        // determine target in the middle of main hain
        let target = main_chain.get(5).unwrap();
        let target_hash = target.hash();
        let main_last = main_chain.last().unwrap();
        let main_last_hash = main_last.hash();

        // insert main chain
        test_harness.insert_chain(main_chain).await;

        // send fcu to target
        test_harness.send_fcu(target_hash, ForkchoiceStatus::Valid).await;

        test_harness.check_canon_commit(target_hash).await;
        test_harness.check_fcu(target_hash, ForkchoiceStatus::Valid).await;

        // send fcu to main tip
        test_harness.send_fcu(main_last_hash, ForkchoiceStatus::Valid).await;

        test_harness.check_canon_commit(main_last_hash).await;
        test_harness.check_fcu(main_last_hash, ForkchoiceStatus::Valid).await;
        test_harness.check_canon_head(main_last_hash);
    }

    #[tokio::test]
    async fn test_engine_tree_valid_forks_with_older_canonical_head() {
        reth_tracing::init_test_tracing();

        let chain_spec = MAINNET.clone();
        let mut test_harness = TestHarness::new(chain_spec.clone());

        // create base chain and setup test harness with it
        let base_chain: Vec<_> = test_harness.block_builder.get_executed_blocks(0..1).collect();
        test_harness = test_harness.with_blocks(base_chain.clone());

        let old_head = base_chain.first().unwrap().block();

        // extend base chain
        let extension_chain = test_harness.block_builder.create_fork(old_head, 5);
        let fork_block = extension_chain.last().unwrap().block.clone();

        test_harness.setup_range_insertion_for_valid_chain(extension_chain.clone());
        test_harness.insert_chain(extension_chain).await;

        // fcu to old_head
        test_harness.fcu_to(old_head.hash(), ForkchoiceStatus::Valid).await;

        // create two competing chains starting from fork_block
        let chain_a = test_harness.block_builder.create_fork(&fork_block, 10);
        let chain_b = test_harness.block_builder.create_fork(&fork_block, 10);

        // insert chain A blocks using newPayload
        test_harness.setup_range_insertion_for_valid_chain(chain_a.clone());
        for block in &chain_a {
            test_harness.send_new_payload(block.clone()).await;
        }

        test_harness.check_canon_chain_insertion(chain_a.clone()).await;

        // insert chain B blocks using newPayload
        test_harness.setup_range_insertion_for_valid_chain(chain_b.clone());
        for block in &chain_b {
            test_harness.send_new_payload(block.clone()).await;
        }

        test_harness.check_canon_chain_insertion(chain_b.clone()).await;

        // send FCU to make the tip of chain B the new head
        let chain_b_tip_hash = chain_b.last().unwrap().hash();
        test_harness.send_fcu(chain_b_tip_hash, ForkchoiceStatus::Valid).await;

        // check for CanonicalChainCommitted event
        test_harness.check_canon_commit(chain_b_tip_hash).await;

        // verify FCU was processed
        test_harness.check_fcu(chain_b_tip_hash, ForkchoiceStatus::Valid).await;

        // verify the new canonical head
        test_harness.check_canon_head(chain_b_tip_hash);

        // verify that chain A is now considered a fork
        assert!(test_harness.tree.is_fork(chain_a.last().unwrap().hash()).unwrap());
    }

    #[tokio::test]
    async fn test_engine_tree_buffered_blocks_are_eventually_connected() {
        let chain_spec = MAINNET.clone();
        let mut test_harness = TestHarness::new(chain_spec.clone());

        let base_chain: Vec<_> = test_harness.block_builder.get_executed_blocks(0..1).collect();
        test_harness = test_harness.with_blocks(base_chain.clone());

        // side chain consisting of two blocks, the last will be inserted first
        // so that we force it to be buffered
        let side_chain =
            test_harness.block_builder.create_fork(base_chain.last().unwrap().block(), 2);

        // buffer last block of side chain
        let buffered_block = side_chain.last().unwrap();
        let buffered_block_hash = buffered_block.hash();

        test_harness.setup_range_insertion_for_valid_chain(vec![buffered_block.clone()]);
        test_harness.send_new_payload(buffered_block.clone()).await;

        assert!(test_harness.tree.state.buffer.block(&buffered_block_hash).is_some());

        let non_buffered_block = side_chain.first().unwrap();
        let non_buffered_block_hash = non_buffered_block.hash();

        // insert block that continues the canon chain, should not be buffered
        test_harness.setup_range_insertion_for_valid_chain(vec![non_buffered_block.clone()]);
        test_harness.send_new_payload(non_buffered_block.clone()).await;
        assert!(test_harness.tree.state.buffer.block(&non_buffered_block_hash).is_none());

        // the previously buffered block should be connected now
        assert!(test_harness.tree.state.buffer.block(&buffered_block_hash).is_none());

        // both blocks are added to the canon chain in order
        test_harness.check_canon_block_added(non_buffered_block_hash).await;
        test_harness.check_canon_block_added(buffered_block_hash).await;
    }

    #[tokio::test]
    async fn test_engine_tree_valid_and_invalid_forks_with_older_canonical_head() {
        reth_tracing::init_test_tracing();

        let chain_spec = MAINNET.clone();
        let mut test_harness = TestHarness::new(chain_spec.clone());

        // create base chain and setup test harness with it
        let base_chain: Vec<_> = test_harness.block_builder.get_executed_blocks(0..1).collect();
        test_harness = test_harness.with_blocks(base_chain.clone());

        let old_head = base_chain.first().unwrap().block();

        // extend base chain
        let extension_chain = test_harness.block_builder.create_fork(old_head, 5);
        let fork_block = extension_chain.last().unwrap().block.clone();
        test_harness.insert_chain(extension_chain).await;

        // fcu to old_head
        test_harness.fcu_to(old_head.hash(), ForkchoiceStatus::Valid).await;

        // create two competing chains starting from fork_block, one of them invalid
        let total_fork_elements = 10;
        let chain_a = test_harness.block_builder.create_fork(&fork_block, total_fork_elements);
        let chain_b = test_harness.block_builder.create_fork(&fork_block, total_fork_elements);

        // insert chain B blocks using newPayload
        test_harness.setup_range_insertion_for_valid_chain(chain_b.clone());
        for block in &chain_b {
            test_harness.send_new_payload(block.clone()).await;
            test_harness.send_fcu(block.hash(), ForkchoiceStatus::Valid).await;
            test_harness.check_canon_block_added(block.hash()).await;
            test_harness.check_canon_commit(block.hash()).await;
            test_harness.check_fcu(block.hash(), ForkchoiceStatus::Valid).await;
        }

        // insert chain A blocks using newPayload, one of the blocks will be invalid
        let invalid_index = 3;
        test_harness.setup_range_insertion_for_invalid_chain(chain_a.clone(), invalid_index);
        for block in &chain_a {
            test_harness.send_new_payload(block.clone()).await;
        }

        // check canon chain insertion up to the invalid index and taking into
        // account reversed ordering
        test_harness
            .check_fork_chain_insertion(
                chain_a[..chain_a.len() - invalid_index - 1].iter().cloned(),
            )
            .await;

        // send FCU to make the tip of chain A, expect invalid
        let chain_a_tip_hash = chain_a.last().unwrap().hash();
        test_harness.fcu_to(chain_a_tip_hash, ForkchoiceStatus::Invalid).await;

        // send FCU to make the tip of chain B the new head
        let chain_b_tip_hash = chain_b.last().unwrap().hash();

        // verify the new canonical head
        test_harness.check_canon_head(chain_b_tip_hash);

        // verify the canonical head didn't change
        test_harness.check_canon_head(chain_b_tip_hash);
    }

    #[tokio::test]
    async fn test_engine_tree_reorg_with_missing_ancestor_expecting_valid() {
        reth_tracing::init_test_tracing();
        let chain_spec = MAINNET.clone();
        let mut test_harness = TestHarness::new(chain_spec.clone());

        let base_chain: Vec<_> = test_harness.block_builder.get_executed_blocks(0..6).collect();
        test_harness = test_harness.with_blocks(base_chain.clone());

        // create a side chain with an invalid block
        let side_chain =
            test_harness.block_builder.create_fork(base_chain.last().unwrap().block(), 15);
        let invalid_index = 9;

        test_harness.setup_range_insertion_for_invalid_chain(side_chain.clone(), invalid_index);

        for (index, block) in side_chain.iter().enumerate() {
            test_harness.send_new_payload(block.clone()).await;

            if index < side_chain.len() - invalid_index - 1 {
                test_harness.send_fcu(block.block.hash(), ForkchoiceStatus::Valid).await;
            }
        }

        // Try to do a forkchoice update to a block after the invalid one
        let fork_tip_hash = side_chain.last().unwrap().hash();
        test_harness.send_fcu(fork_tip_hash, ForkchoiceStatus::Invalid).await;
    }
}
