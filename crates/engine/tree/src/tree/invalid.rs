//! Invalid payload and invalid ancestor handling of the engine API tree handler.

use crate::tree::{
    error::InsertBlockFatalError, payload_validator::EngineValidator, EngineApiTreeHandler,
    WaitForCaches,
};
use alloy_consensus::BlockHeader;
use alloy_eips::eip1898::BlockWithParent;
use alloy_primitives::B256;
use alloy_rpc_types_engine::{PayloadStatus, PayloadStatusEnum, PayloadValidationError};
use reth_engine_primitives::{ConsensusEngineEvent, ExecutionPayload};
use reth_errors::ProviderResult;
use reth_evm::ConfigureEvm;
use reth_payload_primitives::{BuiltPayload, PayloadTypes};
use reth_primitives_traits::{NodePrimitives, SealedBlock};
use reth_provider::{
    BalProvider, BlockReader, ChangeSetReader, DatabaseProviderFactory, PruneCheckpointReader,
    StageCheckpointReader, StateProviderFactory, StateReader, StorageChangeSetReader,
    StorageSettingsCache,
};
use tracing::debug;

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
    pub(super) fn latest_valid_hash_for_invalid_payload(
        &mut self,
        parent_hash: B256,
    ) -> ProviderResult<Option<B256>> {
        // Check if parent exists in side chain or in canonical chain.
        if self.has_block_by_hash(parent_hash)? {
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
            if current_block.is_none() && self.has_block_by_hash(current_hash)? {
                return Ok(Some(current_hash))
            }
        }
        Ok(None)
    }

    /// Prepares the invalid payload response for the given hash, checking the
    /// database for the parent hash and populating the payload status with the latest valid hash
    /// according to the engine api spec.
    fn prepare_invalid_response(&mut self, parent_hash: B256) -> ProviderResult<PayloadStatus> {
        let valid_parent_hash = match self.sealed_header_by_hash(parent_hash)? {
            // Edge case: the `latestValid` field is the zero hash if the parent block is the
            // terminal PoW block, which we need to identify by looking at the parent's block
            // difficulty
            Some(parent) if !parent.difficulty().is_zero() => Some(B256::ZERO),
            Some(_) => Some(parent_hash),
            None => self.latest_valid_hash_for_invalid_payload(parent_hash)?,
        };

        Ok(PayloadStatus::from_status(PayloadStatusEnum::Invalid {
            validation_error: PayloadValidationError::LinksToRejectedPayload.to_string(),
        })
        .with_latest_valid_hash(valid_parent_hash.unwrap_or_default()))
    }

    /// Returns true if the given hash is the last received sync target block.
    ///
    /// See [`ForkchoiceStateTracker::sync_target_state`]
    ///
    /// [`ForkchoiceStateTracker::sync_target_state`]: reth_engine_primitives::ForkchoiceStateTracker::sync_target_state
    pub(super) fn is_sync_target_head(&self, block_hash: B256) -> bool {
        if let Some(target) = self.state.forkchoice_state_tracker.sync_target_state() {
            return target.head_block_hash == block_hash
        }
        false
    }

    /// Returns true if the given hash is part of the last received sync target fork choice update.
    ///
    /// See [`ForkchoiceStateTracker::sync_target_state`]
    ///
    /// [`ForkchoiceStateTracker::sync_target_state`]: reth_engine_primitives::ForkchoiceStateTracker::sync_target_state
    pub(super) fn is_any_sync_target(&self, block_hash: B256) -> bool {
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
    pub(super) fn check_invalid_ancestor_with_head(
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
        self.emit_event(ConsensusEngineEvent::InvalidBlock {
            block: Box::new(head),
            error: PayloadValidationError::LinksToRejectedPayload.to_string(),
        });

        Ok(status)
    }

    /// Finds any invalid ancestor for the given payload.
    ///
    /// This function first checks if the block itself is in the invalid headers cache (to
    /// avoid re-executing a known-invalid block). Then it walks up the chain of buffered
    /// ancestors and checks if any ancestor is marked as invalid.
    ///
    /// The check works by:
    /// 1. Checking if the block hash itself is in the `invalid_headers` map
    /// 2. Finding the lowest buffered ancestor for the given block hash
    /// 3. If the ancestor is the same as the block hash itself, using the parent hash instead
    /// 4. Checking if this ancestor is in the `invalid_headers` map
    ///
    /// Returns the invalid ancestor block info if found, or None if no invalid ancestor exists.
    pub(super) fn find_invalid_ancestor(
        &mut self,
        payload: &T::ExecutionData,
    ) -> Option<BlockWithParent> {
        let parent_hash = payload.parent_hash();
        let block_hash = payload.block_hash();

        // Check if the block itself is already known to be invalid, avoiding re-execution
        if let Some(entry) = self.state.invalid_headers.get(&block_hash) {
            return Some(entry);
        }

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
    pub(super) fn handle_invalid_ancestor_payload(
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
    pub(super) fn check_invalid_ancestor(
        &mut self,
        head: B256,
    ) -> ProviderResult<Option<PayloadStatus>> {
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
}
