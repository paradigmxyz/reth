use crate::{
    backfill::BackfillAction,
    chain::FromOrchestrator,
    engine::{DownloadRequest, EngineApiEvent, FromEngine},
    persistence::PersistenceHandle,
};
use reth_beacon_consensus::{
    BeaconEngineMessage, ForkchoiceStateTracker, InvalidHeaderCache, OnForkChoiceUpdated,
};
use reth_blockchain_tree::{
    error::InsertBlockErrorKind, BlockAttachment, BlockBuffer, BlockStatus,
};
use reth_blockchain_tree_api::{error::InsertBlockError, InsertPayloadOk};
use reth_consensus::{Consensus, PostExecutionInput};
use reth_engine_primitives::EngineTypes;
use reth_errors::{ConsensusError, ProviderResult};
use reth_evm::execute::{BlockExecutorProvider, Executor};
use reth_payload_primitives::PayloadTypes;
use reth_payload_validator::ExecutionPayloadValidator;
use reth_primitives::{
    Address, Block, BlockNumber, GotExpected, Receipts, Requests, SealedBlock,
    SealedBlockWithSenders, B256, U256,
};
use reth_provider::{
    BlockReader, ExecutionOutcome, StateProvider, StateProviderFactory, StateRootProvider,
};
use reth_revm::database::StateProviderDatabase;
use reth_rpc_types::{
    engine::{
        CancunPayloadFields, ForkchoiceState, PayloadStatus, PayloadStatusEnum,
        PayloadValidationError,
    },
    ExecutionPayload,
};
use reth_trie::{updates::TrieUpdates, HashedPostState};
use std::{
    collections::{BTreeMap, HashMap},
    marker::PhantomData,
    sync::{mpsc::Receiver, Arc},
};
use tokio::sync::{mpsc::UnboundedSender, oneshot};
use tracing::*;

mod memory_overlay;
pub use memory_overlay::MemoryOverlayStateProvider;

/// Maximum number of blocks to be kept only in memory without triggering persistence.
const PERSISTENCE_THRESHOLD: u64 = 256;

/// Represents an executed block stored in-memory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecutedBlock {
    block: Arc<SealedBlock>,
    senders: Arc<Vec<Address>>,
    execution_output: Arc<ExecutionOutcome>,
    hashed_state: Arc<HashedPostState>,
    trie: Arc<TrieUpdates>,
}

impl ExecutedBlock {
    pub(crate) const fn new(
        block: Arc<SealedBlock>,
        senders: Arc<Vec<Address>>,
        execution_output: Arc<ExecutionOutcome>,
        hashed_state: Arc<HashedPostState>,
        trie: Arc<TrieUpdates>,
    ) -> Self {
        Self { block, senders, execution_output, hashed_state, trie }
    }

    /// Returns a reference to the executed block.
    pub(crate) fn block(&self) -> &SealedBlock {
        &self.block
    }

    /// Returns a reference to the block's senders
    pub(crate) fn senders(&self) -> &Vec<Address> {
        &self.senders
    }

    /// Returns a reference to the block's execution outcome
    pub(crate) fn execution_outcome(&self) -> &ExecutionOutcome {
        &self.execution_output
    }

    /// Returns a reference to the hashed state result of the execution outcome
    pub(crate) fn hashed_state(&self) -> &HashedPostState {
        &self.hashed_state
    }

    /// Returns a reference to the trie updates for the block
    pub(crate) fn trie_updates(&self) -> &TrieUpdates {
        &self.trie
    }
}

/// Keeps track of the state of the tree.
#[derive(Debug, Default)]
pub struct TreeState {
    /// All executed blocks by hash.
    blocks_by_hash: HashMap<B256, ExecutedBlock>,
    /// Executed blocks grouped by their respective block number.
    blocks_by_number: BTreeMap<BlockNumber, Vec<ExecutedBlock>>,
    /// Pending state not yet applied
    pending: Option<Arc<State>>,
    /// Block number and hash of the current head.
    current_head: Option<(BlockNumber, B256)>,
}

