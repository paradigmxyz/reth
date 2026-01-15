use crate::stages::merkle::INVALID_STATE_ROOT_ERROR_MESSAGE;
use alloy_consensus::BlockHeader;
use alloy_primitives::BlockNumber;
use reth_consensus::ConsensusError;
use reth_primitives_traits::{GotExpected, SealedHeader};
use reth_provider::{
    BlockNumReader, ChainStateBlockReader, ChangeSetReader, DBProvider, HeaderProvider,
    ProviderError, PruneCheckpointReader, PruneCheckpointWriter, StageCheckpointReader,
    StageCheckpointWriter, StorageChangeSetReader, TrieWriter,
};
use reth_prune_types::{
    PruneCheckpoint, PruneMode, PruneSegment, MERKLE_CHANGESETS_RETENTION_BLOCKS,
};
use reth_stages_api::{
    BlockErrorKind, ExecInput, ExecOutput, Stage, StageCheckpoint, StageError, StageId,
    UnwindInput, UnwindOutput,
};
use reth_trie::{
    updates::TrieUpdates, HashedPostStateSorted, KeccakKeyHasher, StateRoot, TrieInputSorted,
};
use reth_trie_db::{DatabaseHashedPostState, DatabaseStateRoot};
use std::{ops::Range, sync::Arc};
use tracing::{debug, error};

/// The `MerkleChangeSets` stage.
///
/// This stage processes and maintains trie changesets from the finalized block to the latest block.
#[derive(Debug, Clone)]
pub struct MerkleChangeSets {
    /// The number of blocks to retain changesets for, used as a fallback when the finalized block
    /// is not found. Defaults to [`MERKLE_CHANGESETS_RETENTION_BLOCKS`] (2 epochs in beacon
    /// chain).
    retention_blocks: u64,
}

impl MerkleChangeSets {
    /// Creates a new `MerkleChangeSets` stage with the default retention blocks.
    pub const fn new() -> Self {
        Self { retention_blocks: MERKLE_CHANGESETS_RETENTION_BLOCKS }
    }

    /// Creates a new `MerkleChangeSets` stage with a custom finalized block height.
    pub const fn with_retention_blocks(retention_blocks: u64) -> Self {
        Self { retention_blocks }
    }

    /// Returns the range of blocks which are already computed. Will return an empty range if none
    /// have been computed.
    fn computed_range<Provider>(
        provider: &Provider,
        checkpoint: Option<StageCheckpoint>,
    ) -> Result<Range<BlockNumber>, StageError>
    where
        Provider: PruneCheckpointReader,
    {
        let to = checkpoint.map(|chk| chk.block_number).unwrap_or_default();

        // Get the prune checkpoint for MerkleChangeSets to use as the lower bound. If there's no
        // prune checkpoint or if the pruned block number is None, return empty range
        let Some(from) = provider
            .get_prune_checkpoint(PruneSegment::MerkleChangeSets)?
            .and_then(|chk| chk.block_number)
            // prune checkpoint indicates the last block pruned, so the block after is the start of
            // the computed data
            .map(|block_number| block_number + 1)
        else {
            return Ok(0..0)
        };

        Ok(from..to + 1)
    }

    /// Determines the target range for changeset computation based on the checkpoint and provider
    /// state.
    ///
    /// Returns the target range (exclusive end) to compute changesets for.
    fn determine_target_range<Provider>(
        &self,
        provider: &Provider,
    ) -> Result<Range<BlockNumber>, StageError>
    where
        Provider: StageCheckpointReader + ChainStateBlockReader,
    {
        // Get merkle checkpoint which represents our target end block
        let merkle_checkpoint = provider
            .get_stage_checkpoint(StageId::MerkleExecute)?
            .map(|checkpoint| checkpoint.block_number)
            .unwrap_or(0);

        let target_end = merkle_checkpoint + 1; // exclusive

        // Calculate the target range based on the finalized block and the target block.
        // We maintain changesets from the finalized block to the latest block.
        let finalized_block = provider.last_finalized_block_number()?;

        // Calculate the fallback start position based on retention blocks
        let retention_based_start = merkle_checkpoint.saturating_sub(self.retention_blocks);

        // If the finalized block was way in the past then we don't want to generate changesets for
        // all of those past blocks; we only care about the recent history.
        //
        // Use maximum of finalized_block and retention_based_start if finalized_block exists,
        // otherwise just use retention_based_start.
        let mut target_start = finalized_block
            .map(|finalized| finalized.saturating_add(1).max(retention_based_start))
            .unwrap_or(retention_based_start);

        // We cannot revert the genesis block; target_start must be >0
        target_start = target_start.max(1);

        Ok(target_start..target_end)
    }

