use super::setup;
use crate::utils::DbTool;
use eyre::Result;
use reth_db::{database::Database, table::TableImporter, tables, DatabaseEnv};
use reth_primitives::{stage::StageCheckpoint, BlockNumber, ChainSpec, PruneModes};
use reth_provider::{DatabaseProviderRW, ProviderFactory};
use reth_stages::{
    stages::{
        AccountHashingStage, ExecutionStage, ExecutionStageThresholds, MerkleStage,
        StorageHashingStage, MERKLE_STAGE_DEFAULT_CLEAN_THRESHOLD,
    },
    ExecInput, ExecOutput, Stage, StageError, UnwindInput,
};
use std::{future::poll_fn, path::PathBuf, sync::Arc};
use tracing::info;

pub(crate) async fn dump_merkle_stage<DB: Database>(
    db_tool: &DbTool<'_, DB>,
    from: BlockNumber,
    to: BlockNumber,
    output_db: &PathBuf,
    should_run: bool,
) -> Result<()> {
    let (output_db, tip_block_number) = setup(from, to, output_db, db_tool)?;

    output_db.update(|tx| {
        tx.import_table_with_range::<tables::Headers, _>(&db_tool.db.tx()?, Some(from), to)
    })??;

    output_db.update(|tx| {
        tx.import_table_with_range::<tables::AccountChangeSet, _>(&db_tool.db.tx()?, Some(from), to)
    })??;

    unwind_and_copy(db_tool, (from, to), tip_block_number, &output_db).await?;

    if should_run {
        dry_run(db_tool.chain.clone(), output_db, to, from).await?;
    }

    Ok(())
}

/// Dry-run an unwind to FROM block and copy the necessary table data to the new database.
async fn unwind_and_copy<DB: Database>(
    db_tool: &DbTool<'_, DB>,
    range: (u64, u64),
    tip_block_number: u64,
    output_db: &DatabaseEnv,
) -> eyre::Result<()> {
    let (from, to) = range;
    let factory = ProviderFactory::new(db_tool.db, db_tool.chain.clone());
    let provider = factory.provider_rw()?;

    let unwind = UnwindInput {
        unwind_to: from,
        checkpoint: StageCheckpoint::new(tip_block_number),
        bad_block: None,
    };
    let execute_input =
        reth_stages::ExecInput { target: Some(to), checkpoint: Some(StageCheckpoint::new(from)) };

    // Unwind hashes all the way to FROM

    StorageHashingStage::default().unwind(&provider, unwind).unwrap();
    AccountHashingStage::default().unwind(&provider, unwind).unwrap();

    MerkleStage::default_unwind().unwind(&provider, unwind)?;

    // Bring Plainstate to TO (hashing stage execution requires it)
    let mut exec_stage = ExecutionStage::new(
        reth_revm::Factory::new(db_tool.chain.clone()),
        ExecutionStageThresholds {
            max_blocks: Some(u64::MAX),
            max_changes: None,
            max_cumulative_gas: None,
        },
        MERKLE_STAGE_DEFAULT_CLEAN_THRESHOLD,
        PruneModes::all(),
    );

    exec_stage.unwind(
        &provider,
        UnwindInput {
            unwind_to: to,
            checkpoint: StageCheckpoint::new(tip_block_number),
            bad_block: None,
        },
    )?;

    // Bring hashes to TO

    poll_and_execute(
        &mut AccountHashingStage { clean_threshold: u64::MAX, commit_threshold: u64::MAX },
        &provider,
        execute_input,
    )
    .await
    .unwrap();
    poll_and_execute(
        &mut StorageHashingStage { clean_threshold: u64::MAX, commit_threshold: u64::MAX },
        &provider,
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

    Ok(())
}

// todo: move to test_utils in reth_stages and use it where we currently manually poll
async fn poll_and_execute<DB: Database, S: Stage<DB>>(
    stage: &mut S,
    provider: &DatabaseProviderRW<'_, &DB>,
    input: ExecInput,
) -> Result<ExecOutput, StageError> {
    poll_fn(|cx| stage.poll_ready(cx, input)).await.and_then(|_| stage.execute(&provider, input))
}

/// Try to re-execute the stage straightaway
async fn dry_run<DB: Database>(
    chain: Arc<ChainSpec>,
    output_db: DB,
    to: u64,
    from: u64,
) -> eyre::Result<()> {
    info!(target: "reth::cli", "Executing stage.");
    let factory = ProviderFactory::new(&output_db, chain);
    let provider = factory.provider_rw()?;
    let mut exec_output = false;
    while !exec_output {
        exec_output = poll_and_execute(
            &mut MerkleStage::Execution {
                // Forces updating the root instead of calculating from scratch
                clean_threshold: u64::MAX,
            },
            &provider,
            reth_stages::ExecInput {
                target: Some(to),
                checkpoint: Some(StageCheckpoint::new(from)),
            },
        )
        .await?
        .done;
    }

    info!(target: "reth::cli", "Success.");

    Ok(())
}
