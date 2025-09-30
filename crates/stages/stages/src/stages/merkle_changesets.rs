use crate::stages::merkle::INVALID_STATE_ROOT_ERROR_MESSAGE;
use alloy_consensus::BlockHeader;
use alloy_primitives::BlockNumber;
use reth_consensus::ConsensusError;
use reth_primitives_traits::{GotExpected, SealedHeader};
use reth_provider::{
    ChainStateBlockReader, DBProvider, HeaderProvider, ProviderError, StageCheckpointReader,
    TrieWriter,
};
use reth_stages_api::{
    BlockErrorKind, CheckpointBlockRange, ExecInput, ExecOutput, MerkleChangeSetsCheckpoint, Stage,
    StageCheckpoint, StageError, StageId, UnwindInput, UnwindOutput,
};
use reth_trie::{updates::TrieUpdates, HashedPostState, KeccakKeyHasher, StateRoot, TrieInput};
use reth_trie_db::{DatabaseHashedPostState, DatabaseStateRoot};
use std::ops::{Range, RangeInclusive};
use tracing::{debug, error};

/// The `MerkleChangeSets` stage.
///
/// This stage processes and maintains trie changesets from the finalized block to the latest block.
#[derive(Debug, Clone)]
pub struct MerkleChangeSets;

impl MerkleChangeSets {
    /// Creates a new `MerkleChangeSets` stage.
    pub const fn new() -> Self {
        Self
    }

    /// Returns the range of blocks which are already computed. Will return an empty range if none
    /// have been computed.
    fn computed_range(checkpoint: Option<StageCheckpoint>) -> Range<BlockNumber> {
        let CheckpointBlockRange { from, to } = checkpoint
            .map(|chk| chk.merkle_changesets_stage_checkpoint().unwrap_or_default())
            .unwrap_or_default()
            .block_range;
        from..to + 1
    }

