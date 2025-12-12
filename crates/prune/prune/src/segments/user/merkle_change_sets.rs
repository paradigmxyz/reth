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
        let Some(block_range) = input.get_next_block_range() else {
            trace!(target: "pruner", "No change sets to prune");
            return Ok(SegmentOutput::done())
        };

        let block_range_end = *block_range.end();
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
