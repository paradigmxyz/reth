use crate::{
    db::DbTool,
    dirs::{DbPath, PlatformPath},
};
use eyre::Result;
use reth_db::{
    cursor::DbCursorRO, database::Database, table::TableImporter, tables, transaction::DbTx,
};
use reth_provider::Transaction;
use reth_staged_sync::utils::init::init_db;
use reth_stages::{stages::StorageHashingStage, Stage, StageId, UnwindInput};
use std::ops::DerefMut;
use tracing::info;

pub(crate) async fn dump_hashing_storage_stage<DB: Database>(
    db_tool: &mut DbTool<'_, DB>,
    from: u64,
    to: u64,
    output_db: &PlatformPath<DbPath>,
    dry_run: bool,
) -> Result<()> {
    assert!(from < to, "FROM block should be bigger than TO block.");

    info!(target: "reth::cli", "Creating separate db at {}", output_db);

    let output_db = init_db(output_db)?;

    output_db.update(|tx| {
        tx.import_table_with_range::<tables::BlockTransitionIndex, _>(
            &db_tool.db.tx()?,
            Some(from - 1),
            to + 1,
        )
    })??;

    // Find the latest block to unwind from
    let (tip_block_number, _) = db_tool
        .db
        .view(|tx| tx.cursor_read::<tables::BlockTransitionIndex>()?.last())??
        .expect("some");

    // Dry-run an unwind to FROM block, so we can get the PlainStorageState and
    // PlainAccountState safely. There might be some state dependency from an address
    // which hasn't been changed in the given range.
    {
        let mut unwind_tx = Transaction::new(db_tool.db)?;
        let mut exec_stage = StorageHashingStage::default();

        exec_stage
            .unwind(
                &mut unwind_tx,
                UnwindInput { unwind_to: from, stage_progress: tip_block_number, bad_block: None },
            )
            .await?;

        let unwind_inner_tx = unwind_tx.deref_mut();

        output_db
            .update(|tx| tx.import_dupsort::<tables::PlainStorageState, _>(unwind_inner_tx))??;

        // TODO optimize
        output_db
            .update(|tx| tx.import_dupsort::<tables::StorageChangeSet, _>(unwind_inner_tx))??;

        // We don't want to actually commit these changes to our original database.
        unwind_tx.drop()?;
    }

    // Try to re-execute the stage without committing
    if dry_run {
        info!(target: "reth::cli", "Executing stage. [dry-run]");

        let mut tx = Transaction::new(&output_db)?;

        let mut sh = StorageHashingStage::default();
        sh.clean_threshold = 1;
        sh.execute(
            &mut tx,
            reth_stages::ExecInput {
                previous_stage: Some((StageId("Another"), to)),
                stage_progress: Some(from),
            },
        )
        .await?;

        tx.drop()?;
        info!(target: "reth::cli", "Success.");
    }

    Ok(())
}
