use crate::utils::DbTool;
use eyre::Result;
use reth_db::{database::Database, table::TableImporter, tables};
use reth_primitives::{stage::StageCheckpoint, BlockNumber};
use reth_provider::ProviderFactory;
use reth_stages::{stages::AccountHashingStage, Stage, UnwindInput};
use std::ops::RangeInclusive;
use tracing::info;

pub(crate) async fn dump_hashing_account_stage<DB: Database>(
    src: &DbTool<'_, DB>,
    dest: &ProviderFactory<DB>,
    block_range: RangeInclusive<BlockNumber>,
    tip: BlockNumber,
    should_run: bool,
) -> Result<()> {
    // Import relevant AccountChangeSets
    dest.as_db().update(|tx| {
        tx.import_table_with_range::<tables::AccountChangeSet, _>(
            &src.db.tx()?,
            Some(*block_range.start()),
            *block_range.end(),
        )
    })??;

    unwind_and_copy(src, dest, *block_range.start(), tip)?;

    if should_run {
        dry_run(dest, block_range).await?;
    }

    Ok(())
}

/// Dry-run an unwind to FROM block and copy the necessary table data to the new database.
fn unwind_and_copy<DB: Database>(
    src: &DbTool<'_, DB>,
    dest: &ProviderFactory<DB>,
    unwind_to: BlockNumber,
    tip: BlockNumber,
) -> eyre::Result<()> {
    let factory = src.provider_factory();
    let provider = factory.provider_rw()?;
    let mut exec_stage = AccountHashingStage::default();

    exec_stage.unwind(
        &provider,
        UnwindInput { unwind_to, checkpoint: StageCheckpoint::new(tip), bad_block: None },
    )?;
    let unwind_inner_tx = provider.into_tx();

    dest.as_db()
        .update(|tx| tx.import_table::<tables::PlainAccountState, _>(&unwind_inner_tx))??;

    Ok(())
}

/// Try to re-execute the stage straightaway
async fn dry_run<DB: Database>(
    dest: &ProviderFactory<DB>,
    block_range: RangeInclusive<BlockNumber>,
) -> eyre::Result<()> {
    info!(target: "reth::cli", "Executing stage.");

    let provider = dest.provider_rw()?;
    let mut stage = AccountHashingStage {
        clean_threshold: 1, // Forces hashing from scratch
        ..Default::default()
    };

    loop {
        let input = reth_stages::ExecInput {
            checkpoint: Some(StageCheckpoint::new(*block_range.start())),
            target: Some(*block_range.end()),
        };
        if stage.execute(&provider, input)?.done {
            break
        }
    }

    info!(target: "reth::cli", "Success.");

    Ok(())
}
