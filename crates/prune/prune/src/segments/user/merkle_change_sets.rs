use crate::{
    db_ext::DbTxPruneExt,
    segments::{PruneInput, Segment},
    PrunerError,
};
use alloy_primitives::B256;
use reth_db_api::{models::BlockNumberHashedAddress, table::Value, tables, transaction::DbTxMut};
use reth_primitives_traits::NodePrimitives;
use reth_provider::{
    errors::provider::ProviderResult, BlockReader, ChainStateBlockReader, DBProvider,
    NodePrimitivesProvider, PruneCheckpointWriter, TransactionsProvider,
};
use reth_prune_types::{
    PruneCheckpoint, PruneMode, PrunePurpose, PruneSegment, SegmentOutput, SegmentOutputCheckpoint,
};
use tracing::{instrument, trace};

/// Pruning segment for Merkle trie changesets (`AccountsTrieChangeSets` and
/// `StoragesTrieChangeSets`).
///
/// The pruning behavior depends on the configured mode:
/// - `PruneMode::Full`: Aggressively prunes all changesets up to the finalized block, keeping only
///   changesets from the finalized block onwards (for potential reorgs)
/// - `PruneMode::Distance(n)`: Keeps exactly the last `n` blocks of changesets, regardless of the
///   finalized block position
#[derive(Debug)]
pub struct MerkleChangeSets {
    mode: PruneMode,
}

impl MerkleChangeSets {
    pub const fn new(mode: PruneMode) -> Self {
        Self { mode }
    }
}

impl<Provider> Segment<Provider> for MerkleChangeSets
where
    Provider: DBProvider<Tx: DbTxMut>
        + PruneCheckpointWriter
        + TransactionsProvider
        + BlockReader
        + ChainStateBlockReader
        + NodePrimitivesProvider<Primitives: NodePrimitives<Receipt: Value>>,
{
    fn segment(&self) -> PruneSegment {
        PruneSegment::MerkleChangeSets
    }

    fn mode(&self) -> Option<PruneMode> {
        Some(self.mode)
    }

    fn purpose(&self) -> PrunePurpose {
        PrunePurpose::User
    }

    #[instrument(level = "trace", target = "pruner", skip(self, provider), ret)]
    fn prune(&self, provider: &Provider, input: PruneInput) -> Result<SegmentOutput, PrunerError> {
        // Determine the prune target based on the configured mode
        let prune_to_block = match self.mode {
            PruneMode::Full => {
                // For Full mode, aggressively prune up to the finalized block

                // Prune everything at and before the finalized block
                match provider.last_finalized_block_number()? {
                    Some(num) => num,
                    None => {
                        trace!(target: "pruner", "No finalized block found, skipping merkle changesets pruning");
                        return Ok(SegmentOutput::done())
                    }
                }
            }
            PruneMode::Distance(distance) => {
                // For Distance mode, keep exactly the specified distance of blocks
                // This respects the configured distance regardless of finalized block
                input.to_block.saturating_sub(distance)
            }
            // For Before mode we prune up to, but not including, the specified block
            PruneMode::Before(block_number) => block_number.saturating_sub(1),
        };

        // If there's nothing to prune (e.g., we're at genesis), return early
        if prune_to_block == 0 {
            trace!(target: "pruner", "Target block is at or near genesis, nothing to prune");
            return Ok(SegmentOutput::done())
        }

        // Get the range to prune based on checkpoint and our calculated prune target
        let from_block = input.get_start_next_block_range();
        if from_block > prune_to_block {
            trace!(target: "pruner",
                from_block,
                prune_to_block,
                ?self.mode,
                "Already pruned to target");
            return Ok(SegmentOutput::done())
        }

        let block_range = from_block..=prune_to_block;
        let block_range_end = *block_range.end();

        trace!(target: "pruner",
            ?block_range,
            ?self.mode,
            "Pruning merkle changesets based on configured mode");
        let mut limiter = input.limiter;

        // Create range for StoragesTrieChangeSets which uses BlockNumberHashedAddress as key
        let storage_range_start: BlockNumberHashedAddress =
            (*block_range.start(), B256::ZERO).into();
        let storage_range_end: BlockNumberHashedAddress =
            (*block_range.end() + 1, B256::ZERO).into();
        let storage_range = storage_range_start..storage_range_end;

        let mut last_storages_pruned_block = None;
        let (storages_pruned, done) =
            provider.tx_ref().prune_table_with_range::<tables::StoragesTrieChangeSets>(
                storage_range,
                &mut limiter,
                |_| false,
                |(BlockNumberHashedAddress((block_number, _)), _)| {
                    last_storages_pruned_block = Some(block_number);
                },
            )?;

        trace!(target: "pruner", %storages_pruned, %done, "Pruned storages change sets");

        let mut last_accounts_pruned_block = block_range_end;
        let last_storages_pruned_block = last_storages_pruned_block
            // If there's more storage changesets to prune, set the checkpoint block number to
            // previous, so we could finish pruning its storage changesets on the next run.
            .map(|block_number| if done { block_number } else { block_number.saturating_sub(1) })
            .unwrap_or(block_range_end);

        let (accounts_pruned, done) =
            provider.tx_ref().prune_table_with_range::<tables::AccountsTrieChangeSets>(
                block_range,
                &mut limiter,
                |_| false,
                |row| last_accounts_pruned_block = row.0,
            )?;

        trace!(target: "pruner", %accounts_pruned, %done, "Pruned accounts change sets");

        let progress = limiter.progress(done);

        Ok(SegmentOutput {
            progress,
            pruned: accounts_pruned + storages_pruned,
            checkpoint: Some(SegmentOutputCheckpoint {
                block_number: Some(last_storages_pruned_block.min(last_accounts_pruned_block)),
                tx_number: None,
            }),
        })
    }

    fn save_checkpoint(
        &self,
        provider: &Provider,
        checkpoint: PruneCheckpoint,
    ) -> ProviderResult<()> {
        provider.save_prune_checkpoint(PruneSegment::MerkleChangeSets, checkpoint)
    }
}
