//! RocksDB `AccountsHistory` pruner segment.
//!
//! Prunes RocksDB history indices only. Reads changesets via `ChangeSetReader` (MDBX or static
//! files) to determine which addresses need their history shards pruned. Does NOT delete MDBX
//! changesets - that's handled by the regular `AccountHistory` segment.

use crate::{
    segments::{PruneInput, Segment},
    PrunerError,
};
use reth_provider::{ChangeSetReader, PruneShardOutcome, RocksDBProviderFactory};
use reth_prune_types::{
    PruneMode, PrunePurpose, PruneSegment, SegmentOutput, SegmentOutputCheckpoint,
};
use rustc_hash::FxHashMap;
use tracing::{instrument, trace};

#[derive(Debug)]
pub struct AccountsHistoryPruner {
    mode: PruneMode,
}

impl AccountsHistoryPruner {
    pub const fn new(mode: PruneMode) -> Self {
        Self { mode }
    }
}

impl<Provider> Segment<Provider> for AccountsHistoryPruner
where
    Provider: ChangeSetReader + RocksDBProviderFactory,
{
    fn segment(&self) -> PruneSegment {
        PruneSegment::AccountHistory
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
                trace!(target: "pruner", "No account history to prune");
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

        let mut highest_deleted_accounts = FxHashMap::default();
        let mut last_changeset_pruned_block = None;
        let mut scanned_changesets = 0usize;
        let mut done = true;

        for block in range {
            if limiter.is_limit_reached() {
                done = false;
                break;
            }

            let changes = provider.account_block_changeset(block)?;
            let changes_count = changes.len();

            for change in changes {
                highest_deleted_accounts.insert(change.address, block);
            }

            scanned_changesets += changes_count;
            limiter.increment_deleted_entries_count_by(changes_count);
            last_changeset_pruned_block = Some(block);
        }
        trace!(target: "pruner", scanned = %scanned_changesets, %done, "Scanned account changesets");

        let last_changeset_pruned_block = last_changeset_pruned_block
            .map(|block_number| if done { block_number } else { block_number.saturating_sub(1) })
            .unwrap_or(range_end);

        let mut keys_deleted = 0usize;
        let mut keys_updated = 0usize;

        provider.with_rocksdb_batch(|mut batch| {
            for (address, highest_block) in &highest_deleted_accounts {
                match batch.prune_account_history_to(*address, *highest_block)? {
                    PruneShardOutcome::Deleted => keys_deleted += 1,
                    PruneShardOutcome::Updated => keys_updated += 1,
                    PruneShardOutcome::Unchanged => {}
                }
            }
            Ok(((), Some(batch.into_inner())))
        })?;

        trace!(target: "pruner", keys_deleted, keys_updated, %done, "Pruned account history (RocksDB indices)");

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
