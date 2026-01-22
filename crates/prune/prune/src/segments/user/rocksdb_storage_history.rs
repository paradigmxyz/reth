//! `RocksDB` storage history index pruner.

use crate::{
    segments::{PruneInput, Segment},
    PrunerError,
};
use reth_provider::{PruneShardOutcome, RocksDBProviderFactory, StorageChangeSetReader};
use reth_prune_types::{
    PruneMode, PrunePurpose, PruneSegment, SegmentOutput, SegmentOutputCheckpoint,
};
use rustc_hash::FxHashMap;
use tracing::{instrument, trace};

#[derive(Debug)]
pub struct StoragesHistoryPruner {
    mode: PruneMode,
}

impl StoragesHistoryPruner {
    pub const fn new(mode: PruneMode) -> Self {
        Self { mode }
    }
}

impl<Provider> Segment<Provider> for StoragesHistoryPruner
where
    Provider: StorageChangeSetReader + RocksDBProviderFactory,
{
    fn segment(&self) -> PruneSegment {
        PruneSegment::StorageHistory
    }

    fn mode(&self) -> Option<PruneMode> {
        Some(self.mode)
    }

    fn purpose(&self) -> PrunePurpose {
        PrunePurpose::User
    }

    #[instrument(target = "pruner", skip(self, provider), ret(level = "trace"))]
    fn prune(&self, provider: &Provider, input: PruneInput) -> Result<SegmentOutput, PrunerError> {
        let range = match input.get_next_block_range() {
            Some(range) => range,
            None => {
                trace!(target: "pruner", "No storage history to prune");
                return Ok(SegmentOutput::done())
            }
        };
        let range_end = *range.end();

        let mut limiter = input.limiter;
        if limiter.is_limit_reached() {
            return Ok(SegmentOutput::not_done(
                limiter.interrupt_reason(),
                input.previous_checkpoint.map(SegmentOutputCheckpoint::from_prune_checkpoint),
            ))
        }

        let mut highest_deleted_storages = FxHashMap::default();
        let mut last_changeset_pruned_block = None;
        let mut scanned_changesets = 0usize;
        let mut done = true;

        for block in range {
            let changes = provider.storage_block_changeset(block)?;
            let changes_count = changes.len();

            for change in changes {
                highest_deleted_storages.insert((change.address, change.key), block);
            }

            scanned_changesets += changes_count;
            limiter.increment_deleted_entries_count_by(changes_count);
            last_changeset_pruned_block = Some(block);

            if limiter.is_limit_reached() {
                done = false;
                break;
            }
        }
        trace!(target: "pruner", scanned = %scanned_changesets, %done, "Scanned storage changesets");

        let last_changeset_pruned_block = last_changeset_pruned_block.unwrap_or(range_end);

        let mut keys_deleted = 0usize;
        let mut keys_updated = 0usize;

        provider.with_rocksdb_batch(|mut batch| {
            for ((address, storage_key), highest_block) in &highest_deleted_storages {
                let prune_to = (*highest_block).min(last_changeset_pruned_block);
                match batch.prune_storage_history_to(*address, *storage_key, prune_to)? {
                    PruneShardOutcome::Deleted => keys_deleted += 1,
                    PruneShardOutcome::Updated => keys_updated += 1,
                    PruneShardOutcome::Unchanged => {}
                }
            }
            Ok(((), Some(batch.into_inner())))
        })?;

        trace!(target: "pruner", keys_deleted, keys_updated, %done, "Pruned storage history (RocksDB indices)");

        let progress = limiter.progress(done);

        Ok(SegmentOutput {
            progress,
            pruned: scanned_changesets + keys_deleted,
            checkpoint: Some(SegmentOutputCheckpoint {
                block_number: Some(last_changeset_pruned_block),
                tx_number: None,
            }),
        })
    }
}
