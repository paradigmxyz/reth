//! `RocksDB` `StoragesHistory` pruner segment.

#![cfg(all(unix, feature = "rocksdb"))]

use crate::{
    db_ext::DbTxPruneExt,
    segments::{PruneInput, Segment},
    PrunerError,
};
use reth_db_api::{models::BlockNumberAddress, tables, transaction::DbTxMut};
use reth_provider::{DBProvider, PruneShardOutcome, RocksDBProviderFactory};
use reth_prune_types::{
    PruneMode, PrunePurpose, PruneSegment, SegmentOutput, SegmentOutputCheckpoint,
};
use rustc_hash::FxHashMap;
use tracing::{instrument, trace};

/// Number of storage history tables to prune in one step.
///
/// Storage History consists of two tables: [`tables::StorageChangeSets`] (MDBX) and
/// [`tables::StoragesHistory`] (`RocksDB`). We want to prune them to the same block number.
const STORAGE_HISTORY_TABLES_TO_PRUNE: usize = 2;

/// RocksDB-based `StoragesHistory` pruner segment.
///
/// This segment prunes:
/// 1. [`tables::StorageChangeSets`] from MDBX (the authoritative changeset data)
/// 2. [`tables::StoragesHistory`] from `RocksDB` (the history indices)
///
/// The `RocksDB` history pruning is changeset-driven: we only prune `RocksDB` shards
/// for (address, slot) pairs that had changesets pruned in this run.
#[derive(Debug)]
pub struct StoragesHistoryPruner {
    mode: PruneMode,
}

impl StoragesHistoryPruner {
    /// Creates a new [`StoragesHistoryPruner`] with the given prune mode.
    pub const fn new(mode: PruneMode) -> Self {
        Self { mode }
    }
}

impl<Provider> Segment<Provider> for StoragesHistoryPruner
where
    Provider: DBProvider<Tx: DbTxMut> + RocksDBProviderFactory,
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

        let mut limiter = if let Some(limit) = input.limiter.deleted_entries_limit() {
            let per_table = limit.saturating_div(STORAGE_HISTORY_TABLES_TO_PRUNE).max(1);
            input.limiter.set_deleted_entries_limit(per_table)
        } else {
            input.limiter
        };
        if limiter.is_limit_reached() {
            return Ok(SegmentOutput::not_done(
                limiter.interrupt_reason(),
                input.previous_checkpoint.map(SegmentOutputCheckpoint::from_prune_checkpoint),
            ))
        }

        let mut last_changeset_pruned_block = None;
        // Deleted storage changeset keys (address, storage key) with the highest block number
        // deleted for that key.
        let mut highest_deleted_storages = FxHashMap::default();
        let (pruned_changesets, done) =
            provider.tx_ref().prune_table_with_range::<tables::StorageChangeSets>(
                BlockNumberAddress::range(range),
                &mut limiter,
                |_| false,
                |(BlockNumberAddress((block_number, address)), entry)| {
                    highest_deleted_storages.insert((address, entry.key), block_number);
                    last_changeset_pruned_block = Some(block_number);
                },
            )?;
        trace!(target: "pruner", deleted = %pruned_changesets, %done, "Pruned storage history (changesets)");

        let last_changeset_pruned_block = last_changeset_pruned_block
            // If there's more storage changesets to prune, set the checkpoint block number to
            // previous, so we could finish pruning its storage changesets on the next run.
            .map(|block_number| if done { block_number } else { block_number.saturating_sub(1) })
            .unwrap_or(range_end);

        // Prune RocksDB history shards for affected storage slots
        let mut deleted_shards = 0usize;
        let mut updated_shards = 0usize;

        provider.with_rocksdb_batch(|mut batch| {
            for ((address, storage_key), highest_block) in &highest_deleted_storages {
                let to_block = *highest_block;
                match batch.prune_storage_history_to(*address, *storage_key, to_block)? {
                    PruneShardOutcome::Deleted => deleted_shards += 1,
                    PruneShardOutcome::Updated => updated_shards += 1,
                    PruneShardOutcome::Unchanged => {}
                }
            }
            Ok(((), Some(batch.into_inner())))
        })?;

        trace!(target: "pruner", deleted = deleted_shards, updated = updated_shards, %done, "Pruned storage history (RocksDB indices)");

        let progress = limiter.progress(done);

        Ok(SegmentOutput {
            progress,
            pruned: pruned_changesets + deleted_shards,
            checkpoint: Some(SegmentOutputCheckpoint {
                block_number: Some(last_changeset_pruned_block),
                tx_number: None,
            }),
        })
    }
}
