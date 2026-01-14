use std::sync::Arc;

use super::setup;
use alloy_primitives::{Address, BlockNumber};
use eyre::Result;
use reth_config::config::EtlConfig;
use reth_consensus::FullConsensus;
use reth_db::DatabaseEnv;
use reth_db_api::{database::Database, models::BlockNumberAddress, table::TableImporter, tables};
use reth_db_common::DbTool;
use reth_evm::ConfigureEvm;
use reth_exex::ExExManagerHandle;
use reth_node_core::dirs::{ChainPath, DataDirPath};
use reth_provider::{
    providers::{ProviderNodeTypes, RocksDBProvider, StaticFileProvider},
    DatabaseProviderFactory, ProviderFactory,
};
use reth_stages::{
    stages::{
        AccountHashingStage, ExecutionStage, MerkleStage, StorageHashingStage,
        MERKLE_STAGE_DEFAULT_REBUILD_THRESHOLD,
    },
    ExecutionStageThresholds, Stage, StageCheckpoint, UnwindInput,
};
use tracing::info;

pub(crate) async fn dump_merkle_stage<N>(
    db_tool: &DbTool<N>,
    from: BlockNumber,
    to: BlockNumber,
    output_datadir: ChainPath<DataDirPath>,
    should_run: bool,
    evm_config: impl ConfigureEvm<Primitives = N::Primitives>,
    consensus: impl FullConsensus<N::Primitives> + 'static,
) -> Result<()>
where
    N: ProviderNodeTypes<DB = Arc<DatabaseEnv>>,
{
    let (output_db, tip_block_number) = setup(from, to, &output_datadir.db(), db_tool)?;

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

    unwind_and_copy(db_tool, (from, to), tip_block_number, &output_db, evm_config, consensus)?;

    if should_run {
        dry_run(
            ProviderFactory::<N>::new(
                Arc::new(output_db),
                db_tool.chain(),
                StaticFileProvider::read_write(output_datadir.static_files())?,
                RocksDBProvider::builder(output_datadir.rocksdb()).with_default_tables().build()?,
            )?,
            to,
            from,
        )?;
    }

    Ok(())
}

/// Dry-run an unwind to FROM block and copy the necessary table data to the new database.
fn unwind_and_copy<N: ProviderNodeTypes>(
    db_tool: &DbTool<N>,
    range: (u64, u64),
    tip_block_number: u64,
    output_db: &DatabaseEnv,
    evm_config: impl ConfigureEvm<Primitives = N::Primitives>,
    consensus: impl FullConsensus<N::Primitives> + 'static,
) -> eyre::Result<()> {
    let (from, to) = range;
    let provider = db_tool.provider_factory.database_provider_rw()?;

    let unwind = UnwindInput {
        unwind_to: from,
        checkpoint: StageCheckpoint::new(tip_block_number),
        bad_block: None,
    };
    let execute_input =
        reth_stages::ExecInput { target: Some(to), checkpoint: Some(StageCheckpoint::new(from)) };

    // Unwind hashes all the way to FROM
    StorageHashingStage::default().unwind(&provider, unwind)?;
    AccountHashingStage::default().unwind(&provider, unwind)?;
    MerkleStage::default_unwind().unwind(&provider, unwind)?;

    // Bring Plainstate to TO (hashing stage execution requires it)
    let mut exec_stage = ExecutionStage::new(
        evm_config, // Not necessary for unwinding.
        Arc::new(consensus),
        ExecutionStageThresholds {
            max_blocks: Some(u64::MAX),
            max_changes: None,
            max_cumulative_gas: None,
            max_duration: None,
        },
        MERKLE_STAGE_DEFAULT_REBUILD_THRESHOLD,
        ExExManagerHandle::empty(),
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
    .execute(&provider, execute_input)?;
    StorageHashingStage {
        clean_threshold: u64::MAX,
        commit_threshold: u64::MAX,
        etl_config: EtlConfig::default(),
    }
    .execute(&provider, execute_input)?;

    let unwind_inner_tx = provider.into_tx();

    output_db.update(|tx| {
        tx.import_table_with_range::<tables::StorageChangeSets, _>(
            &unwind_inner_tx,
            Some(BlockNumberAddress((from, Address::ZERO))),
            BlockNumberAddress((to, Address::repeat_byte(0xff))),
        )
    })??;

    output_db.update(|tx| tx.import_table::<tables::HashedAccounts, _>(&unwind_inner_tx))??;
    output_db.update(|tx| tx.import_dupsort::<tables::HashedStorages, _>(&unwind_inner_tx))??;
    output_db.update(|tx| tx.import_table::<tables::AccountsTrie, _>(&unwind_inner_tx))??;
    output_db.update(|tx| tx.import_dupsort::<tables::StoragesTrie, _>(&unwind_inner_tx))??;

    Ok(())
}

/// Try to re-execute the stage straight away
fn dry_run<N>(output_provider_factory: ProviderFactory<N>, to: u64, from: u64) -> eyre::Result<()>
where
    N: ProviderNodeTypes,
{
    info!(target: "reth::cli", "Executing stage.");
    let provider = output_provider_factory.database_provider_rw()?;

    let mut stage = MerkleStage::Execution {
        // Forces updating the root instead of calculating from scratch
        rebuild_threshold: u64::MAX,
        incremental_threshold: u64::MAX,
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
