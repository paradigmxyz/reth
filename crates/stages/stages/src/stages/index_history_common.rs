//! Common utilities for index history stages.
//!
//! This module contains shared helpers used by both [`IndexAccountHistoryStage`] and
//! [`IndexStorageHistoryStage`] to reduce code duplication.

use reth_db_api::{table::Table, transaction::DbTxMut};
use reth_provider::{
    DBProvider, PruneCheckpointReader, PruneCheckpointWriter, RocksDBProviderFactory,
};
use reth_prune_types::{PruneCheckpoint, PruneMode, PrunePurpose, PruneSegment};
use reth_stages_api::{ExecInput, StageCheckpoint, StageError};

/// Advances the checkpoint if the prune mode allows skipping blocks.
///
/// When a prune mode is configured (e.g., `Before(N)` or `Distance(D)`), blocks that
/// would be immediately pruned don't need to be indexed. This function advances the
/// checkpoint past those blocks.
///
/// Also saves a prune checkpoint if one doesn't already exist, to ensure the pruner
/// doesn't skip the unpruned range.
pub(super) fn maybe_advance_checkpoint_for_prune<Provider>(
    provider: &Provider,
    mut input: ExecInput,
    prune_mode: Option<PruneMode>,
    segment: PruneSegment,
) -> Result<ExecInput, StageError>
where
    Provider: PruneCheckpointReader + PruneCheckpointWriter,
{
    if let Some((target_prunable_block, prune_mode)) = prune_mode
        .map(|mode| mode.prune_target_block(input.target(), segment, PrunePurpose::User))
        .transpose()?
        .flatten() &&
        target_prunable_block > input.checkpoint().block_number
    {
        input.checkpoint = Some(StageCheckpoint::new(target_prunable_block));

        // Save prune checkpoint only if we don't have one already.
        // Otherwise, pruner may skip the unpruned range of blocks.
        if provider.get_prune_checkpoint(segment)?.is_none() {
            provider.save_prune_checkpoint(
                segment,
                PruneCheckpoint {
                    block_number: Some(target_prunable_block),
                    tx_number: None,
                    prune_mode,
                },
            )?;
        }
    }
    Ok(input)
}

/// Clears the history table on first sync.
///
/// On first sync we might have history coming from genesis. We clear the table since it's
/// faster to rebuild from scratch.
///
/// Note: `RocksDB` `clear()` executes immediately (not deferred to commit like MDBX),
/// but this is safe for `first_sync` because if we crash before commit, the
/// checkpoint stays at 0 and we'll just clear and rebuild again on restart. The
/// source data (changesets) is intact.
pub(super) fn clear_table_on_first_sync<Provider, T>(
    provider: &Provider,
    use_rocksdb: bool,
) -> Result<(), StageError>
where
    Provider: DBProvider<Tx: DbTxMut> + RocksDBProviderFactory,
    T: Table,
{
    if use_rocksdb {
        provider.rocksdb_provider().clear::<T>()?;
    } else {
        provider.tx_ref().clear::<T>()?;
    }
    Ok(())
}

/// Flushes `RocksDB` if needed after writing history indices.
#[cfg(all(unix, feature = "rocksdb"))]
pub(super) fn flush_rocksdb_if_needed<Provider>(
    provider: &Provider,
    use_rocksdb: bool,
    table_name: &'static str,
) -> Result<(), StageError>
where
    Provider: RocksDBProviderFactory,
{
    if use_rocksdb {
        provider.commit_pending_rocksdb_batches()?;
        provider.rocksdb_provider().flush(&[table_name])?;
    }
    Ok(())
}
