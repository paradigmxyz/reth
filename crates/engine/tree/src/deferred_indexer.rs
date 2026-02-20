//! Deferred history indexer implementation using pipeline stages.
//!
//! This implements
//! [`DeferredHistoryIndexer`](crate::persistence::DeferredHistoryIndexer) by running the
//! history indexing stages (`TransactionLookup`, `IndexStorageHistory`, `IndexAccountHistory`)
//! incrementally inside the persistence service's thread. This eliminates MDBX write-lock
//! contention that would occur if these stages ran on a separate thread.

use reth_config::config::StageConfig;
use reth_provider::{
    providers::ProviderNodeTypes, DBProvider, DatabaseProviderFactory, ProviderFactory,
    StageCheckpointReader, StageCheckpointWriter,
};
use reth_prune::PruneModes;
use reth_stages::{
    stages::{IndexAccountHistoryStage, IndexStorageHistoryStage, TransactionLookupStage},
    ExecInput, Stage, StageId,
};
use tracing::{debug, info, warn};

use crate::persistence::DeferredHistoryIndexer;

/// Maximum number of blocks to index per tick.
const DEFAULT_BATCH_SIZE: u64 = 10_000;

/// Deferred history indexer that runs pipeline stages inside the persistence service.
///
/// Processes history indexing in small batches, round-robining between the three deferred
/// stages (`TransactionLookup`, `IndexStorageHistory`, `IndexAccountHistory`).
pub struct StageDeferredHistoryIndexer<N: ProviderNodeTypes> {
    provider_factory: ProviderFactory<N>,
    tx_lookup: TransactionLookupStage,
    index_storage: IndexStorageHistoryStage,
    index_account: IndexAccountHistoryStage,
    batch_size: u64,
    caught_up: bool,
    /// Round-robin index: 0 = tx_lookup, 1 = index_storage, 2 = index_account
    next_stage: usize,
}

impl<N: ProviderNodeTypes> std::fmt::Debug for StageDeferredHistoryIndexer<N> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StageDeferredHistoryIndexer")
            .field("batch_size", &self.batch_size)
            .field("caught_up", &self.caught_up)
            .field("next_stage", &self.next_stage)
            .finish()
    }
}

impl<N: ProviderNodeTypes> StageDeferredHistoryIndexer<N> {
    /// Creates a new deferred history indexer.
    pub fn new(
        provider_factory: ProviderFactory<N>,
        stages_config: &StageConfig,
        prune_modes: &PruneModes,
    ) -> Self {
        let etl = stages_config.etl.clone();

        Self {
            provider_factory,
            tx_lookup: TransactionLookupStage::new(
                stages_config.transaction_lookup,
                etl.clone(),
                prune_modes.transaction_lookup,
            ),
            index_storage: IndexStorageHistoryStage::new(
                stages_config.index_storage_history,
                etl.clone(),
                prune_modes.storage_history,
            ),
            index_account: IndexAccountHistoryStage::new(
                stages_config.index_account_history,
                etl,
                prune_modes.account_history,
            ),
            batch_size: DEFAULT_BATCH_SIZE,
            caught_up: false,
            next_stage: 0,
        }
    }

