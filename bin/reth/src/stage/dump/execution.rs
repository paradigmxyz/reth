use crate::utils::DbTool;
use eyre::Result;
use reth_db::{
    cursor::DbCursorRO, database::Database, table::TableImporter, tables, transaction::DbTx,
};
use reth_primitives::{stage::StageCheckpoint, BlockNumber, ChainSpec};
use reth_provider::ProviderFactory;
use reth_revm::EvmProcessorFactory;
use reth_stages::{stages::ExecutionStage, ExecInput, Stage, UnwindInput};
use std::{ops::RangeInclusive, sync::Arc};
use tracing::info;

pub(crate) async fn dump_execution_stage<DB: Database>(
    src: &DbTool<'_, DB>,
    dest: &ProviderFactory<DB>,
    block_range: RangeInclusive<BlockNumber>,
    tip: BlockNumber,
    should_run: bool,
) -> Result<()> {
    import_tables_with_range(src, dest, block_range.clone())?;

    unwind_and_copy(src, dest, *block_range.start(), tip).await?;

    if should_run {
        dry_run(dest, src.chain.clone(), block_range).await?;
    }

    Ok(())
}

/// Imports all the tables that can be copied over a range.
fn import_tables_with_range<DB: Database>(
    src: &DbTool<'_, DB>,
    dest: &ProviderFactory<DB>,
    block_range: RangeInclusive<BlockNumber>,
) -> eyre::Result<()> {
    //  We're not sharing the transaction in case the memory grows too much.
    let from = *block_range.start();
    let to = *block_range.end();

    dest.as_db().update(|tx| {
        tx.import_table_with_range::<tables::CanonicalHeaders, _>(&src.db.tx()?, Some(from), to)
    })??;
    dest.as_db().update(|tx| {
        tx.import_table_with_range::<tables::HeaderTD, _>(&src.db.tx()?, Some(from), to)
    })??;
    dest.as_db().update(|tx| {
        tx.import_table_with_range::<tables::Headers, _>(&src.db.tx()?, Some(from), to)
    })??;
    dest.as_db().update(|tx| {
        tx.import_table_with_range::<tables::BlockBodyIndices, _>(&src.db.tx()?, Some(from), to)
    })??;
    dest.as_db().update(|tx| {
        tx.import_table_with_range::<tables::BlockOmmers, _>(&src.db.tx()?, Some(from), to)
    })??;

    // Find range of transactions that need to be copied over
    let (from_tx, to_tx) = src.db.view(|read_tx| {
        let mut read_cursor = read_tx.cursor_read::<tables::BlockBodyIndices>()?;
        let (_, from_block) =
            read_cursor.seek(from)?.ok_or(eyre::eyre!("BlockBody {from} does not exist."))?;
        let (_, to_block) =
            read_cursor.seek(to)?.ok_or(eyre::eyre!("BlockBody {to} does not exist."))?;

        Ok::<(u64, u64), eyre::ErrReport>((
            from_block.first_tx_num,
            to_block.first_tx_num + to_block.tx_count,
        ))
    })??;

    // TODO:
    dest.as_db().update(|tx| {
        tx.import_table_with_range::<tables::Transactions, _>(&src.db.tx()?, Some(from_tx), to_tx)
    })??;

    dest.as_db().update(|tx| {
        tx.import_table_with_range::<tables::TxSenders, _>(&src.db.tx()?, Some(from_tx), to_tx)
    })??;

    Ok(())
}

/// Dry-run an unwind to FROM block, so we can get the PlainStorageState and
/// PlainAccountState safely. There might be some state dependency from an address
/// which hasn't been changed in the given range.
async fn unwind_and_copy<DB: Database>(
    src: &DbTool<'_, DB>,
    dest: &ProviderFactory<DB>,
    unwind_to: BlockNumber,
    tip: BlockNumber,
) -> eyre::Result<()> {
    let factory = src.provider_factory();
    let provider = factory.provider_rw()?;

    let mut exec_stage =
        ExecutionStage::new_with_factory(EvmProcessorFactory::new(src.chain.clone()));

    exec_stage.unwind(
        &provider,
        UnwindInput { unwind_to, checkpoint: StageCheckpoint::new(tip), bad_block: None },
    )?;

    let unwind_inner_tx = provider.into_tx();

    dest.as_db()
        .update(|tx| tx.import_dupsort::<tables::PlainStorageState, _>(&unwind_inner_tx))??;
    dest.as_db()
        .update(|tx| tx.import_table::<tables::PlainAccountState, _>(&unwind_inner_tx))??;
    dest.as_db().update(|tx| tx.import_table::<tables::Bytecodes, _>(&unwind_inner_tx))??;

    Ok(())
}

/// Try to re-execute the stage without committing
async fn dry_run<DB: Database>(
    dest: &ProviderFactory<DB>,
    chain: Arc<ChainSpec>,
    block_range: RangeInclusive<BlockNumber>,
) -> eyre::Result<()> {
    info!(target: "reth::cli", "Executing stage. [dry-run]");

    let mut exec_stage = ExecutionStage::new_with_factory(EvmProcessorFactory::new(chain.clone()));

    let input = ExecInput {
        checkpoint: Some(StageCheckpoint::new(*block_range.start())),
        target: Some(*block_range.end()),
    };
    exec_stage.execute(&dest.provider_rw()?, input)?;

    info!(target: "reth::cli", "Success");

    Ok(())
}
