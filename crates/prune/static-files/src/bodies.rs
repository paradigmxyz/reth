//! Transaction pruning functionality for static files.

use alloy_primitives::BlockNumber;
use reth_provider::{BlockReader, StaticFileProviderFactory};
use reth_prune_types::{
    PruneMode, PruneProgress, PrunePurpose, PruneSegment, SegmentOutput, SegmentOutputCheckpoint,
};
use reth_static_file_types::StaticFileSegment;
use reth_storage_errors::provider::ProviderResult;

/// Segment responsible for pruning transactions in static files.
///
/// This segment is controlled by the `bodies_history` configuration.
#[derive(Debug)]
pub struct Bodies {
    mode: Option<PruneMode>,
}

impl Bodies {
    /// Creates a new [`Bodies`] segment with the given prune mode.
    pub const fn new(mode: PruneMode) -> Self {
        Self { mode: Some(mode) }
    }

    /// Returns the segment type.
    pub const fn segment(&self) -> PruneSegment {
        PruneSegment::Bodies
    }

    /// Returns the prune mode.
    pub const fn mode(&self) -> Option<PruneMode> {
        self.mode
    }

    /// Returns the purpose of this segment.
    pub const fn purpose(&self) -> PrunePurpose {
        PrunePurpose::User
    }

    /// Prunes transactions in static files based on the target block.
    ///
    /// Read `StaticFileProvider::delete_segment_below_block` for more on static file pruning.
    pub fn prune<Provider>(
        &self,
        provider: &Provider,
        target_block: BlockNumber,
    ) -> ProviderResult<SegmentOutput>
    where
        Provider: StaticFileProviderFactory + BlockReader,
    {
        let deleted_headers = provider
            .static_file_provider()
            .delete_segment_below_block(StaticFileSegment::Transactions, target_block + 1)?;

        if deleted_headers.is_empty() {
            return Ok(SegmentOutput::done())
        }

        let tx_ranges = deleted_headers.iter().filter_map(|header| header.tx_range());

        let pruned = tx_ranges
            .clone()
            .map(|range| range.end().saturating_sub(range.start()).saturating_add(1))
            .sum::<u64>() as usize;

        Ok(SegmentOutput {
            progress: PruneProgress::Finished,
            pruned,
            checkpoint: Some(SegmentOutputCheckpoint {
                block_number: Some(target_block),
                tx_number: tx_ranges.map(|range| range.end()).max(),
            }),
        })
    }
}
