use super::setup;
use crate::utils::DbTool;
use eyre::Result;
use reth_config::config::EtlConfig;
use reth_db::{database::Database, table::TableImporter, tables, DatabaseEnv};
use reth_node_core::dirs::{ChainPath, DataDirPath};
use reth_node_ethereum::EthEvmConfig;
use reth_primitives::{stage::StageCheckpoint, BlockNumber, PruneModes};
use reth_provider::ProviderFactory;
use reth_stages::{
    stages::{
        AccountHashingStage, ExecutionStage, ExecutionStageThresholds, MerkleStage,
        StorageHashingStage, MERKLE_STAGE_DEFAULT_CLEAN_THRESHOLD,
    },
    Stage, UnwindInput,
};
use tracing::info;

pub(crate) async fn dump_merkle_stage<DB: Database>(
    db_tool: &DbTool<DB>,
    from: BlockNumber,
    to: BlockNumber,
    output_datadir: ChainPath<DataDirPath>,
    should_run: bool,
) -> Result<()> {
    let (output_db, tip_block_number) = setup(from, to, &output_datadir.db_path(), db_tool)?;

    output_db.update(|tx| {
        tx.import_table_with_range::<tables::Headers, _>(
            &db_tool.provider_factory.db_ref().tx()?,
            Some(from),
            to,
        )
    })??;

    output_db.update(|tx| {
        tx.import_table_with_range::<tables::AccountChangeSets, _>(
            &db_tool.provider_factory.db_ref().tx()?,
            Some(from),
            to,
        )
    })??;

    unwind_and_copy(db_tool, (from, to), tip_block_number, &output_db).await?;

    if should_run {
        dry_run(
            ProviderFactory::new(
                output_db,
                db_tool.chain.clone(),
                output_datadir.static_files_path(),
            )?,
            to,
            from,
        )
        .await?;
    }

    Ok(())
}

/// Dry-run an unwind to FROM block and copy the necessary table data to the new database.
async fn unwind_and_copy<DB: Database>(
    db_tool: &DbTool<DB>,
    range: (u64, u64),
    tip_block_number: u64,
    output_db: &DatabaseEnv,
) -> eyre::Result<()> {
    let (from, to) = range;
    let provider = db_tool.provider_factory.provider_rw()?;

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
        reth_revm::EvmProcessorFactory::new(db_tool.chain.clone(), EthEvmConfig::default()),
        ExecutionStageThresholds {
            max_blocks: Some(u64::MAX),
            max_changes: None,
            max_cumulative_gas: None,
            max_duration: None,
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
    AccountHashingStage {
        clean_threshold: u64::MAX,
        commit_threshold: u64::MAX,
        etl_config: EtlConfig::default(),
    }
    .execute(&provider, execute_input)
    .unwrap();
    StorageHashingStage {
        clean_threshold: u64::MAX,
        commit_threshold: u64::MAX,
        etl_config: EtlConfig::default(),
    }
    .execute(&provider, execute_input)
    .unwrap();

    let unwind_inner_tx = provider.into_tx();

    // TODO optimize we can actually just get the entries we need
    output_db
        .update(|tx| tx.import_dupsort::<tables::StorageChangeSets, _>(&unwind_inner_tx))??;

    output_db.update(|tx| tx.import_table::<tables::HashedAccounts, _>(&unwind_inner_tx))??;
    output_db.update(|tx| tx.import_dupsort::<tables::HashedStorages, _>(&unwind_inner_tx))??;
    output_db.update(|tx| tx.import_table::<tables::AccountsTrie, _>(&unwind_inner_tx))??;
    output_db.update(|tx| tx.import_dupsort::<tables::StoragesTrie, _>(&unwind_inner_tx))??;

    Ok(())
}

/// Try to re-execute the stage straightaway
async fn dry_run<DB: Database>(
    output_provider_factory: ProviderFactory<DB>,
    to: u64,
    from: u64,
) -> eyre::Result<()> {
    info!(target: "reth::cli", "Executing stage.");
    let provider = output_provider_factory.provider_rw()?;

    let mut stage = MerkleStage::Execution {
        // Forces updating the root instead of calculating from scratch
        clean_threshold: u64::MAX,
    };

    loop {
        let input = reth_stages::ExecInput {
            target: Some(to),
            checkpoint: Some(StageCheckpoint::new(from)),
        };
        if stage.execute(&provider, input)?.done {
            break
        }
    }

    info!(target: "reth::cli", "Success");

    Ok(())
}
