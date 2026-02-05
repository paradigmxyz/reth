//! Common utilities for history prune segments (account and storage history).

#[cfg(all(unix, feature = "rocksdb"))]
use alloy_primitives::BlockNumber;
#[cfg(all(unix, feature = "rocksdb"))]
use reth_provider::StaticFileProviderFactory;
#[cfg(all(unix, feature = "rocksdb"))]
use reth_prune_types::{SegmentOutput, SegmentOutputCheckpoint};
#[cfg(all(unix, feature = "rocksdb"))]
use reth_static_file_types::StaticFileSegment;

#[cfg(all(unix, feature = "rocksdb"))]
use crate::{PruneLimiter, PrunerError};

/// Builds the [`SegmentOutput`] for `RocksDB` history pruning.
///
/// This consolidates the output construction logic shared by both account and storage
/// history segments when pruning from `RocksDB`.
#[cfg(all(unix, feature = "rocksdb"))]
pub(super) fn build_rocksdb_prune_output(
    limiter: &PruneLimiter,
    done: bool,
    changesets_processed: usize,
    deleted_shards: usize,
    updated_shards: usize,
    last_pruned_block: BlockNumber,
) -> SegmentOutput {
    let progress = limiter.progress(done);
    SegmentOutput {
        progress,
        pruned: changesets_processed + deleted_shards + updated_shards,
        checkpoint: Some(SegmentOutputCheckpoint {
            block_number: Some(last_pruned_block),
            tx_number: None,
        }),
    }
}

/// Deletes the static file segment below the given block when pruning is complete.
///
/// This is only performed when `done` is true to ensure we don't delete static files
/// that still contain unpruned data.
#[cfg(all(unix, feature = "rocksdb"))]
pub(super) fn maybe_delete_static_file_segment<Provider>(
    provider: &Provider,
    done: bool,
    segment: StaticFileSegment,
    last_pruned_block: BlockNumber,
) -> Result<(), PrunerError>
where
    Provider: StaticFileProviderFactory,
{
    if done {
        provider
            .static_file_provider()
            .delete_segment_below_block(segment, last_pruned_block + 1)?;
    }
    Ok(())
}
