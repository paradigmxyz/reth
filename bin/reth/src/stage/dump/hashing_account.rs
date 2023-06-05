use super::setup;
use crate::utils::DbTool;
use eyre::Result;
use reth_db::{database::Database, table::TableImporter, tables};
use reth_primitives::{
    stage::{StageCheckpoint, StageId},
    BlockNumber, MAINNET,
};
use reth_provider::{DatabaseProvider, ShareableDatabase};
use reth_stages::{stages::AccountHashingStage, UnwindInput};
use std::path::PathBuf;
use tracing::info;

pub(crate) async fn dump_hashing_account_stage<DB: Database>(
    db_tool: &mut DbTool<'_, DB>,
    from: BlockNumber,
    to: BlockNumber,
    output_db: &PathBuf,
    should_run: bool,
) -> Result<()> {
    let (output_db, tip_block_number) = setup::<DB>(from, to, output_db, db_tool)?;

    // Import relevant AccountChangeSets
    output_db.update(|tx| {
        tx.import_table_with_range::<tables::AccountChangeSet, _>(&db_tool.db.tx()?, Some(from), to)
    })??;

    unwind_and_copy::<DB>(db_tool, from, tip_block_number, &output_db).await?;

    if should_run {
        dry_run(output_db, to, from).await?;
    }

    Ok(())
}

/// Dry-run an unwind to FROM block and copy the necessary table data to the new database.
async fn unwind_and_copy<DB: Database>(
    db_tool: &mut DbTool<'_, DB>,
    from: u64,
    tip_block_number: u64,
    output_db: &reth_db::mdbx::Env<reth_db::mdbx::WriteMap>,
) -> eyre::Result<()> {
    let mut provider =
        DatabaseProvider::new_rw(db_tool.db.tx_mut()?, std::sync::Arc::new(MAINNET.clone()));
    let mut exec_stage = AccountHashingStage::default();

    <AccountHashingStage as reth_stages::Stage<DB>>::unwind(
        &mut exec_stage,
        &mut provider,
        UnwindInput {
            unwind_to: from,
            checkpoint: StageCheckpoint::new(tip_block_number),
            bad_block: None,
        },
    )
    .await?;
    let unwind_inner_tx = provider.into_tx();

    output_db.update(|tx| tx.import_table::<tables::PlainAccountState, _>(&unwind_inner_tx))??;

    drop(unwind_inner_tx);

    Ok(())
}

/// Try to re-execute the stage straightaway
async fn dry_run<DB: Database>(output_db: DB, to: u64, from: u64) -> eyre::Result<()> {
    info!(target: "reth::cli", "Executing stage.");

    let shareable_db = ShareableDatabase::new(output_db, std::sync::Arc::new(MAINNET.clone()));
    let mut provider = shareable_db.provider_rw()?;
    let mut exec_stage = AccountHashingStage {
        clean_threshold: 1, // Forces hashing from scratch
        ..Default::default()
    };

    let mut exec_output = false;
    while !exec_output {
        exec_output = <AccountHashingStage as reth_stages::Stage<DB>>::execute(
            &mut exec_stage,
            &mut provider,
            reth_stages::ExecInput {
                previous_stage: Some((StageId::Other("Another"), StageCheckpoint::new(to))),
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