    /// Calculates the trie updates given a [`TrieInputSorted`], asserting that the resulting state
    /// root matches the expected one for the block.
    fn calculate_block_trie_updates<Provider: DBProvider + HeaderProvider>(
        provider: &Provider,
        block_number: BlockNumber,
        input: TrieInputSorted,
    ) -> Result<TrieUpdates, StageError> {
        let (root, trie_updates) =
            StateRoot::overlay_root_from_nodes_with_updates(provider.tx_ref(), input).map_err(
                |e| {
                    error!(
                            target: "sync::stages::merkle_changesets",
                            %e,
                            ?block_number,
                            "Incremental state root failed! {INVALID_STATE_ROOT_ERROR_MESSAGE}");
                    StageError::Fatal(Box::new(e))
                },
            )?;

        let block = provider
            .header_by_number(block_number)?
            .ok_or_else(|| ProviderError::HeaderNotFound(block_number.into()))?;

        let (got, expected) = (root, block.state_root());
        if got != expected {
            // Only seal the header when we need it for the error
            let header = SealedHeader::seal_slow(block);
            error!(
                target: "sync::stages::merkle_changesets",
                ?block_number,
                ?got,
                ?expected,
                "Failed to verify block state root! {INVALID_STATE_ROOT_ERROR_MESSAGE}",
            );
            return Err(StageError::Block {
                error: BlockErrorKind::Validation(ConsensusError::BodyStateRootDiff(
                    GotExpected { got, expected }.into(),
                )),
                block: Box::new(header.block_with_parent()),
            })
        }

        Ok(trie_updates)
    }

