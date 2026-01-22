//! `RocksDB` `AccountsHistory` pruner segment.
//!
//! # Atomicity Limitation
//!
//! This segment prunes MDBX changesets first, then `RocksDB` history indices second.
//! These operations are **not atomic** across the two databases:
//!
//! - If `RocksDB` pruning fails after MDBX changesets are committed, `RocksDB` history indices will
//!   be stale (pointing to deleted changesets).
//! - Recovery is problematic because the pruned changeset data (needed to derive affected keys) is
//!   already gone from MDBX.
//!
//! **Operational recommendation**: Treat `RocksDB` pruning errors as serious. If
//! `RocksDB` fails, consider rebuilding history indices rather than assuming
//! re-running prune will fix the inconsistency.

use crate::{
    db_ext::DbTxPruneExt,
    segments::{PruneInput, Segment},
    PrunerError,
};
use reth_db_api::{tables, transaction::DbTxMut};
use reth_provider::{DBProvider, PruneShardOutcome, RocksDBProviderFactory};
use reth_prune_types::{
    PruneMode, PrunePurpose, PruneSegment, SegmentOutput, SegmentOutputCheckpoint,
};
use rustc_hash::FxHashMap;
use tracing::{instrument, trace};

/// Number of account history tables to prune in one step.
///
/// Account History consists of two tables: [`tables::AccountChangeSets`] (MDBX) and
/// [`tables::AccountsHistory`] (`RocksDB`). We want to prune them to the same block number.
const ACCOUNT_HISTORY_TABLES_TO_PRUNE: usize = 2;

/// RocksDB-based `AccountsHistory` pruner segment.
///
/// This segment prunes:
/// 1. [`tables::AccountChangeSets`] from MDBX (the authoritative changeset data)
/// 2. [`tables::AccountsHistory`] from `RocksDB` (the history indices)
///
/// The `RocksDB` history pruning is changeset-driven: we only prune `RocksDB` shards
/// for addresses that had changesets pruned in this run.
#[derive(Debug)]
pub struct AccountsHistoryPruner {
    mode: PruneMode,
}

impl AccountsHistoryPruner {
    /// Creates a new [`AccountsHistoryPruner`] with the given prune mode.
    pub const fn new(mode: PruneMode) -> Self {
        Self { mode }
    }
}

impl<Provider> Segment<Provider> for AccountsHistoryPruner
where
    Provider: DBProvider<Tx: DbTxMut> + RocksDBProviderFactory,
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

        let mut limiter = if let Some(limit) = input.limiter.deleted_entries_limit() {
            let per_table = limit.saturating_div(ACCOUNT_HISTORY_TABLES_TO_PRUNE).max(1);
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
        // Deleted account changeset keys (account addresses) with the highest block number deleted
        // for that key.
        let mut highest_deleted_accounts = FxHashMap::default();
        let (pruned_changesets, done) =
            provider.tx_ref().prune_table_with_range::<tables::AccountChangeSets>(
                range,
                &mut limiter,
                |_| false,
                |(block_number, account)| {
                    highest_deleted_accounts.insert(account.address, block_number);
                    last_changeset_pruned_block = Some(block_number);
                },
            )?;
        trace!(target: "pruner", pruned = %pruned_changesets, %done, "Pruned account history (changesets)");

        let last_changeset_pruned_block = last_changeset_pruned_block
            // If there's more account changesets to prune, set the checkpoint block number to
            // previous, so we could finish pruning its account changesets on the next run.
            .map(|block_number| if done { block_number } else { block_number.saturating_sub(1) })
            .unwrap_or(range_end);

        // Prune RocksDB history shards for affected accounts
        let mut deleted_shards = 0usize;
        let mut updated_shards = 0usize;

        provider.with_rocksdb_batch(|mut batch| {
            for (address, highest_block) in &highest_deleted_accounts {
                match batch.prune_account_history_to(*address, *highest_block)? {
                    PruneShardOutcome::Deleted => deleted_shards += 1,
                    PruneShardOutcome::Updated => updated_shards += 1,
                    PruneShardOutcome::Unchanged => {}
                }
            }
            Ok(((), Some(batch.into_inner())))
        })?;

        trace!(target: "pruner", deleted = deleted_shards, updated = updated_shards, %done, "Pruned account history (RocksDB indices)");

        let progress = limiter.progress(done);

        Ok(SegmentOutput {
            progress,
            pruned: pruned_changesets + deleted_shards + updated_shards,
            checkpoint: Some(SegmentOutputCheckpoint {
                block_number: Some(last_changeset_pruned_block),
                tx_number: None,
            }),
        })
    }
}
