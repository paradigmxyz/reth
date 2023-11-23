use crate::utils::DbTool;
use eyre::Result;
use reth_db::{database::Database, table::TableImporter, tables};
use reth_primitives::{stage::StageCheckpoint, BlockNumber, PruneModes};
use reth_provider::ProviderFactory;
use reth_stages::{
    stages::{
        AccountHashingStage, ExecutionStage, ExecutionStageThresholds, MerkleStage,
        StorageHashingStage, MERKLE_STAGE_DEFAULT_CLEAN_THRESHOLD,
    },
    Stage, UnwindInput,
};
use std::ops::RangeInclusive;
use tracing::info;

pub(crate) async fn dump_merkle_stage<DB: Database>(
    src: &DbTool<'_, DB>,
    dest: &ProviderFactory<DB>,
    block_range: RangeInclusive<BlockNumber>,
    tip: BlockNumber,
    should_run: bool,
) -> Result<()> {
    dest.as_db().update(|tx| {
        tx.import_table_with_range::<tables::Headers, _>(
            &src.db.tx()?,
            Some(*block_range.start()),
            *block_range.end(),
        )
    })??;

    dest.as_db().update(|tx| {
        tx.import_table_with_range::<tables::AccountChangeSet, _>(
            &src.db.tx()?,
            Some(*block_range.start()),
            *block_range.end(),
        )
    })??;

    unwind_and_copy(src, dest, block_range.clone(), tip).await?;

    if should_run {
        dry_run(dest, block_range).await?;
    }

    Ok(())
}

/// Dry-run an unwind to FROM block and copy the necessary table data to the new database.
async fn unwind_and_copy<DB: Database>(
    src: &DbTool<'_, DB>,
    dest: &ProviderFactory<DB>,
    block_range: RangeInclusive<BlockNumber>,
    tip: BlockNumber,
) -> eyre::Result<()> {
    let factory = src.provider_factory();
    let provider = factory.provider_rw()?;

    let unwind = UnwindInput {
        unwind_to: *block_range.start(),
        checkpoint: StageCheckpoint::new(tip),
        bad_block: None,
    };
    let execute_input = reth_stages::ExecInput {
        checkpoint: Some(StageCheckpoint::new(*block_range.start())),
        target: Some(*block_range.end()),
    };

    // Unwind hashes all the way to FROM

    StorageHashingStage::default().unwind(&provider, unwind).unwrap();
    AccountHashingStage::default().unwind(&provider, unwind).unwrap();

    MerkleStage::default_unwind().unwind(&provider, unwind)?;

    // Bring Plainstate to TO (hashing stage execution requires it)
    let mut exec_stage = ExecutionStage::new(
        reth_revm::EvmProcessorFactory::new(src.chain.clone()),
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
            unwind_to: *block_range.end(),
            checkpoint: StageCheckpoint::new(tip),
            bad_block: None,
        },
    )?;

    // Bring hashes to TO
    AccountHashingStage { clean_threshold: u64::MAX, commit_threshold: u64::MAX }
        .execute(&provider, execute_input)
        .unwrap();
    StorageHashingStage { clean_threshold: u64::MAX, commit_threshold: u64::MAX }
        .execute(&provider, execute_input)
        .unwrap();

    let unwind_inner_tx = provider.into_tx();

    // TODO optimize we can actually just get the entries we need
    dest.as_db()
        .update(|tx| tx.import_dupsort::<tables::StorageChangeSet, _>(&unwind_inner_tx))??;

    dest.as_db().update(|tx| tx.import_table::<tables::HashedAccount, _>(&unwind_inner_tx))??;
    dest.as_db().update(|tx| tx.import_dupsort::<tables::HashedStorage, _>(&unwind_inner_tx))??;
    dest.as_db().update(|tx| tx.import_table::<tables::AccountsTrie, _>(&unwind_inner_tx))??;
    dest.as_db().update(|tx| tx.import_dupsort::<tables::StoragesTrie, _>(&unwind_inner_tx))??;

    Ok(())
}

/// Try to re-execute the stage straightaway
async fn dry_run<DB: Database>(
    dest: &ProviderFactory<DB>,
    block_range: RangeInclusive<BlockNumber>,
) -> eyre::Result<()> {
    info!(target: "reth::cli", "Executing stage.");
    let provider = dest.provider_rw()?;

    let mut stage = MerkleStage::Execution {
        // Forces updating the root instead of calculating from scratch
        clean_threshold: u64::MAX,
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

    info!(target: "reth::cli", "Success");

    Ok(())
}
