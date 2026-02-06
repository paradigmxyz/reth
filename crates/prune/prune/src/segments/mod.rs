mod receipts;
mod set;
mod user;

use crate::{PruneLimiter, PrunerError};
use alloy_primitives::{BlockNumber, TxNumber};
use reth_provider::{
    errors::provider::ProviderResult, BlockReader, PruneCheckpointWriter, StaticFileProviderFactory,
};
use reth_prune_types::{
    PruneCheckpoint, PruneMode, PruneProgress, PrunePurpose, PruneSegment, SegmentOutput,
    SegmentOutputCheckpoint,
};
use reth_stages_types::StageId;
use reth_static_file_types::StaticFileSegment;
pub use set::SegmentSet;
use std::{fmt::Debug, ops::RangeInclusive};
use tracing::error;
pub use user::{
    AccountHistory, Bodies, Receipts as UserReceipts, ReceiptsByLogs, SenderRecovery,
    StorageHistory, TransactionLookup,
};

/// Prunes data from static files for a given segment.
///
/// This is a generic helper function used by both receipts and bodies pruning
/// when data is stored in static files.
///
/// The checkpoint block number is set to the highest block in the actually deleted files,
/// not `input.to_block`, since `to_block` might refer to a block in the middle of an
/// undeleted file.
pub(crate) fn prune_static_files<Provider>(
    provider: &Provider,
    input: PruneInput,
    segment: StaticFileSegment,
) -> Result<SegmentOutput, PrunerError>
where
    Provider: StaticFileProviderFactory,
{
    let deleted_headers =
        provider.static_file_provider().delete_segment_below_block(segment, input.to_block + 1)?;

    if deleted_headers.is_empty() {
        return Ok(SegmentOutput {
            progress: PruneProgress::Finished,
            pruned: 0,
            checkpoint: input
                .previous_checkpoint
                .map(SegmentOutputCheckpoint::from_prune_checkpoint),
        })
    }

    let tx_ranges = deleted_headers.iter().filter_map(|header| header.tx_range());

    let pruned = tx_ranges.clone().map(|range| range.len()).sum::<u64>() as usize;

    // The highest block number in the deleted files is the actual checkpoint.
    let checkpoint_block = deleted_headers
        .iter()
        .filter_map(|header| header.block_range())
        .map(|range| range.end())
        .max();

    Ok(SegmentOutput {
        progress: PruneProgress::Finished,
        pruned,
        checkpoint: Some(SegmentOutputCheckpoint {
            block_number: checkpoint_block,
            tx_number: tx_ranges.map(|range| range.end()).max(),
        }),
    })
}

/// Deletes ALL static file jars for a given segment.
///
/// This is used for `PruneMode::Full` where all data should be removed, including the highest jar.
/// Unlike [`prune_static_files`], this does not preserve the most recent jar.
pub(crate) fn delete_static_files_segment<Provider>(
    provider: &Provider,
    input: PruneInput,
    segment: StaticFileSegment,
) -> Result<SegmentOutput, PrunerError>
where
    Provider: StaticFileProviderFactory,
{
    let deleted_headers = provider.static_file_provider().delete_segment(segment)?;

    if deleted_headers.is_empty() {
        return Ok(SegmentOutput::done())
    }

    let tx_ranges = deleted_headers.iter().filter_map(|header| header.tx_range());

    let pruned = tx_ranges.clone().map(|range| range.len()).sum::<u64>() as usize;

    Ok(SegmentOutput {
        progress: PruneProgress::Finished,
        pruned,
        checkpoint: Some(SegmentOutputCheckpoint {
            block_number: Some(input.to_block),
            tx_number: tx_ranges.map(|range| range.end()).max(),
        }),
    })
}

/// A segment represents a pruning of some portion of the data.
///
/// Segments are called from [`Pruner`](crate::Pruner) with the following lifecycle:
/// 1. Call [`Segment::prune`] with `delete_limit` of [`PruneInput`].
/// 2. If [`Segment::prune`] returned a [`Some`] in `checkpoint` of [`SegmentOutput`], call
///    [`Segment::save_checkpoint`].
/// 3. Subtract `pruned` of [`SegmentOutput`] from `delete_limit` of next [`PruneInput`].
pub trait Segment<Provider>: Debug + Send + Sync {
    /// Segment of data that's pruned.
    fn segment(&self) -> PruneSegment;

    /// Prune mode with which the segment was initialized.
    fn mode(&self) -> Option<PruneMode>;

    /// Purpose of the segment.
    fn purpose(&self) -> PrunePurpose;

    /// Prune data for [`Self::segment`] using the provided input.
    fn prune(&self, provider: &Provider, input: PruneInput) -> Result<SegmentOutput, PrunerError>;

