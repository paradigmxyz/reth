use crate::{
    dirs::{DbPath, PlatformPath},
    dump_stage::setup,
    utils::DbTool,
};
use eyre::Result;
use reth_db::{
    cursor::DbCursorRO, database::Database, table::TableImporter, tables, transaction::DbTx,
};
use reth_primitives::MAINNET;
use reth_provider::Transaction;
use reth_stages::{stages::ExecutionStage, Stage, StageId, UnwindInput};
use std::{ops::DerefMut, sync::Arc};
use tracing::info;

pub(crate) async fn dump_execution_stage<DB: Database>(
    db_tool: &mut DbTool<'_, DB>,
    from: u64,
    to: u64,
    output_db: &PlatformPath<DbPath>,
    should_run: bool,
) -> Result<()> {
    let (output_db, tip_block_number) = setup::<DB>(from, to, output_db, db_tool)?;

    import_tables_with_range::<DB>(&output_db, db_tool, from, to)?;

    unwind_and_copy::<DB>(db_tool, from, tip_block_number, &output_db).await?;

    if should_run {
        dry_run(output_db, to, from).await?;
    }

    Ok(())
}

/// Imports all the tables that can be copied over a range.
fn import_tables_with_range<DB: Database>(
    output_db: &reth_db::mdbx::Env<reth_db::mdbx::WriteMap>,
    db_tool: &mut DbTool<'_, DB>,
    from: u64,
    to: u64,
) -> eyre::Result<()> {
    //  We're not sharing the transaction in case the memory grows too much.

    output_db.update(|tx| {
        tx.import_table_with_range::<tables::CanonicalHeaders, _>(&db_tool.db.tx()?, Some(from), to)
    })??;
    output_db.update(|tx| {
        tx.import_table_with_range::<tables::HeaderTD, _>(&db_tool.db.tx()?, Some(from), to)
    })??;
    output_db.update(|tx| {
        tx.import_table_with_range::<tables::Headers, _>(&db_tool.db.tx()?, Some(from), to)
    })??;
    output_db.update(|tx| {
        tx.import_table_with_range::<tables::BlockBodies, _>(&db_tool.db.tx()?, Some(from), to)
    })??;
    output_db.update(|tx| {
        tx.import_table_with_range::<tables::BlockOmmers, _>(&db_tool.db.tx()?, Some(from), to)
    })??;

    // Find range of transactions that need to be copied over
    let (from_tx, to_tx) = db_tool.db.view(|read_tx| {
        let mut read_cursor = read_tx.cursor_read::<tables::BlockBodies>()?;
        let (_, from_block) =
            read_cursor.seek(from)?.ok_or(eyre::eyre!("BlockBody {from} does not exist."))?;
        let (_, to_block) =
            read_cursor.seek(to)?.ok_or(eyre::eyre!("BlockBody {to} does not exist."))?;

        Ok::<(u64, u64), eyre::ErrReport>((
            from_block.start_tx_id,
            to_block.start_tx_id + to_block.tx_count,
        ))
    })??;

    output_db.update(|tx| {
        tx.import_table_with_range::<tables::Transactions, _>(
            &db_tool.db.tx()?,
            Some(from_tx),
            to_tx,
        )
    })??;

    output_db.update(|tx| {
        tx.import_table_with_range::<tables::TxSenders, _>(&db_tool.db.tx()?, Some(from_tx), to_tx)
    })??;

    Ok(())
}

/// Dry-run an unwind to FROM block, so we can get the PlainStorageState and
/// PlainAccountState safely. There might be some state dependency from an address
/// which hasn't been changed in the given range.
async fn unwind_and_copy<DB: Database>(
    db_tool: &mut DbTool<'_, DB>,
    from: u64,
    tip_block_number: u64,
    output_db: &reth_db::mdbx::Env<reth_db::mdbx::WriteMap>,
) -> eyre::Result<()> {
    let mut unwind_tx = Transaction::new(db_tool.db)?;

    let mut exec_stage =
        ExecutionStage::new_with_factory(reth_executor::Factory::new(Arc::new(MAINNET.clone())));

    exec_stage
        .unwind(
            &mut unwind_tx,
            UnwindInput { unwind_to: from, stage_progress: tip_block_number, bad_block: None },
        )
        .await?;

    let unwind_inner_tx = unwind_tx.deref_mut();

    output_db.update(|tx| tx.import_dupsort::<tables::PlainStorageState, _>(unwind_inner_tx))??;
    output_db.update(|tx| tx.import_table::<tables::PlainAccountState, _>(unwind_inner_tx))??;
    output_db.update(|tx| tx.import_table::<tables::Bytecodes, _>(unwind_inner_tx))??;

    unwind_tx.drop()?;

    Ok(())
}

/// Try to re-execute the stage without committing
async fn dry_run(
    output_db: reth_db::mdbx::Env<reth_db::mdbx::WriteMap>,
    to: u64,
    from: u64,
) -> eyre::Result<()> {
    info!(target: "reth::cli", "Executing stage. [dry-run]");

    let mut tx = Transaction::new(&output_db)?;
    let mut exec_stage =
        ExecutionStage::new_with_factory(reth_executor::Factory::new(Arc::new(MAINNET.clone())));

    exec_stage
        .execute(
            &mut tx,
            reth_stages::ExecInput {
                previous_stage: Some((StageId("Another"), to)),
                stage_progress: Some(from),
            },
        )
        .await?;

    tx.drop()?;

    info!(target: "reth::cli", "Success.");

    Ok(())
}
