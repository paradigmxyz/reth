//! Sync finalization: stage checkpoints and static file segment advancement.

use crate::{storage::db_err, SnapSyncError};
use reth_db_api::{tables, transaction::DbTxMut};
use reth_provider::DatabaseProviderFactory;
use reth_storage_api::DBProvider;

/// Writes stage checkpoints for all stages that snap sync satisfies.
///
/// After BAL healing completes, the database state corresponds to `target_block`.
/// This records that fact so the pipeline can resume from the correct point.
pub(crate) fn write_snap_stage_checkpoints<F>(
    factory: &F,
    target_block: u64,
) -> Result<(), SnapSyncError>
where
    F: DatabaseProviderFactory,
    F::ProviderRW: DBProvider + reth_provider::StaticFileProviderFactory,
    <F::ProviderRW as DBProvider>::Tx: DbTxMut,
{
    use reth_provider::{StaticFileProviderFactory, StaticFileSegment, StaticFileWriter};
    use reth_stages_types::{StageCheckpoint, StageId};

    let checkpoint = StageCheckpoint::new(target_block);
    let stages = [
        StageId::Bodies,
        StageId::SenderRecovery,
        StageId::Execution,
        StageId::AccountHashing,
        StageId::StorageHashing,
        StageId::TransactionLookup,
        StageId::IndexAccountHistory,
        StageId::IndexStorageHistory,
    ];

    let provider = factory.database_provider_rw().map_err(db_err)?;
    {
        let tx = provider.tx_ref();
        for stage_id in stages {
            tx.put::<tables::StageCheckpoints>(stage_id.to_string(), checkpoint).map_err(db_err)?;
        }
    }

    // Advance static file segments that snap sync did not populate (headers are
    // already filled by Phase A). Without this, the persistence service would fail
    // with `UnexpectedStaticFileBlockNumber` when writing blocks after the snap
    // target because these segments would still be at block 0.
    let segments = [
        StaticFileSegment::Transactions,
        StaticFileSegment::TransactionSenders,
        StaticFileSegment::Receipts,
        StaticFileSegment::AccountChangeSets,
        StaticFileSegment::StorageChangeSets,
    ];
    let sfp = provider.static_file_provider();
    for segment in segments {
        let mut writer = sfp.get_writer(0, segment).map_err(|e| {
            SnapSyncError::Database(format!("static file writer for {segment:?}: {e}"))
        })?;
        writer.ensure_at_block(target_block).map_err(|e| {
            SnapSyncError::Database(format!("ensure_at_block({target_block}) for {segment:?}: {e}"))
        })?;
    }
    sfp.commit().map_err(|e| SnapSyncError::Database(format!("static file commit: {e}")))?;

    provider.commit().map_err(db_err)?;
    Ok(())
}

/// Finalizes snap sync by writing stage checkpoints and advancing static file segments.
#[allow(dead_code)]
pub(crate) fn finalize_snap_sync<F>(factory: &F, target_block: u64) -> Result<(), SnapSyncError>
where
    F: DatabaseProviderFactory,
    F::ProviderRW: DBProvider + reth_provider::StaticFileProviderFactory,
    <F::ProviderRW as DBProvider>::Tx: DbTxMut,
{
    write_snap_stage_checkpoints(factory, target_block)
}