    /// Save checkpoint for [`Self::segment`] to the database.
    fn save_checkpoint(
        &self,
        provider: &Provider,
        checkpoint: PruneCheckpoint,
    ) -> ProviderResult<()>
    where
        Provider: PruneCheckpointWriter,
    {
        provider.save_prune_checkpoint(self.segment(), checkpoint)
    }

    /// Returns the stage this segment depends on, if any.
    ///
    /// If this returns `Some(stage_id)`, the pruner will skip this segment if the stage
    /// has not yet caught up with the `Finish` stage checkpoint.
    fn required_stage(&self) -> Option<StageId> {
        None
    }
}

/// Segment pruning input, see [`Segment::prune`].
#[derive(Debug)]
#[cfg_attr(test, derive(Clone))]
pub struct PruneInput {
    pub(crate) previous_checkpoint: Option<PruneCheckpoint>,
    /// Target block up to which the pruning needs to be done, inclusive.
    pub(crate) to_block: BlockNumber,
    /// Limits pruning of a segment.
    pub(crate) limiter: PruneLimiter,
}

impl PruneInput {
    /// Get next inclusive tx number range to prune according to the checkpoint and `to_block` block
    /// number.
    ///
    /// To get the range start:
    /// 1. If checkpoint exists, get next block body and return its first tx number.
    /// 2. If checkpoint doesn't exist, return 0.
    ///
    /// To get the range end: get last tx number for `to_block`.
    pub(crate) fn get_next_tx_num_range<Provider: BlockReader>(
        &self,
        provider: &Provider,
    ) -> ProviderResult<Option<RangeInclusive<TxNumber>>> {
        let from_tx_number = self.previous_checkpoint
            // Checkpoint exists, prune from the next transaction after the highest pruned one
            .and_then(|checkpoint| match checkpoint.tx_number {
                Some(tx_number) => Some(tx_number + 1),
                _ => {
                    error!(target: "pruner", ?checkpoint, "Expected transaction number in prune checkpoint, found None");
                    None
                },
            })
            // No checkpoint exists, prune from genesis
            .unwrap_or_default();

        let to_tx_number = match provider.block_body_indices(self.to_block)? {
            Some(body) => {
                let last_tx = body.last_tx_num();
                if last_tx + body.tx_count() == 0 {
                    // Prevents a scenario where the pruner correctly starts at a finalized block,
                    // but the first transaction (tx_num = 0) only appears on a non-finalized one.
                    // Should only happen on a test/hive scenario.
                    return Ok(None)
                }
                last_tx
            }
            None => return Ok(None),
        };

        let range = from_tx_number..=to_tx_number;
        if range.is_empty() {
            return Ok(None)
        }

        Ok(Some(range))
    }

    /// Get next inclusive block range to prune according to the checkpoint, `to_block` block
    /// number and `limit`.
    ///
    /// To get the range start (`from_block`):
    /// 1. If checkpoint exists, use next block.
    /// 2. If checkpoint doesn't exist, use block 0.
    ///
    /// To get the range end: use block `to_block`.
    pub(crate) fn get_next_block_range(&self) -> Option<RangeInclusive<BlockNumber>> {
        let from_block = self.get_start_next_block_range();
        let range = from_block..=self.to_block;
        if range.is_empty() {
            return None
        }

        Some(range)
    }

