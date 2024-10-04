use std::sync::Arc;

use super::setup;
use eyre::Result;
use reth_db::{tables, DatabaseEnv};
use reth_db_api::{database::Database, table::TableImporter};
use reth_db_common::DbTool;
use reth_node_builder::NodeTypesWithDBAdapter;
use reth_node_core::dirs::{ChainPath, DataDirPath};
use reth_provider::{
    providers::{ProviderNodeTypes, StaticFileProvider},
    DatabaseProviderFactory, ProviderFactory,
};
use reth_stages::{stages::StorageHashingStage, Stage, StageCheckpoint, UnwindInput};
use tracing::info;

pub(crate) async fn dump_hashing_storage_stage<N: ProviderNodeTypes>(
    db_tool: &DbTool<N>,
    from: u64,
    to: u64,
    output_datadir: ChainPath<DataDirPath>,
    should_run: bool,
) -> Result<()> {
    let (output_db, tip_block_number) = setup(from, to, &output_datadir.db(), db_tool)?;

    unwind_and_copy(db_tool, from, tip_block_number, &output_db)?;

    if should_run {
        dry_run(
            ProviderFactory::<NodeTypesWithDBAdapter<N, Arc<DatabaseEnv>>>::new(
                Arc::new(output_db),
                db_tool.chain(),
                StaticFileProvider::read_write(output_datadir.static_files())?,
            ),
            to,
            from,
        )?;
    }

    Ok(())
}

/// Dry-run an unwind to FROM block and copy the necessary table data to the new database.
fn unwind_and_copy<N: ProviderNodeTypes>(
    db_tool: &DbTool<N>,
    from: u64,
    tip_block_number: u64,
    output_db: &DatabaseEnv,
) -> eyre::Result<()> {
    let provider = db_tool.provider_factory.database_provider_rw()?;

    let mut exec_stage = StorageHashingStage::default();

    exec_stage.unwind(
        &provider,
        UnwindInput {
            unwind_to: from,
            checkpoint: StageCheckpoint::new(tip_block_number),
            bad_block: None,
        },
    )?;
    let unwind_inner_tx = provider.into_tx();

    // TODO optimize we can actually just get the entries we need for both these tables
    output_db
        .update(|tx| tx.import_dupsort::<tables::PlainStorageState, _>(&unwind_inner_tx))??;
    output_db
        .update(|tx| tx.import_dupsort::<tables::StorageChangeSets, _>(&unwind_inner_tx))??;

    Ok(())
}

/// Try to re-execute the stage straight away
fn dry_run<N: ProviderNodeTypes>(
    output_provider_factory: ProviderFactory<N>,
    to: u64,
    from: u64,
) -> eyre::Result<()> {
    info!(target: "reth::cli", "Executing stage.");

    let provider = output_provider_factory.database_provider_rw()?;
    let mut stage = StorageHashingStage {
        clean_threshold: 1, // Forces hashing from scratch
        ..Default::default()
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

    info!(target: "reth::cli", "Success.");

    Ok(())
}
