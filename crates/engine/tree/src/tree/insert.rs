//! Payload and block insertion of the engine API tree handler.

use crate::{
    engine::EngineApiEvent,
    tree::{
        error::{
            InsertBlockError, InsertBlockFatalError, InsertBlockProcessingError,
            InsertBlockValidationError, InsertPayloadError,
        },
        payload_validator::{EngineValidator, TreeCtx},
        types::ValidationOutput,
        BlockStatus, EngineApiTreeHandler, InsertPayloadOk, TreeAction, TreeEvent, TreeOutcome,
        TryInsertPayloadResult, WaitForCaches,
    },
};
use alloy_consensus::BlockHeader;
use alloy_eips::{eip1898::BlockWithParent, BlockNumHash, NumHash};
use alloy_primitives::B256;
use alloy_rpc_types_engine::{PayloadStatus, PayloadStatusEnum};
use reth_consensus::Consensus;
use reth_engine_primitives::{ConsensusEngineEvent, ExecutionPayload, SlowBlockInfo};
use reth_errors::{ConsensusError, ProviderResult};
use reth_evm::ConfigureEvm;
use reth_network_p2p::full_block::SealedBlockWithAccessList;
use reth_payload_primitives::{BuiltPayload, NewPayloadError, PayloadTypes};
use reth_primitives_traits::{FastInstant as Instant, NodePrimitives, SealedBlock, SealedHeader};
use reth_provider::{
    BalProvider, BlockReader, ChangeSetReader, DatabaseProviderFactory, PruneCheckpointReader,
    StageCheckpointReader, StateProviderFactory, StateReader, StorageChangeSetReader,
    StorageSettingsCache,
};
use tracing::{debug, error, instrument, trace, warn};

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
    pub(super) fn on_new_payload(
        &mut self,
        payload: T::ExecutionData,
    ) -> Result<TreeOutcome<PayloadStatus>, InsertBlockProcessingError> {
        let _thread_resource_usage =
            self.metrics.engine.new_payload.measure_thread_resource_usage();
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

        let mut outcome = if self.backfill_sync_state.is_idle() {
            self.try_insert_payload(payload)?.into_outcome()
        } else {
            // While buffering, only already known blocks are answered as valid.
            let status = self.try_buffer_payload(payload)?;
            let already_seen = status.is_valid();
            TreeOutcome::new(status).with_already_seen(already_seen)
        };

        // if the block is valid and it is the current sync target head, make it canonical. While
        // backfill is not idle the pipeline owns the database, so leave canonicalization to the end
        // of backfill.
        if outcome.outcome.is_valid() &&
            self.backfill_sync_state.is_idle() &&
            self.is_sync_target_head(block_hash)
        {
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
    #[instrument(level = "debug", target = "engine::tree", skip_all)]
    pub(super) fn try_insert_payload(
        &mut self,
        payload: T::ExecutionData,
    ) -> Result<TryInsertPayloadResult, InsertBlockProcessingError> {
        let block_hash = payload.block_hash();
        let num_hash = payload.num_hash();
        let parent_hash = payload.parent_hash();
        let mut latest_valid_hash = None;

        match self.insert_payload(payload) {
            Ok(status) => {
                let (status, already_seen) = match status {
                    InsertPayloadOk::Inserted(BlockStatus::Valid) => {
                        latest_valid_hash = Some(block_hash);
                        self.try_connect_buffered_blocks(num_hash)?;
                        (PayloadStatusEnum::Valid, false)
                    }
                    InsertPayloadOk::AlreadySeen(BlockStatus::Valid) => {
                        latest_valid_hash = Some(block_hash);
                        (PayloadStatusEnum::Valid, true)
                    }
                    InsertPayloadOk::Inserted(BlockStatus::Disconnected { .. }) => {
                        (PayloadStatusEnum::Syncing, false)
                    }
                    InsertPayloadOk::AlreadySeen(BlockStatus::Disconnected { .. }) => {
                        // not known to be invalid, but we don't know anything else
                        (PayloadStatusEnum::Syncing, true)
                    }
                };

                Ok(TryInsertPayloadResult {
                    status: PayloadStatus::new(status, latest_valid_hash),
                    already_seen,
                })
            }
            Err(error) => {
                let status = match error {
                    InsertPayloadError::Block(error) => self.on_insert_block_error(error)?,
                    InsertPayloadError::Payload(error) => self
                        .on_new_payload_error(error, num_hash, parent_hash)
                        .map_err(InsertBlockFatalError::from)?,
                };

                Ok(TryInsertPayloadResult { status, already_seen: false })
            }
        }
    }

    /// Stores a payload for later processing during backfill sync.
    ///
    /// During backfill, the node lacks the state needed to validate payloads,
    /// so they are buffered (stored in memory) until their parent blocks are synced.
    ///
    /// Returns:
    /// - `Valid`: Block is already known, so it is not validated again
    /// - `Syncing`: Payload successfully buffered
    /// - Error status: Payload is malformed or invalid
    pub(super) fn try_buffer_payload(
        &mut self,
        payload: T::ExecutionData,
    ) -> Result<PayloadStatus, InsertBlockProcessingError> {
        let parent_hash = payload.parent_hash();
        let num_hash = payload.num_hash();

        match self.payload_validator.convert_payload_to_block(payload) {
            // if the block is well-formed and not already known, buffer it for later
            Ok(block) => {
                if self.is_block_known(num_hash).map_err(InsertBlockFatalError::from)? {
                    return Ok(PayloadStatus::new(PayloadStatusEnum::Valid, Some(num_hash.hash)))
                }
                if let Err(error) = self.buffer_block(block) {
                    self.on_insert_block_error(error)
                } else {
                    Ok(PayloadStatus::from_status(PayloadStatusEnum::Syncing))
                }
            }
            Err(error) => Ok(self
                .on_new_payload_error(error, num_hash, parent_hash)
                .map_err(InsertBlockFatalError::from)?),
        }
    }

    /// Returns `true` if a block with the given hash is known, either in memory or in the
    /// database. This is a lightweight existence check that avoids constructing a full
    /// [`SealedHeader`].
    pub(super) fn has_block_by_hash(&self, hash: B256) -> ProviderResult<bool> {
        if self.state.tree_state.contains_hash(&hash) {
            Ok(true)
        } else {
            self.provider.is_known(hash)
        }
    }

    /// Returns `true` if the block was already validated: it is in memory, or persisted at or
    /// below the last persisted block.
    ///
    /// The last persisted block never exceeds the Finish checkpoint, the highest block every stage
    /// processed, so a stored header at or below it belongs to a validated block. Backfill writes
    /// headers above it before executing their blocks. A backfill run that unwinds below it ends
    /// right away, and the last persisted block is lowered before any headers are written again.
    fn is_block_known(&self, block: BlockNumHash) -> ProviderResult<bool> {
        if self.state.tree_state.contains_hash(&block.hash) {
            return Ok(true)
        }
        Ok(block.number <= self.persistence_state.last_persisted_block.number &&
            self.provider.sealed_header_by_hash(block.hash)?.is_some())
    }

    /// Return sealed block header from in-memory state or database by hash.
    pub(super) fn sealed_header_by_hash(
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
    pub(super) fn lowest_buffered_ancestor_or(&self, hash: B256) -> B256 {
        self.state
            .buffer
            .lowest_ancestor(&hash)
            .map(|block| block.parent_hash())
            .unwrap_or_else(|| hash)
    }

    /// Validate if block is correct and satisfies all the consensus rules that concern the header
    /// and block body itself.
    fn validate_block(&self, block: &SealedBlock<N::Block>) -> Result<(), ConsensusError> {
        if let Err(e) = self.consensus.validate_header(block.sealed_header()) {
            error!(target: "engine::tree", block = ?block.block_with_parent(), %e, "Failed to validate header");
            return Err(e)
        }

        if let Err(e) = self.consensus.validate_block_pre_execution(block) {
            error!(target: "engine::tree", block = ?block.block_with_parent(), %e, "Failed to validate block");
            return Err(e)
        }

        Ok(())
    }

    /// Attempts to connect any buffered blocks that are connected to the given parent hash.
    #[instrument(level = "debug", target = "engine::tree", skip(self))]
    pub(super) fn try_connect_buffered_blocks(
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
                        if let Err(InsertBlockProcessingError::Fatal(fatal)) =
                            self.on_insert_block_error(err)
                        {
                            warn!(target: "engine::tree", %fatal, "fatal error occurred while connecting buffered blocks");
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
        self.state.buffer.insert_block(block.into());
        Ok(())
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
            |this, payload| Ok(this.payload_validator.convert_payload_to_block(payload)?.into()),
        )
    }

    pub(super) fn insert_block(
        &mut self,
        block: SealedBlockWithAccessList<N::Block>,
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
    #[instrument(level = "debug", target = "engine::tree", skip_all, fields(?block_id))]
    pub(super) fn insert_block_or_payload<Input, Err>(
        &mut self,
        block_id: BlockWithParent,
        input: Input,
        execute: impl FnOnce(&mut V, Input, TreeCtx<'_, N>) -> Result<ValidationOutput<N>, Err>,
        convert_to_block: impl FnOnce(
            &mut Self,
            Input,
        ) -> Result<SealedBlockWithAccessList<N::Block>, Err>,
    ) -> Result<InsertPayloadOk, Err>
    where
        Err: From<InsertBlockError<N::Block>>,
    {
        let block_insert_start = Instant::now();
        let block_num_hash = block_id.block;
        debug!(target: "engine::tree", block=?block_num_hash, parent = ?block_id.parent, "Inserting new block into tree");

        match self.is_block_known(block_num_hash) {
            Ok(true) => {
                convert_to_block(self, input)?;
                return Ok(InsertPayloadOk::AlreadySeen(BlockStatus::Valid));
            }
            Ok(false) => {}
            Err(err) => {
                let block = convert_to_block(self, input)?;
                return Err(InsertBlockError::new(block.split().0, err.into()).into());
            }
        }

        // Ensure that the parent state is available.
        if !self.state.tree_state.contains_hash(&block_id.parent) {
            let parent_exists = match self.provider.header(block_id.parent) {
                Ok(header) => header.is_some(),
                Err(err) => {
                    let block = convert_to_block(self, input)?;
                    return Err(InsertBlockError::new(block.split().0, err.into()).into());
                }
            };

            if !parent_exists {
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
        }

        // determine whether we are on a fork chain by comparing the block number with the
        // canonical head. This is a simple check that is sufficient for the event emission below.
        // A block is considered a fork if its number is less than or equal to the canonical head,
        // as this indicates there's already a canonical block at that height.
        let is_fork = block_id.block.number <= self.state.tree_state.current_canonical_head.number;

        let ctx = TreeCtx::new(&mut self.state, &self.canonical_in_memory_state);

        let start = Instant::now();

        let ValidationOutput { executed_block: executed, execution_timing_stats: timing_stats } =
            execute(&mut self.payload_validator, input, ctx)?;

        if let Some(raw_bal) = executed.bal().map(|bal| bal.as_raw_bal().clone()) {
            let num_hash = executed.recovered_block().num_hash();
            if let Err(err) = self.provider.bal_store().insert(num_hash, raw_bal) {
                warn!(
                    target: "engine::tree",
                    ?num_hash,
                    %err,
                    "Failed to store validated block access list"
                );
            }
        }

        // Emit slow block event immediately after execution so it appears even when
        // persistence hasn't completed yet (e.g. blocks arriving faster than persistence).
        if let Some(stats) = timing_stats {
            if let Some(threshold) = self.config.slow_block_threshold() {
                let total_duration = stats.execution_duration + stats.state_hash_duration;
                if total_duration > threshold {
                    self.emit_event(ConsensusEngineEvent::SlowBlock(SlowBlockInfo {
                        stats: stats.clone(),
                        commit_duration: None,
                        total_duration,
                    }));
                }
            }
            self.execution_timing_stats.insert(executed.recovered_block().hash(), stats);
        }

        let is_pending = self.state.tree_state.canonical_block_hash() ==
            executed.recovered_block().parent_hash();
        self.state.tree_state.insert_executed(executed.clone());

        if is_pending {
            debug!(target: "engine::tree", pending=?block_num_hash, "updating pending block");
            self.canonical_in_memory_state.set_pending_block(executed.clone());
        }

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
    pub(super) fn on_insert_block_error(
        &mut self,
        error: InsertBlockError<N::Block>,
    ) -> Result<PayloadStatus, InsertBlockProcessingError> {
        let (block, error) = error.split();

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
        // The Amsterdam Engine API requires `latestValidHash: null` for an undecodable BAL.
        // <https://github.com/ethereum/execution-apis/blob/df75e230befef0de56ee8833322ed714bacb479c/src/engine/amsterdam.md?plain=1#L129>
        let latest_valid_hash =
            if matches!(&validation_err, InsertBlockValidationError::BlockAccessListDecode(_)) {
                None
            } else {
                self.latest_valid_hash_for_invalid_payload(block.parent_hash())
                    .map_err(InsertBlockFatalError::from)?
            };

        // keep track of the invalid header unless the consensus impl considers it transient
        let is_transient = match &validation_err {
            InsertBlockValidationError::Consensus(err) => self.consensus.is_transient_error(err),
            _ => false,
        };
        if is_transient {
            warn!(
                target: "engine::tree",
                invalid_hash=%block.hash(),
                invalid_number=block.number(),
                %validation_err,
                "Skipping invalid header cache insert for transient validation error",
            );
        } else {
            self.state.invalid_headers.insert(block.block_with_parent());
        }
        self.emit_event(EngineApiEvent::BeaconConsensus(ConsensusEngineEvent::InvalidBlock {
            block: Box::new(block),
            error: validation_err.to_string(),
        }));

        Ok(PayloadStatus::new(
            PayloadStatusEnum::Invalid { validation_error: validation_err.to_string() },
            latest_valid_hash,
        ))
    }

    /// Handles a [`NewPayloadError`] by converting it to a [`PayloadStatus`].
    pub(super) fn on_new_payload_error(
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
}