    /// Runs one stage batch and returns whether work was done.
    fn run_stage_batch(
        &mut self,
        stage_idx: usize,
        tip: u64,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>>
    where
        TransactionLookupStage: Stage<<ProviderFactory<N> as DatabaseProviderFactory>::ProviderRW>,
        IndexStorageHistoryStage:
            Stage<<ProviderFactory<N> as DatabaseProviderFactory>::ProviderRW>,
        IndexAccountHistoryStage:
            Stage<<ProviderFactory<N> as DatabaseProviderFactory>::ProviderRW>,
    {
        let stage_id = match stage_idx {
            0 => StageId::TransactionLookup,
            1 => StageId::IndexStorageHistory,
            2 => StageId::IndexAccountHistory,
            _ => unreachable!(),
        };

        let provider_rw = self.provider_factory.database_provider_rw()?;
        let checkpoint = provider_rw.get_stage_checkpoint(stage_id)?.unwrap_or_default();

        if checkpoint.block_number >= tip {
            provider_rw.commit()?;
            return Ok(false);
        }

        let batch_target =
            std::cmp::min(tip, checkpoint.block_number.saturating_add(self.batch_size));
        let input = ExecInput { target: Some(batch_target), checkpoint: Some(checkpoint) };

        debug!(
            target: "engine::persistence::deferred_indexer",
            %stage_id,
            from = checkpoint.block_number,
            to = batch_target,
            %tip,
            "Running deferred indexing batch"
        );

        let result = match stage_idx {
            0 => self.tx_lookup.execute(&provider_rw, input),
            1 => self.index_storage.execute(&provider_rw, input),
            2 => self.index_account.execute(&provider_rw, input),
            _ => unreachable!(),
        };

        match result {
            Ok(output) => {
                provider_rw.save_stage_checkpoint(stage_id, output.checkpoint)?;
                provider_rw.commit()?;
                info!(
                    target: "engine::persistence::deferred_indexer",
                    %stage_id,
                    checkpoint = output.checkpoint.block_number,
                    %tip,
                    "Deferred indexing batch complete"
                );
                Ok(true)
            }
            Err(err) => {
                warn!(
                    target: "engine::persistence::deferred_indexer",
                    %stage_id,
                    %err,
                    "Deferred indexing batch failed, will retry"
                );
                drop(provider_rw);
                Ok(false)
            }
        }
    }

    /// Check if all three stages have caught up to the pipeline tip.
    fn check_caught_up(&self) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        let provider_ro = self.provider_factory.database_provider_ro()?;
        let tip =
            provider_ro.get_stage_checkpoint(StageId::Finish)?.map(|c| c.block_number).unwrap_or(0);

        if tip == 0 {
            return Ok(false);
        }

        let tx_lookup = provider_ro
            .get_stage_checkpoint(StageId::TransactionLookup)?
            .map(|c| c.block_number)
            .unwrap_or(0);
        let storage = provider_ro
            .get_stage_checkpoint(StageId::IndexStorageHistory)?
            .map(|c| c.block_number)
            .unwrap_or(0);
        let account = provider_ro
            .get_stage_checkpoint(StageId::IndexAccountHistory)?
            .map(|c| c.block_number)
            .unwrap_or(0);

        Ok(tx_lookup >= tip && storage >= tip && account >= tip)
    }
}

impl<N> DeferredHistoryIndexer for StageDeferredHistoryIndexer<N>
where
    N: ProviderNodeTypes,
    TransactionLookupStage: Stage<<ProviderFactory<N> as DatabaseProviderFactory>::ProviderRW>,
    IndexStorageHistoryStage: Stage<<ProviderFactory<N> as DatabaseProviderFactory>::ProviderRW>,
    IndexAccountHistoryStage: Stage<<ProviderFactory<N> as DatabaseProviderFactory>::ProviderRW>,
{
    fn tick(&mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        // Read the current pipeline tip
        let tip = {
            let provider_ro = self.provider_factory.database_provider_ro()?;
            provider_ro.get_stage_checkpoint(StageId::Finish)?.map(|c| c.block_number).unwrap_or(0)
        };

        if tip == 0 {
            return Ok(());
        }

        // Run one batch for the current stage
        let current = self.next_stage;
        let did_work = self.run_stage_batch(current, tip)?;

        // Advance to next stage (round-robin)
        self.next_stage = (self.next_stage + 1) % 3;

        // Check if all stages are caught up
        if (!did_work || self.check_caught_up()?) &&
            !self.caught_up &&
            self.check_caught_up().unwrap_or(false)
        {
            info!(target: "engine::persistence::deferred_indexer", "Deferred history indexing caught up to tip");
            self.caught_up = true;
        }

        Ok(())
    }

    fn is_caught_up(&self) -> bool {
        self.caught_up
    }

    fn on_reorg(&mut self, _new_tip_num: u64) {
        self.caught_up = false;
    }
}