    /// Calculates the trie updates given a [`TrieInput`], asserting that the resulting state root
    /// matches the expected one for the block.
    fn calculate_block_trie_updates<Provider: DBProvider + HeaderProvider>(
        provider: &Provider,
        block_number: BlockNumber,
        input: TrieInput,
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

        let header = SealedHeader::seal_slow(block);

        let (got, expected) = (root, header.state_root());
        if got != expected {
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
        target_start: BlockNumber,
        target_end: BlockNumber, // inclusive
    ) -> Result<(), StageError>
    where
        Provider: StageCheckpointReader
            + TrieWriter
            + DBProvider
            + HeaderProvider
            + ChainStateBlockReader,
    {
        let target_range = target_start..=target_end;
        debug!(
            target: "sync::stages::merkle_changesets",
            ?target_range,
            "Starting trie changeset computation",
        );

        // We need to distinguish a full revert and a per-block revert. A full revert reverts
        // changes starting at db tip all the way to a block. A per-block revert only reverts
        // a block's changes.
        //
        // We need to calculate the full HashedPostState reverts for every block in the target
        // range. The full HashedPostState revert for block N can be calculated as:
        //
        //
        // ```
        // // where `extend` overwrites any shared keys
        // state_revert(N) = state_revert(N + 1).extend(per_block_state_revert(N))
        // ```
        //
        // We need per-block reverts to the prefix set for each individual block. By using the
        // per-block reverts to calculate full reverts on-the-fly we can save a bunch of memory.
        debug!(
            target: "sync::stages::merkle_changesets",
            ?target_range,
            "Computing per-block state reverts",
        );
        let mut per_block_state_reverts = Vec::new();
        for block_number in target_range.clone() {
            per_block_state_reverts.push(HashedPostState::from_reverts::<KeccakKeyHasher>(
                provider.tx_ref(),
                block_number..=block_number,
            )?);
        }

        let per_block_state_revert = |block_number| -> &HashedPostState {
            &per_block_state_reverts[(block_number - target_start) as usize]
        };

        let state_revert = |block_number| -> HashedPostState {
            let mut r = HashedPostState::default();
            for n in (block_number..=target_end).rev() {
                r.extend_ref(per_block_state_revert(n))
            }
            r
        };

        // To calculate the changeset for a block, we first need the TrieUpdates which are generated
        // as a result of processing the block. To get these we need:
        // 1) The TrieUpdates which revert the db's trie to _prior_ to the block
        // 2) The HashedPostState to revert the db's state to _after_ the block
        //
        // To get (1) for `target_start` we need to do a big state root calculation which takes into
        // account all changes between that block and db tip. For each block after the
        // `target_start` we can update (1) using the TrieUpdates which were output by the previous
        // block only targetting the state changes of that block.
        debug!(
            target: "sync::stages::merkle_changesets",
            ?target_start,
            "Computing trie state at starting block",
        );
        let mut input = TrieInput::default();
        input.state = state_revert(target_start);
        input.prefix_sets = input.state.construct_prefix_sets();
        // TODO handle reverting to genesis
        input.nodes =
            Self::calculate_block_trie_updates(provider, target_start - 1, input.clone())?;

        for block_number in target_range {
            debug!(
                target: "sync::stages::merkle_changesets",
                ?block_number,
                "Computing trie updates for block",
            );
            // Revert the state so that this block has been just processed, meaning we take the full
            // revert of the subsequent block.
            input.state = state_revert(block_number + 1);

            // Construct prefix sets from only this block's `HashedPostState`, because we only care
            // about trie updates which occurred as a result of this block being processed.
            let this_state_revert = per_block_state_revert(block_number);
            input.prefix_sets = this_state_revert.construct_prefix_sets();

            // Calculate the trie updates for this block, then apply those updates to the reverts.
            // We calculate the overlay which will be passed into the next step using the trie
            // reverts prior to them being updated.
            let this_trie_updates =
                Self::calculate_block_trie_updates(provider, block_number, input.clone())?;

            let trie_overlay = input.nodes.clone().into_sorted();
            input.nodes.extend_ref(&this_trie_updates);
            let this_trie_updates = this_trie_updates.into_sorted();

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

    /// Determines the target range for changeset computation based on the checkpoint and provider
    /// state.
    ///
    /// Returns the target range (inclusive) to compute changesets for.
    fn determine_target_range<Provider>(
        checkpoint: Option<StageCheckpoint>,
        provider: &Provider,
    ) -> Result<RangeInclusive<BlockNumber>, StageError>
    where
        Provider: StageCheckpointReader + ChainStateBlockReader,
    {
        // Get merkle checkpoint which represents our target end block
        let merkle_checkpoint = provider
            .get_stage_checkpoint(StageId::MerkleExecute)?
            .map(|checkpoint| checkpoint.block_number)
            .unwrap_or(0);

        // Get the previously computed range
        let computed_range = Self::computed_range(checkpoint);

        // Calculate the target range based on the finalized block and the target block.
        // We maintain changesets from the finalized block to the latest block.
        let finalized_block = provider.last_finalized_block_number()?.unwrap_or(0);
        let mut target_start = finalized_block.saturating_add(1); // Start from block after finalized
        let target_end = merkle_checkpoint; // inclusive

        // We want the target range to not include any data already computed previously, if
        // possible, so we start the target range from the end of the computed range if that is
        // greater.
        //
        // ------------------------------> Block #
        //    |------prev-----|
        //              |-----target-----|
        //                    |--actual--|
        //
        // However, if the target start is less than the previously computed start, we don't want to
        // do this, as it would leave a gap of data at `target_start..=computed_range.start`.
        //
        // ------------------------------> Block #
        //           |---prev---|
        //      |-------target-------|
        //      |-------actual-------|
        //
        if target_start >= computed_range.start {
            target_start = target_start.max(computed_range.end);
        }

        Ok(target_start..=target_end)
    }
}

impl Default for MerkleChangeSets {
    fn default() -> Self {
        Self::new()
    }
}

impl<Provider> Stage<Provider> for MerkleChangeSets
where
    Provider:
        StageCheckpointReader + TrieWriter + DBProvider + HeaderProvider + ChainStateBlockReader,
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

        // Determine the target range for changeset computation
        let target_range = Self::determine_target_range(input.checkpoint, provider)?;
        let target_start = *target_range.start();
        let target_end = *target_range.end();

        // Get the previously computed range and determine how to update it
        let mut computed_range = Self::computed_range(input.checkpoint);

        // Determine if we need to clear all changesets or just from target_start onwards
        if target_start >= computed_range.start {
            // Clear from target_start onwards to ensure no stale data exists
            provider.clear_trie_changesets_from(target_start)?;

            computed_range.end = target_end + 1;
            if computed_range.start == 0 {
                computed_range.start = target_start;
            }
        } else {
            // If the target start is less than the previously computed start, then the target range
            // overlaps entirely with the previously computed one. We therefore need to clear out
            // the previously computed data, so as not to conflict.
            provider.clear_trie_changesets()?;
            computed_range = target_start..target_end + 1;
        }

        // Populate the target range with changesets
        Self::populate_range(provider, target_start, target_end)?;

        let checkpoint_block_range = CheckpointBlockRange {
            from: computed_range.start,
            // computed_range.end is exclusive
            to: computed_range.end.saturating_sub(1),
        };

        let checkpoint = StageCheckpoint::new(checkpoint_block_range.to)
            .with_merkle_changesets_stage_checkpoint(MerkleChangeSetsCheckpoint {
                block_range: checkpoint_block_range,
            });

        Ok(ExecOutput::done(checkpoint))
    }

    fn unwind(
        &mut self,
        provider: &Provider,
        input: UnwindInput,
    ) -> Result<UnwindOutput, StageError> {
        provider.clear_trie_changesets_from(input.unwind_to + 1)?;

        let mut computed_range = Self::computed_range(Some(input.checkpoint));
        computed_range.end = input.unwind_to + 1;
        if computed_range.start > computed_range.end {
            computed_range.start = computed_range.end;
        }

        let checkpoint_block_range = CheckpointBlockRange {
            from: computed_range.start,
            // computed_range.end is exclusive
            to: computed_range.end.saturating_sub(1),
        };

        // Get the finalized block to determine if we need to recompute
        let finalized_block = provider.last_finalized_block_number()?.unwrap_or(0);

        // We will set the checkpoint block range to be "correct" regardless, but if reth needs to
        // recompute some range of changesets on startup we set the checkpoint block to 0 to force
        // the execute stage to run.
        let checkpoint_block = if computed_range.start <= finalized_block {
            // If we've unwound to or before the finalized block, we need to recompute
            0
        } else {
            input.unwind_to
        };

        let checkpoint = StageCheckpoint::new(checkpoint_block)
            .with_merkle_changesets_stage_checkpoint(MerkleChangeSetsCheckpoint {
                block_range: checkpoint_block_range,
            });

        Ok(UnwindOutput { checkpoint })
    }
}