    fn populate_range<Provider>(
        provider: &Provider,
        target_range: Range<BlockNumber>,
    ) -> Result<(), StageError>
    where
        Provider: StageCheckpointReader
            + TrieWriter
            + DBProvider
            + HeaderProvider
            + ChainStateBlockReader
            + BlockNumReader
            + ChangeSetReader
            + StorageChangeSetReader,
    {
        let target_start = target_range.start;
        let target_end = target_range.end;
        debug!(
            target: "sync::stages::merkle_changesets",
            ?target_range,
            "Starting trie changeset computation",
        );

        // We need to distinguish a cumulative revert and a per-block revert. A cumulative revert
        // reverts changes starting at db tip all the way to a block. A per-block revert only
        // reverts a block's changes.
        //
        // We need to calculate the cumulative HashedPostState reverts for every block in the
        // target range. The cumulative HashedPostState revert for block N can be calculated as:
        //
        //
        // ```
        // // where `extend` overwrites any shared keys
        // cumulative_state_revert(N) = cumulative_state_revert(N + 1).extend(get_block_state_revert(N))
        // ```
        //
        // We need per-block reverts to calculate the prefix set for each individual block. By
        // using the per-block reverts to calculate cumulative reverts on-the-fly we can save a
        // bunch of memory.
        debug!(
            target: "sync::stages::merkle_changesets",
            ?target_range,
            "Computing per-block state reverts",
        );
        let range_len = target_end - target_start;
        let mut per_block_state_reverts = Vec::with_capacity(range_len as usize);
        for block_number in target_range.clone() {
            per_block_state_reverts.push(HashedPostStateSorted::from_reverts::<KeccakKeyHasher>(
                provider,
                block_number..=block_number,
            )?);
        }

        // Helper to retrieve state revert data for a specific block from the pre-computed array
        let get_block_state_revert = |block_number: BlockNumber| -> &HashedPostStateSorted {
            let index = (block_number - target_start) as usize;
            &per_block_state_reverts[index]
        };

        // Helper to accumulate state reverts from a given block to the target end
        let compute_cumulative_state_revert = |block_number: BlockNumber| -> HashedPostStateSorted {
            let mut cumulative_revert = HashedPostStateSorted::default();
            for n in (block_number..target_end).rev() {
                cumulative_revert.extend_ref(get_block_state_revert(n))
            }
            cumulative_revert
        };

        // To calculate the changeset for a block, we first need the TrieUpdates which are
        // generated as a result of processing the block. To get these we need:
        // 1) The TrieUpdates which revert the db's trie to _prior_ to the block
        // 2) The HashedPostStateSorted to revert the db's state to _after_ the block
        //
        // To get (1) for `target_start` we need to do a big state root calculation which takes
        // into account all changes between that block and db tip. For each block after the
        // `target_start` we can update (1) using the TrieUpdates which were output by the previous
        // block, only targeting the state changes of that block.
        debug!(
            target: "sync::stages::merkle_changesets",
            ?target_start,
            "Computing trie state at starting block",
        );
        let initial_state = compute_cumulative_state_revert(target_start);
        let initial_prefix_sets = initial_state.construct_prefix_sets();
        let initial_input =
            TrieInputSorted::new(Arc::default(), Arc::new(initial_state), initial_prefix_sets);
        // target_start will be >= 1, see `determine_target_range`.
        let mut nodes = Arc::new(
            Self::calculate_block_trie_updates(provider, target_start - 1, initial_input)?
                .into_sorted(),
        );

        for block_number in target_range {
            debug!(
                target: "sync::stages::merkle_changesets",
                ?block_number,
                "Computing trie updates for block",
            );
            // Revert the state so that this block has been just processed, meaning we take the
            // cumulative revert of the subsequent block.
            let state = Arc::new(compute_cumulative_state_revert(block_number + 1));

            // Construct prefix sets from only this block's `HashedPostStateSorted`, because we only
            // care about trie updates which occurred as a result of this block being processed.
            let prefix_sets = get_block_state_revert(block_number).construct_prefix_sets();

            let input = TrieInputSorted::new(Arc::clone(&nodes), state, prefix_sets);

            // Calculate the trie updates for this block, then apply those updates to the reverts.
            // We calculate the overlay which will be passed into the next step using the trie
            // reverts prior to them being updated.
            let this_trie_updates =
                Self::calculate_block_trie_updates(provider, block_number, input)?.into_sorted();

            let trie_overlay = Arc::clone(&nodes);
            let mut nodes_mut = Arc::unwrap_or_clone(nodes);
            nodes_mut.extend_ref(&this_trie_updates);
            nodes = Arc::new(nodes_mut);

            // Write the changesets to the DB using the trie updates produced by the block, and the
            // trie reverts as the overlay.
            debug!(
                target: "sync::stages::merkle_changesets",
                ?block_number,
                "Writing trie changesets for block",
            );
            provider.write_trie_changesets(
                block_number,
                &this_trie_updates,
                Some(&trie_overlay),
            )?;
        }

        Ok(())
    }
}

impl Default for MerkleChangeSets {
    fn default() -> Self {
        Self::new()
    }
}

