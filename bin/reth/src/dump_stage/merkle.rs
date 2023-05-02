use crate::{dump_stage::setup, utils::DbTool};
use eyre::Result;
use reth_db::{database::Database, table::TableImporter, tables};
use reth_primitives::{BlockNumber, MAINNET};
use reth_provider::Transaction;
use reth_stages::{
    stages::{AccountHashingStage, ExecutionStage, MerkleStage, StorageHashingStage},
    Stage, StageId, UnwindInput,
};
use std::{ops::DerefMut, path::PathBuf, sync::Arc};
use tracing::info;

pub(crate) async fn dump_merkle_stage<DB: Database>(
    db_tool: &mut DbTool<'_, DB>,
    from: BlockNumber,
    to: BlockNumber,
    output_db: &PathBuf,
    should_run: bool,
) -> Result<()> {
    let (output_db, tip_block_number) = setup::<DB>(from, to, output_db, db_tool)?;

    output_db.update(|tx| {
        tx.import_table_with_range::<tables::Headers, _>(&db_tool.db.tx()?, Some(from), to)
    })??;

    output_db.update(|tx| {
        tx.import_table_with_range::<tables::AccountChangeSet, _>(&db_tool.db.tx()?, Some(from), to)
    })??;

    unwind_and_copy::<DB>(db_tool, (from, to), tip_block_number, &output_db).await?;

    if should_run {
        dry_run(output_db, to, from).await?;
    }

    Ok(())
}

/// Dry-run an unwind to FROM block and copy the necessary table data to the new database.
async fn unwind_and_copy<DB: Database>(
    db_tool: &mut DbTool<'_, DB>,
    range: (u64, u64),
    tip_block_number: u64,
    output_db: &reth_db::mdbx::Env<reth_db::mdbx::WriteMap>,
) -> eyre::Result<()> {
    let (from, to) = range;
    let mut unwind_tx = Transaction::new(db_tool.db)?;
    let unwind = UnwindInput { unwind_to: from, stage_progress: tip_block_number, bad_block: None };
    let execute_input = reth_stages::ExecInput {
        previous_stage: Some((StageId("Another"), to)),
        stage_progress: Some(from),
    };

    // Unwind hashes all the way to FROM
    StorageHashingStage::default().unwind(&mut unwind_tx, unwind).await.unwrap();
    AccountHashingStage::default().unwind(&mut unwind_tx, unwind).await.unwrap();

    MerkleStage::default_unwind().unwind(&mut unwind_tx, unwind).await?;

    // Bring Plainstate to TO (hashing stage execution requires it)
    let mut exec_stage =
        ExecutionStage::new(reth_revm::Factory::new(Arc::new(MAINNET.clone())), u64::MAX);

    exec_stage
        .unwind(
            &mut unwind_tx,
            UnwindInput { unwind_to: to, stage_progress: tip_block_number, bad_block: None },
        )
        .await?;

    // Bring hashes to TO
    AccountHashingStage { clean_threshold: u64::MAX, commit_threshold: u64::MAX }
        .execute(&mut unwind_tx, execute_input)
        .await
        .unwrap();
    StorageHashingStage { clean_threshold: u64::MAX, commit_threshold: u64::MAX }
        .execute(&mut unwind_tx, execute_input)
        .await
        .unwrap();

    let unwind_inner_tx = unwind_tx.deref_mut();

    // TODO optimize we can actually just get the entries we need
    output_db.update(|tx| tx.import_dupsort::<tables::StorageChangeSet, _>(unwind_inner_tx))??;

    output_db.update(|tx| tx.import_table::<tables::HashedAccount, _>(unwind_inner_tx))??;
    output_db.update(|tx| tx.import_dupsort::<tables::HashedStorage, _>(unwind_inner_tx))??;
    output_db.update(|tx| tx.import_table::<tables::AccountsTrie, _>(unwind_inner_tx))??;
    output_db.update(|tx| tx.import_dupsort::<tables::StoragesTrie, _>(unwind_inner_tx))??;

    unwind_tx.drop()?;

    Ok(())
}

/// Try to re-execute the stage straightaway
async fn dry_run(
    output_db: reth_db::mdbx::Env<reth_db::mdbx::WriteMap>,
    to: u64,
    from: u64,
) -> eyre::Result<()> {
    info!(target: "reth::cli", "Executing stage.");

    let mut tx = Transaction::new(&output_db)?;
    let mut exec_output = false;
    while !exec_output {
        exec_output = MerkleStage::Execution {
            clean_threshold: u64::MAX, /* Forces updating the root instead of calculating from
                                        * scratch */
        }
        .execute(
            &mut tx,
            reth_stages::ExecInput {
                previous_stage: Some((StageId("Another"), to)),
                stage_progress: Some(from),
            },
        )
        .await?
        .done;
    }

    tx.drop()?;

    info!(target: "reth::cli", "Success.");

    Ok(())
}
