use super::setup;
use crate::utils::DbTool;
use eyre::Result;
use reth_db::{database::Database, table::TableImporter, tables};
use reth_primitives::{
    stage::{StageCheckpoint, StageId},
    BlockNumber, MAINNET,
};
use reth_provider::{DatabaseProvider, ShareableDatabase};
use reth_revm::Factory;
use reth_stages::{
    stages::{
        AccountHashingStage, ExecutionStage, ExecutionStageThresholds, MerkleStage,
        StorageHashingStage,
    },
    UnwindInput,
};
use std::{path::PathBuf, sync::Arc};
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
    let mut provider =
        DatabaseProvider::new_rw(db_tool.db.tx_mut()?, std::sync::Arc::new(MAINNET.clone()));

    let unwind = UnwindInput {
        unwind_to: from,
        checkpoint: StageCheckpoint::new(tip_block_number),
        bad_block: None,
    };
    let execute_input = reth_stages::ExecInput {
        previous_stage: Some((StageId::Other("Another"), to)),
        checkpoint: Some(StageCheckpoint::new(from)),
    };

    // Unwind hashes all the way to FROM
    <StorageHashingStage as reth_stages::Stage<DB>>::unwind(
        &mut StorageHashingStage::default(),
        &mut provider,
        unwind,
    )
    .await
    .unwrap();
    <AccountHashingStage as reth_stages::Stage<DB>>::unwind(
        &mut AccountHashingStage::default(),
        &mut provider,
        unwind,
    )
    .await
    .unwrap();

    <MerkleStage as reth_stages::Stage<DB>>::unwind(
        &mut MerkleStage::default_unwind(),
        &mut provider,
        unwind,
    )
    .await?;

    // Bring Plainstate to TO (hashing stage execution requires it)
    let mut exec_stage = ExecutionStage::new(
        reth_revm::Factory::new(Arc::new(MAINNET.clone())),
        ExecutionStageThresholds {
            max_blocks: Some(u64::MAX),
            max_changes: None,
            max_changesets: None,
        },
    );

    <ExecutionStage<Factory> as reth_stages::Stage<DB>>::unwind(
        &mut exec_stage,
        &mut provider,
        UnwindInput {
            unwind_to: to,
            checkpoint: StageCheckpoint::new(tip_block_number),
            bad_block: None,
        },
    )
    .await?;

    // Bring hashes to TO
    <AccountHashingStage as reth_stages::Stage<DB>>::execute(
        &mut AccountHashingStage { clean_threshold: u64::MAX, commit_threshold: u64::MAX },
        &mut provider,
        execute_input,
    )
    .await
    .unwrap();
    <StorageHashingStage as reth_stages::Stage<DB>>::execute(
        &mut StorageHashingStage { clean_threshold: u64::MAX, commit_threshold: u64::MAX },
        &mut provider,
        execute_input,
    )
    .await
    .unwrap();

    let unwind_inner_tx = provider.into_tx();

    // TODO optimize we can actually just get the entries we need
    output_db.update(|tx| tx.import_dupsort::<tables::StorageChangeSet, _>(&unwind_inner_tx))??;

    output_db.update(|tx| tx.import_table::<tables::HashedAccount, _>(&unwind_inner_tx))??;
    output_db.update(|tx| tx.import_dupsort::<tables::HashedStorage, _>(&unwind_inner_tx))??;
    output_db.update(|tx| tx.import_table::<tables::AccountsTrie, _>(&unwind_inner_tx))??;
    output_db.update(|tx| tx.import_dupsort::<tables::StoragesTrie, _>(&unwind_inner_tx))??;

    drop(unwind_inner_tx);

    Ok(())
}

/// Try to re-execute the stage straightaway
async fn dry_run<DB: Database>(output_db: DB, to: u64, from: u64) -> eyre::Result<()> {
    info!(target: "reth::cli", "Executing stage.");
    let shareable_db = ShareableDatabase::new(output_db, std::sync::Arc::new(MAINNET.clone()));
    let mut provider = shareable_db.provider_rw()?;
    let mut exec_output = false;
    while !exec_output {
        exec_output = <MerkleStage as reth_stages::Stage<DB>>::execute(
            &mut MerkleStage::Execution {
                clean_threshold: u64::MAX, /* Forces updating the root instead of calculating
                                            * from
                                            * scratch */
            },
            &mut provider,
            reth_stages::ExecInput {
                previous_stage: Some((StageId::Other("Another"), to)),
                checkpoint: Some(StageCheckpoint::new(from)),
            },
        )
        .await?
        .done;
    }

    drop(provider.into_tx());

    info!(target: "reth::cli", "Success.");

    Ok(())
}
