use crate::{engine::DownloadRequest, pipeline::PipelineAction};
use parking_lot::Mutex;
use reth_beacon_consensus::{ForkchoiceStateTracker, InvalidHeaderCache, OnForkChoiceUpdated};
use reth_blockchain_tree_api::{error::InsertBlockError, InsertPayloadOk};
use reth_engine_primitives::EngineTypes;
use reth_errors::ProviderResult;
use reth_payload_validator::ExecutionPayloadValidator;
use reth_primitives::{Block, BlockNumber, SealedBlock, SealedBlockWithSenders, B256};
use reth_provider::BlockReader;
use reth_rpc_types::{
    engine::{CancunPayloadFields, ForkchoiceState, PayloadStatus, PayloadStatusEnum},
    ExecutionPayload,
};
use std::{
    collections::{BTreeMap, HashMap},
    marker::PhantomData,
    sync::Arc,
};
use tracing::*;

/// Represents an executed block stored in-memory.
#[derive(Clone, Debug)]
struct ExecutedBlock {
    block: Arc<SealedBlockWithSenders>,
    state: Arc<()>,
    trie: Arc<()>,
}

/// Keeps track of the state of the tree.
#[derive(Clone, Debug)]
pub struct TreeState {
    /// All executed blocks by hash.
    blocks_by_hash: HashMap<B256, ExecutedBlock>,
    /// Executed blocks grouped by their respective block number.
    blocks_by_number: BTreeMap<BlockNumber, Vec<ExecutedBlock>>,
}

impl TreeState {
    fn block_by_hash(&self, _hash: B256) -> Option<SealedBlock> {
        todo!()
    }

    fn buffer(&mut self) {}

    /// Insert executed block into the state.
    fn insert_executed(&mut self, executed: ExecutedBlock) {
        self.blocks_by_number.entry(executed.block.number).or_default().push(executed.clone());
        let existing = self.blocks_by_hash.insert(executed.block.hash(), executed);
        debug_assert!(existing.is_none(), "inserted duplicate block");
    }

    /// Remove blocks before specified block number.
    fn remove_before(&mut self, block_number: BlockNumber) {
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
}

/// Tracks the state of the engine api internals.
///
/// This type is shareable.
#[derive(Clone, Debug)]
pub struct EngineApiTreeState {
    /// Tracks the state of the blockchain tree.
    tree_state: TreeState,
    /// Tracks the received forkchoice state updates received by the CL.
    forkchoice_state_tracker: ForkchoiceStateTracker,
    /// Tracks the header of invalid payloads that were rejected by the engine because they're
    /// invalid.
    invalid_headers: Arc<Mutex<InvalidHeaderCache>>,
}

/// The type responsible for processing engine API requests.
///
/// TODO: design: should the engine handler functions also accept the response channel or return the
/// result and the caller redirects the response
pub trait EngineApiTreeHandler: Send + Sync + Clone {
    /// The engine type that this handler is for.
    type Engine: EngineTypes;

    /// Invoked when previously requested blocks were downloaded.
    fn on_downloaded(&self, blocks: Vec<SealedBlockWithSenders>) -> Option<TreeEvent>;

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
        &self,
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
        &self,
        state: ForkchoiceState,
        attrs: Option<<Self::Engine as EngineTypes>::PayloadAttributes>,
    ) -> TreeOutcome<Result<OnForkChoiceUpdated, String>>;
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
    pub fn new(outcome: T) -> Self {
        Self { outcome, event: None }
    }

    /// Set event on the outcome.
    pub fn with_event(mut self, event: TreeEvent) -> Self {
        self.event = Some(event);
        self
    }
}

/// Events that can be emitted by the [EngineApiTreeHandler].
#[derive(Debug)]
pub enum TreeEvent {
    /// Pipeline action is needed.
    PipelineAction(PipelineAction),
    /// Block download is needed.
    Download(DownloadRequest),
}

#[derive(Clone, Debug)]
pub struct EngineApiTreeHandlerImpl<P, T: EngineTypes> {
    provider: P,
    payload_validator: ExecutionPayloadValidator,
    state: EngineApiTreeState,
    _marker: PhantomData<T>,
}

impl<P, T> EngineApiTreeHandlerImpl<P, T>
where
    P: BlockReader,
    T: EngineTypes,
{
    /// Return block from database or in-memory state by hash.
    fn block_by_hash(&self, hash: B256) -> ProviderResult<Option<Block>> {
        // check database first
        let mut block = self.provider.block_by_hash(hash)?;
        if block.is_none() {
            // Note: it's fine to return the unsealed block because the caller already has
            // the hash
            block = self.state.tree_state.block_by_hash(hash).map(|block| block.unseal());
        }
        Ok(block)
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
        &self,
        parent_hash: B256,
    ) -> ProviderResult<Option<B256>> {
        // Check if parent exists in side chain or in canonical chain.
        if self.block_by_hash(parent_hash)?.is_some() {
            return Ok(Some(parent_hash))
        }

        // iterate over ancestors in the invalid cache
        // until we encounter the first valid ancestor
        let mut current_hash = parent_hash;
        let mut invalid_headers = self.state.invalid_headers.lock();
        let mut current_header = invalid_headers.get(&current_hash);
        while let Some(header) = current_header {
            current_hash = header.parent_hash;
            current_header = invalid_headers.get(&current_hash);

            // If current_header is None, then the current_hash does not have an invalid
            // ancestor in the cache, check its presence in blockchain tree
            if current_header.is_none() && self.block_by_hash(current_hash)?.is_some() {
                return Ok(Some(current_hash))
            }
        }
        Ok(None)
    }

    fn insert_block_without_senders(
        &self,
        block: SealedBlock,
    ) -> Result<InsertPayloadOk, InsertBlockError> {
        match block.try_seal_with_senders() {
            Ok(block) => self.insert_block(block),
            Err(block) => Err(InsertBlockError::sender_recovery_error(block)),
        }
    }

    fn insert_block(
        &self,
        _block: SealedBlockWithSenders,
    ) -> Result<InsertPayloadOk, InsertBlockError> {
        todo!()
    }
}

impl<P, T> EngineApiTreeHandler for EngineApiTreeHandlerImpl<P, T>
where
    P: BlockReader + Clone,
    T: EngineTypes,
{
    type Engine = T;

    fn on_downloaded(&self, blocks: Vec<SealedBlockWithSenders>) -> Option<TreeEvent> {
        todo!()
    }

    fn on_new_payload(
        &self,
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

        // TODO:
        let _ = self.insert_block_without_senders(block);

        todo!()
    }

    fn on_forkchoice_updated(
        &self,
        state: ForkchoiceState,
        attrs: Option<<Self::Engine as EngineTypes>::PayloadAttributes>,
    ) -> TreeOutcome<Result<OnForkChoiceUpdated, String>> {
        todo!()
    }
}
