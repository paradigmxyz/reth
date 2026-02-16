//! Background indexer that builds history indices while the node is live syncing.
//!
//! When `deferred_history_indexing` is enabled, the pipeline skips
//! `IndexAccountHistory`, `IndexStorageHistory`, and `TransactionLookup` stages.
//! This background indexer runs those stages incrementally in a separate thread
//! so the node can follow the chain tip without waiting for index generation.
//!
//! Historical RPC queries (`eth_getBalance` at old blocks, `debug_traceTransaction`,
//! etc.) become available progressively as the indexer catches up.

use reth_config::config::EtlConfig;
use reth_provider::{
    providers::ProviderNodeTypes, DBProvider, DatabaseProviderFactory, ProviderFactory,
    StageCheckpointReader, StageCheckpointWriter,
};
use reth_prune::PruneModes;
use reth_stages::{
    stages::{IndexAccountHistoryStage, IndexStorageHistoryStage, TransactionLookupStage},
    ExecInput, Stage, StageId,
};
use reth_tasks::spawn_os_thread;
use std::{thread::JoinHandle, time::Duration};
use tracing::{debug, error, info, warn};

/// Maximum number of blocks to index per batch.
const DEFAULT_BATCH_SIZE: u64 = 10_000;

/// Sleep duration between batches to avoid starving the persistence service of
/// MDBX write access.
const INTER_BATCH_SLEEP: Duration = Duration::from_millis(500);

/// Sleep duration when the indexer is caught up to tip and waiting for new blocks.
const CAUGHT_UP_SLEEP: Duration = Duration::from_secs(30);

/// Spawns the background indexer on a dedicated OS thread.
///
/// Returns a join handle for clean shutdown.
pub fn spawn_background_indexer<N>(
    provider_factory: ProviderFactory<N>,
    prune_modes: PruneModes,
    etl_config: EtlConfig,
) -> JoinHandle<()>
where
    N: ProviderNodeTypes,
{
    info!(target: "reth::background_indexer", "Starting background history indexer");

    spawn_os_thread("background-indexer", move || {
        if let Err(err) = run_background_indexer(provider_factory, prune_modes, etl_config) {
            error!(target: "reth::background_indexer", %err, "Background indexer failed");
        }
    })
}

/// Main loop for the background indexer.
fn run_background_indexer<N>(
    provider_factory: ProviderFactory<N>,
    prune_modes: PruneModes,
    etl_config: EtlConfig,
) -> eyre::Result<()>
where
    N: ProviderNodeTypes,
{
    let mut tx_lookup = TransactionLookupStage::new(
        Default::default(),
        etl_config.clone(),
        prune_modes.transaction_lookup,
    );
    let mut index_storage = IndexStorageHistoryStage::new(
        Default::default(),
        etl_config.clone(),
        prune_modes.storage_history,
    );
    let mut index_account =
        IndexAccountHistoryStage::new(Default::default(), etl_config, prune_modes.account_history);

    loop {
        let mut did_work = false;

        // Get the current pipeline tip (Finish stage checkpoint) to know how far
        // we can index. Use a read-only txn to minimize lock time.
        let tip = {
            let provider_ro = provider_factory.database_provider_ro()?;
            provider_ro.get_stage_checkpoint(StageId::Finish)?.map(|c| c.block_number).unwrap_or(0)
        };

        if tip == 0 {
            std::thread::sleep(CAUGHT_UP_SLEEP);
            continue;
        }

        // Run each deferred stage with its own write transaction to keep lock
        // duration small.
        did_work |=
            run_stage_batch(&provider_factory, &mut tx_lookup, StageId::TransactionLookup, tip)?;

        std::thread::sleep(INTER_BATCH_SLEEP);

        did_work |= run_stage_batch(
            &provider_factory,
            &mut index_storage,
            StageId::IndexStorageHistory,
            tip,
        )?;

        std::thread::sleep(INTER_BATCH_SLEEP);

        did_work |= run_stage_batch(
            &provider_factory,
            &mut index_account,
            StageId::IndexAccountHistory,
            tip,
        )?;

        if did_work {
            std::thread::sleep(INTER_BATCH_SLEEP);
        } else {
            debug!(target: "reth::background_indexer", %tip, "Background indexer caught up, sleeping");
            std::thread::sleep(CAUGHT_UP_SLEEP);
        }
    }
}

/// Runs a single batch of a stage, returning `true` if work was done.
fn run_stage_batch<N, S>(
    provider_factory: &ProviderFactory<N>,
    stage: &mut S,
    stage_id: StageId,
    tip: u64,
) -> eyre::Result<bool>
where
    N: ProviderNodeTypes,
    S: Stage<<ProviderFactory<N> as DatabaseProviderFactory>::ProviderRW>,
{
    let provider_rw = provider_factory.database_provider_rw()?;

    let checkpoint = provider_rw.get_stage_checkpoint(stage_id)?.unwrap_or_default();

    if checkpoint.block_number >= tip {
        provider_rw.commit()?;
        return Ok(false);
    }

    let batch_target =
        std::cmp::min(tip, checkpoint.block_number.saturating_add(DEFAULT_BATCH_SIZE));

    let input = ExecInput { target: Some(batch_target), checkpoint: Some(checkpoint) };

    debug!(
        target: "reth::background_indexer",
        %stage_id,
        from = checkpoint.block_number,
        to = batch_target,
        %tip,
        "Running background indexing batch"
    );

    match stage.execute(&provider_rw, input) {
        Ok(output) => {
            provider_rw.save_stage_checkpoint(stage_id, output.checkpoint)?;
            provider_rw.commit()?;

            info!(
                target: "reth::background_indexer",
                %stage_id,
                checkpoint = output.checkpoint.block_number,
                %tip,
                "Background indexing batch complete"
            );

            Ok(true)
        }
        Err(err) => {
            warn!(
                target: "reth::background_indexer",
                %stage_id,
                %err,
                "Background indexing batch failed, will retry"
            );
            drop(provider_rw);
            std::thread::sleep(Duration::from_secs(5));
            Ok(false)
        }
    }
}