impl<Provider> Stage<Provider> for MerkleChangeSets
where
    Provider: StageCheckpointReader
        + TrieWriter
        + DBProvider
        + HeaderProvider
        + ChainStateBlockReader
        + StageCheckpointWriter
        + PruneCheckpointReader
        + PruneCheckpointWriter
        + ChangeSetReader
        + StorageChangeSetReader
        + BlockNumReader,
{
    fn id(&self) -> StageId {
        StageId::MerkleChangeSets
    }

    fn execute(&mut self, provider: &Provider, input: ExecInput) -> Result<ExecOutput, StageError> {
        // Get merkle checkpoint and assert that the target is the same.
        let merkle_checkpoint = provider
            .get_stage_checkpoint(StageId::MerkleExecute)?
            .map(|checkpoint| checkpoint.block_number)
            .unwrap_or(0);

        if input.target.is_none_or(|target| merkle_checkpoint != target) {
            return Err(StageError::Fatal(eyre::eyre!("Cannot sync stage to block {:?} when MerkleExecute is at block {merkle_checkpoint:?}", input.target).into()))
        }

        let mut target_range = self.determine_target_range(provider)?;

        // Get the previously computed range. This will be updated to reflect the populating of the
        // target range.
        let mut computed_range = Self::computed_range(provider, input.checkpoint)?;
        debug!(
            target: "sync::stages::merkle_changesets",
            ?computed_range,
            ?target_range,
            "Got computed and target ranges",
        );

        // We want the target range to not include any data already computed previously, if
        // possible, so we start the target range from the end of the computed range if that is
        // greater.
        //
        // ------------------------------> Block #
        //    |------computed-----|
        //              |-----target-----|
        //                        |--actual--|
        //
        // However, if the target start is less than the previously computed start, we don't want to
        // do this, as it would leave a gap of data at `target_range.start..=computed_range.start`.
        //
        // ------------------------------> Block #
        //         |---computed---|
        //      |-------target-------|
        //      |-------actual-------|
        //
        if target_range.start >= computed_range.start {
            target_range.start = target_range.start.max(computed_range.end);
        }

        // If target range is empty (target_start >= target_end), stage is already successfully
        // executed.
        if target_range.start >= target_range.end {
            return Ok(ExecOutput::done(StageCheckpoint::new(target_range.end.saturating_sub(1))));
        }

        // If our target range is a continuation of the already computed range then we can keep the
        // already computed data.
        if target_range.start == computed_range.end {
            // Clear from target_start onwards to ensure no stale data exists
            provider.clear_trie_changesets_from(target_range.start)?;
            computed_range.end = target_range.end;
        } else {
            // If our target range is not a continuation of the already computed range then we
            // simply clear the computed data, to make sure there's no gaps or conflicts.
            provider.clear_trie_changesets()?;
            computed_range = target_range.clone();
        }

        // Populate the target range with changesets
        Self::populate_range(provider, target_range)?;

        // Update the prune checkpoint to reflect that all data before `computed_range.start`
        // is not available.
        provider.save_prune_checkpoint(
            PruneSegment::MerkleChangeSets,
            PruneCheckpoint {
                block_number: Some(computed_range.start.saturating_sub(1)),
                tx_number: None,
                prune_mode: PruneMode::Before(computed_range.start),
            },
        )?;

        // `computed_range.end` is exclusive.
        let checkpoint = StageCheckpoint::new(computed_range.end.saturating_sub(1));

        Ok(ExecOutput::done(checkpoint))
    }

    fn unwind(
        &mut self,
        provider: &Provider,
        input: UnwindInput,
    ) -> Result<UnwindOutput, StageError> {
        // Unwinding is trivial; just clear everything after the target block.
        provider.clear_trie_changesets_from(input.unwind_to + 1)?;

        let mut computed_range = Self::computed_range(provider, Some(input.checkpoint))?;
        computed_range.end = input.unwind_to + 1;
        if computed_range.start > computed_range.end {
            computed_range.start = computed_range.end;
        }

        // If we've unwound so far that there are no longer enough trie changesets available then
        // simply clear them and the checkpoints, so that on next pipeline startup they will be
        // regenerated.
        //
        // We don't do this check if the target block is not greater than the retention threshold
        // (which happens near genesis), as in that case would could still have all possible
        // changesets even if the total count doesn't meet the threshold.
        debug!(
            target: "sync::stages::merkle_changesets",
            ?computed_range,
            retention_blocks=?self.retention_blocks,
            "Checking if computed range is over retention threshold",
        );
        if input.unwind_to > self.retention_blocks &&
            computed_range.end - computed_range.start < self.retention_blocks
        {
            debug!(
                target: "sync::stages::merkle_changesets",
                ?computed_range,
                retention_blocks=?self.retention_blocks,
                "Clearing checkpoints completely",
            );
            provider.clear_trie_changesets()?;
            provider
                .save_stage_checkpoint(StageId::MerkleChangeSets, StageCheckpoint::default())?;
            return Ok(UnwindOutput { checkpoint: StageCheckpoint::default() })
        }

        // `computed_range.end` is exclusive
        let checkpoint = StageCheckpoint::new(computed_range.end.saturating_sub(1));

        Ok(UnwindOutput { checkpoint })
    }
}