    /// Returns the start of the next block range.
    ///
    /// 1. If checkpoint exists, use next block.
    /// 2. If checkpoint doesn't exist, use block 0.
    pub(crate) fn get_start_next_block_range(&self) -> u64 {
        self.previous_checkpoint
            .and_then(|checkpoint| checkpoint.block_number)
            // Checkpoint exists, prune from the next block after the highest pruned one
            .map(|block_number| block_number + 1)
            // No checkpoint exists, prune from genesis
            .unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::B256;
    use reth_provider::{
        providers::BlockchainProvider,
        test_utils::{create_test_provider_factory, MockEthProvider},
        BlockWriter,
    };
    use reth_testing_utils::generators::{self, random_block_range, BlockRangeParams};

    #[test]
    fn test_prune_input_get_next_tx_num_range_no_to_block() {
        let input = PruneInput {
            previous_checkpoint: None,
            to_block: 10,
            limiter: PruneLimiter::default(),
        };

        // Default provider with no block corresponding to block 10
        let provider = MockEthProvider::default();

        // No block body for block 10, expected None
        let range = input.get_next_tx_num_range(&provider).expect("Expected range");
        assert!(range.is_none());
    }

    #[test]
    fn test_prune_input_get_next_tx_num_range_no_tx() {
        let input = PruneInput {
            previous_checkpoint: None,
            to_block: 10,
            limiter: PruneLimiter::default(),
        };

        let mut rng = generators::rng();
        let factory = create_test_provider_factory();

        // Generate 10 random blocks with no transactions
        let blocks = random_block_range(
            &mut rng,
            0..=10,
            BlockRangeParams { parent: Some(B256::ZERO), tx_count: 0..1, ..Default::default() },
        );

        // Insert the blocks into the database
        let provider_rw = factory.provider_rw().expect("failed to get provider_rw");
        for block in &blocks {
            provider_rw
                .insert_block(
                    &block.clone().try_recover().expect("failed to seal block with senders"),
                )
                .expect("failed to insert block");
        }
        provider_rw.commit().expect("failed to commit");

        // Create a new provider
        let provider = BlockchainProvider::new(factory).unwrap();

        // Since there are no transactions, expected None
        let range = input.get_next_tx_num_range(&provider).expect("Expected range");
        assert!(range.is_none());
    }

    #[test]
    fn test_prune_input_get_next_tx_num_range_valid() {
        // Create a new prune input
        let input = PruneInput {
            previous_checkpoint: None,
            to_block: 10,
            limiter: PruneLimiter::default(),
        };

        let mut rng = generators::rng();
        let factory = create_test_provider_factory();

        // Generate 10 random blocks with some transactions
        let blocks = random_block_range(
            &mut rng,
            0..=10,
            BlockRangeParams { parent: Some(B256::ZERO), tx_count: 0..5, ..Default::default() },
        );

        // Insert the blocks into the database
        let provider_rw = factory.provider_rw().expect("failed to get provider_rw");
        for block in &blocks {
            provider_rw
                .insert_block(
                    &block.clone().try_recover().expect("failed to seal block with senders"),
                )
                .expect("failed to insert block");
        }
        provider_rw.commit().expect("failed to commit");

        // Create a new provider
        let provider = BlockchainProvider::new(factory).unwrap();

        // Get the next tx number range
        let range = input.get_next_tx_num_range(&provider).expect("Expected range").unwrap();

        // Calculate the total number of transactions
        let num_txs = blocks.iter().map(|block| block.transaction_count() as u64).sum::<u64>();

        assert_eq!(range, 0..=num_txs - 1);
    }

    #[test]
    fn test_prune_input_get_next_tx_checkpoint_without_tx_number() {
        // Create a prune input with a previous checkpoint without a tx number (unexpected)
        let input = PruneInput {
            previous_checkpoint: Some(PruneCheckpoint {
                block_number: Some(5),
                tx_number: None,
                prune_mode: PruneMode::Full,
            }),
            to_block: 10,
            limiter: PruneLimiter::default(),
        };

        let mut rng = generators::rng();
        let factory = create_test_provider_factory();

        // Generate 10 random blocks
        let blocks = random_block_range(
            &mut rng,
            0..=10,
            BlockRangeParams { parent: Some(B256::ZERO), tx_count: 0..5, ..Default::default() },
        );

        // Insert the blocks into the database
        let provider_rw = factory.provider_rw().expect("failed to get provider_rw");
        for block in &blocks {
            provider_rw
                .insert_block(
                    &block.clone().try_recover().expect("failed to seal block with senders"),
                )
                .expect("failed to insert block");
        }
        provider_rw.commit().expect("failed to commit");

        // Create a new provider
        let provider = BlockchainProvider::new(factory).unwrap();

        // Fetch the range and check if it is correct
        let range = input.get_next_tx_num_range(&provider).expect("Expected range").unwrap();

        // Calculate the total number of transactions
        let num_txs = blocks.iter().map(|block| block.transaction_count() as u64).sum::<u64>();

        assert_eq!(range, 0..=num_txs - 1,);
    }

    #[test]
    fn test_prune_input_get_next_tx_empty_range() {
        // Create a new provider via factory
        let mut rng = generators::rng();
        let factory = create_test_provider_factory();

        // Generate 10 random blocks
        let blocks = random_block_range(
            &mut rng,
            0..=10,
            BlockRangeParams { parent: Some(B256::ZERO), tx_count: 0..5, ..Default::default() },
        );

        // Insert the blocks into the database
        let provider_rw = factory.provider_rw().expect("failed to get provider_rw");
        for block in &blocks {
            provider_rw
                .insert_block(
                    &block.clone().try_recover().expect("failed to seal block with senders"),
                )
                .expect("failed to insert block");
        }
        provider_rw.commit().expect("failed to commit");

        // Create a new provider
        let provider = BlockchainProvider::new(factory).unwrap();

        // Get the last tx number
        // Calculate the total number of transactions
        let num_txs = blocks.iter().map(|block| block.transaction_count() as u64).sum::<u64>();
        let max_range = num_txs - 1;

        // Create a prune input with a previous checkpoint that is the last tx number
        let input = PruneInput {
            previous_checkpoint: Some(PruneCheckpoint {
                block_number: Some(5),
                tx_number: Some(max_range),
                prune_mode: PruneMode::Full,
            }),
            to_block: 10,
            limiter: PruneLimiter::default(),
        };

        // We expect an empty range since the previous checkpoint is the last tx number
        let range = input.get_next_tx_num_range(&provider).expect("Expected range");
        assert!(range.is_none());
    }
}
