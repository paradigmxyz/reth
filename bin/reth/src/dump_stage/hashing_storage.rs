use crate::{
    db::DbTool,
    dirs::{DbPath, PlatformPath},
    dump_stage::setup,
};
use eyre::Result;
use reth_db::{database::Database, table::TableImporter, tables};
use reth_provider::Transaction;
use reth_stages::{stages::StorageHashingStage, Stage, UnwindInput};
use std::ops::DerefMut;

pub(crate) async fn dump_hashing_storage_stage<DB: Database>(
    db_tool: &mut DbTool<'_, DB>,
    from: u64,
    to: u64,
    output_db: &PlatformPath<DbPath>,
    should_run: bool,
) -> Result<()> {
    if should_run {
        eyre::bail!("StorageHashing stage does not support dry run.")
    }

    let (output_db, tip_block_number) = setup::<DB>(from, to, output_db, db_tool)?;

    unwind_and_copy::<DB>(db_tool, from, tip_block_number, &output_db).await?;

    Ok(())
}

/// Dry-run an unwind to FROM block and copy the necessary table data to the new database.
async fn unwind_and_copy<DB: Database>(
    db_tool: &mut DbTool<'_, DB>,
    from: u64,
    tip_block_number: u64,
    output_db: &reth_db::mdbx::Env<reth_db::mdbx::WriteMap>,
) -> eyre::Result<()> {
    let mut unwind_tx = Transaction::new(db_tool.db)?;
    let mut exec_stage = StorageHashingStage::default();

    exec_stage
        .unwind(
            &mut unwind_tx,
            UnwindInput { unwind_to: from, stage_progress: tip_block_number, bad_block: None },
        )
        .await?;
    let unwind_inner_tx = unwind_tx.deref_mut();

    // TODO optimize we can actually just get the entries we need for both these tables
    output_db.update(|tx| tx.import_dupsort::<tables::PlainStorageState, _>(unwind_inner_tx))??;
    output_db.update(|tx| tx.import_dupsort::<tables::StorageChangeSet, _>(unwind_inner_tx))??;

    unwind_tx.drop()?;

    Ok(())
}