impl TreeState {
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
}

impl InMemoryState for TreeState {
    fn in_memory_state_by_hash(&self, hash: B256) -> Option<Arc<State>> {
        let sealed_block = self.block_by_hash(hash)?;
        Some(Arc::new(State::new(sealed_block)))
    }

    fn in_memory_state_by_number(&self, number: u64) -> Option<Arc<State>> {
        let sealed_block = self.block_by_number(number)?;
        Some(Arc::new(State::new(sealed_block)))
    }

    fn in_memory_current_head(&self) -> Option<(BlockNumber, B256)> {
        self.current_head
    }

    fn in_memory_pending_state(&self) -> Option<Arc<State>> {
        self.pending.clone()
    }

    fn in_memory_pending_block_hash(&self) -> Option<B256> {
        self.pending.as_ref().map(|state| state.block_hash)
    }
}

/// Tracks the state of the engine api internals.
///
/// This type is shareable.
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
    fn new(block_buffer_limit: u32, max_invalid_header_cache_length: u32) -> Self {
        Self {
            invalid_headers: InvalidHeaderCache::new(max_invalid_header_cache_length),
            buffer: BlockBuffer::new(block_buffer_limit),
            tree_state: TreeState::default(),
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

/// The actions that can be performed on the tree.
#[derive(Debug)]
pub enum TreeAction {
    /// Make target canonical.
    MakeCanonical(B256),
}

#[derive(Debug)]
pub struct EngineApiTreeHandlerImpl<P, E, T: EngineTypes> {
    provider: P,
    executor_provider: E,
    consensus: Arc<dyn Consensus>,
    payload_validator: ExecutionPayloadValidator,
    state: EngineApiTreeState,
    incoming: Receiver<FromEngine<BeaconEngineMessage<T>>>,
    outgoing: UnboundedSender<EngineApiEvent>,
    persistence: PersistenceHandle,
    persistence_state: PersistenceState,
    /// (tmp) The flag indicating whether the pipeline is active.
    is_pipeline_active: bool,
    _marker: PhantomData<T>,
}

impl<P, E, T> EngineApiTreeHandlerImpl<P, E, T>
where
    P: BlockReader + StateProviderFactory + Clone + 'static,
    E: BlockExecutorProvider,
    T: EngineTypes + 'static,
{
    #[allow(clippy::too_many_arguments)]
    fn new(
        provider: P,
        executor_provider: E,
        consensus: Arc<dyn Consensus>,
        payload_validator: ExecutionPayloadValidator,
        incoming: Receiver<FromEngine<BeaconEngineMessage<T>>>,
        outgoing: UnboundedSender<EngineApiEvent>,
        state: EngineApiTreeState,
        persistence: PersistenceHandle,
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
            is_pipeline_active: false,
            state,
            _marker: PhantomData,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn spawn_new(
        provider: P,
        executor_provider: E,
        consensus: Arc<dyn Consensus>,
        payload_validator: ExecutionPayloadValidator,
        incoming: Receiver<FromEngine<BeaconEngineMessage<T>>>,
        state: EngineApiTreeState,
        persistence: PersistenceHandle,
    ) -> UnboundedSender<EngineApiEvent> {
        let (outgoing, rx) = tokio::sync::mpsc::unbounded_channel();
        let task = Self::new(
            provider,
            executor_provider,
            consensus,
            payload_validator,
            incoming,
            outgoing.clone(),
            state,
            persistence,
        );
        std::thread::Builder::new().name("Tree Task".to_string()).spawn(|| task.run()).unwrap();
        outgoing
    }

    fn run(mut self) {
        while let Ok(msg) = self.incoming.recv() {
            match msg {
                FromEngine::Event(event) => match event {
                    FromOrchestrator::BackfillSyncFinished => {
                        todo!()
                    }
                    FromOrchestrator::BackfillSyncStarted => {
                        todo!()
                    }
                },
                FromEngine::Request(request) => match request {
                    BeaconEngineMessage::ForkchoiceUpdated { state, payload_attrs, tx } => {
                        let output = self.on_forkchoice_updated(state, payload_attrs);
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
                        todo!()
                    }
                },
                FromEngine::DownloadedBlocks(blocks) => {
                    if let Some(event) = self.on_downloaded(blocks) {
                        if let Err(err) = self.outgoing.send(EngineApiEvent::FromTree(event)) {
                            error!("Failed to send event: {err:?}");
                        }
                    }
                }
            }

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
                    if let Some(block) =
                        self.state.tree_state.block_by_hash(last_persisted_block_hash)
                    {
                        self.persistence_state.finish(last_persisted_block_hash, block.number);
                        self.remove_persisted_blocks_from_memory();
                    } else {
                        error!("could not find persisted block with hash {last_persisted_block_hash} in memory");
                    }
                }
            }
        }
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

        if self.is_pipeline_active {
            // We can only process new forkchoice updates if the pipeline is idle, since it requires
            // exclusive access to the database
            trace!(target: "consensus::engine", "Pipeline is syncing, skipping forkchoice update");
            return Ok(Some(OnForkChoiceUpdated::syncing()))
        }

        Ok(None)
    }
}

impl<P, E, T> EngineApiTreeHandler for EngineApiTreeHandlerImpl<P, E, T>
where
    P: BlockReader + StateProviderFactory + Clone + 'static,
    E: BlockExecutorProvider,
    T: EngineTypes + 'static,
{
    type Engine = T;

    fn on_downloaded(&mut self, _blocks: Vec<SealedBlockWithSenders>) -> Option<TreeEvent> {
        debug!("not implemented");
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

        let status = if self.is_pipeline_active {
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
                    // TODO: isn't this check redundant?
                    // check if the block's parent is already marked as invalid
                    // if let Some(status) = self
                    //     .check_invalid_ancestor_with_head(block.parent_hash, block.hash())
                    //     .map_err(|error| {
                    //         InsertBlockError::new(block, InsertBlockErrorKind::Provider(error))
                    //     })?
                    // {
                    //     return Ok(status)
                    // }

                    // not known to be invalid, but we don't know anything else
                    PayloadStatusEnum::Syncing
                }
            };
            PayloadStatus::new(status, latest_valid_hash)
        };

        let mut outcome = TreeOutcome::new(status);
        if outcome.outcome.is_valid() {
            if let Some(target) = self.state.forkchoice_state_tracker.sync_target_state() {
                if target.head_block_hash == block_hash {
                    outcome = outcome
                        .with_event(TreeEvent::TreeAction(TreeAction::MakeCanonical(block_hash)));
                }
            }
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

        todo!()
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

/// Represents the tree state kept in memory.
trait InMemoryState: Send + Sync {
    /// Returns the state for a given block hash.
    fn in_memory_state_by_hash(&self, hash: B256) -> Option<Arc<State>>;
    /// Returns the state for a given block number.
    fn in_memory_state_by_number(&self, number: u64) -> Option<Arc<State>>;
    /// Returns the current chain head.
    fn in_memory_current_head(&self) -> Option<(BlockNumber, B256)>;
    /// Returns the pending block hash.
    fn in_memory_pending_block_hash(&self) -> Option<B256>;
    /// Returns the pending state corresponding to the current head plus one,
    /// from the payload received in newPayload that does not have a FCU yet.
    fn in_memory_pending_state(&self) -> Option<Arc<State>>;
}

/// State composed of a block hash, and the receipts, state and transactions root
/// after executing it.
#[derive(Debug, PartialEq, Eq)]
pub struct State {
    /// Block hash defining the state.
    block_hash: B256,
    /// Block number defining the state.
    block_number: BlockNumber,
    /// State root after applying the block.
    state_root: B256,
    /// Transactions root of the block.
    transactions_root: B256,
    /// Receipts root after applying the block.
    receipts_root: B256,
}

impl State {
    fn new(sealed_block: Arc<SealedBlock>) -> Self {
        Self {
            block_hash: sealed_block.hash(),
            block_number: sealed_block.number,
            state_root: sealed_block.state_root,
            transactions_root: sealed_block.transactions_root,
            receipts_root: sealed_block.receipts_root,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{static_files::StaticFileAction, test_utils::get_executed_blocks};
    use reth_beacon_consensus::EthBeaconConsensus;
    use reth_chainspec::{ChainSpecBuilder, MAINNET};
    use reth_ethereum_engine_primitives::EthEngineTypes;
    use reth_evm::test_utils::MockExecutorProvider;
    use reth_provider::test_utils::MockEthProvider;
    use std::sync::mpsc::{channel, Sender};
    use tokio::sync::mpsc::unbounded_channel;

    struct TestHarness {
        tree: EngineApiTreeHandlerImpl<MockEthProvider, MockExecutorProvider, EthEngineTypes>,
        to_tree_tx: Sender<FromEngine<BeaconEngineMessage<EthEngineTypes>>>,
        blocks: Vec<ExecutedBlock>,
        sf_action_rx: Receiver<StaticFileAction>,
    }

    fn get_default_test_harness(number_of_blocks: u64) -> TestHarness {
        let blocks: Vec<_> = get_executed_blocks(0..number_of_blocks).collect();

        let mut blocks_by_hash = HashMap::new();
        let mut blocks_by_number = BTreeMap::new();
        for block in &blocks {
            blocks_by_hash.insert(block.block().hash(), block.clone());
            blocks_by_number
                .entry(block.block().number)
                .or_insert_with(Vec::new)
                .push(block.clone());
        }
        let tree_state = TreeState { blocks_by_hash, blocks_by_number, ..Default::default() };

        let (action_tx, action_rx) = channel();
        let (sf_action_tx, sf_action_rx) = channel();
        let persistence_handle = PersistenceHandle::new(action_tx, sf_action_tx);

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

        TestHarness {
            tree: EngineApiTreeHandlerImpl::new(
                provider,
                executor_factory,
                consensus,
                payload_validator,
                to_tree_rx,
                from_tree_tx,
                engine_api_tree_state,
                persistence_handle,
            ),
            to_tree_tx,
            blocks,
            sf_action_rx,
        }
    }

    #[tokio::test]
    async fn test_tree_persist_blocks() {
        // we need more than PERSISTENCE_THRESHOLD blocks to trigger the
        // persistence task.
        let TestHarness { tree, to_tree_tx, sf_action_rx, mut blocks } =
            get_default_test_harness(PERSISTENCE_THRESHOLD + 1);
        std::thread::Builder::new().name("Tree Task".to_string()).spawn(|| tree.run()).unwrap();

        // send a message to the tree to enter the main loop.
        to_tree_tx.send(FromEngine::DownloadedBlocks(vec![])).unwrap();

        let received_action = sf_action_rx.recv().expect("Failed to receive saved blocks");
        if let StaticFileAction::WriteExecutionData((saved_blocks, _)) = received_action {
            // only PERSISTENCE_THRESHOLD will be persisted
            blocks.pop();
            assert_eq!(saved_blocks.len() as u64, PERSISTENCE_THRESHOLD);
            assert_eq!(saved_blocks, blocks);
        } else {
            panic!("unexpected action received {received_action:?}");
        }
    }

    #[test]
    fn test_in_memory_state_trait_impl() {
        let TestHarness { tree, to_tree_tx, sf_action_rx, blocks } = get_default_test_harness(10);

        let head_block = blocks.last().unwrap().block();
        let first_block = blocks.first().unwrap().block();

        for executed_block in blocks {
            let sealed_block = executed_block.block();

            let expected_state = State::new(Arc::new(sealed_block.clone()));

            let actual_state_by_hash =
                tree.state.tree_state.in_memory_state_by_hash(sealed_block.hash()).unwrap();
            assert_eq!(expected_state, *actual_state_by_hash);

            let actual_state_by_number =
                tree.state.tree_state.in_memory_state_by_number(sealed_block.number).unwrap();
            assert_eq!(expected_state, *actual_state_by_number);
        }
    }
}
