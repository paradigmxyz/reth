//! Deferred history indexer implementation using pipeline stages.
//!
//! Runs history indexing stages (`TransactionLookup`, `IndexStorageHistory`,
//! `IndexAccountHistory`) incrementally inside the persistence service's thread. This eliminates
//! MDBX write-lock contention that would occur if these stages ran on a separate thread.

use reth_config::config::StageConfig;
use reth_provider::{StageCheckpointReader, StageCheckpointWriter};
use reth_prune::{PruneMode, PruneModes, PrunePurpose, PruneSegment};
use reth_stages::{
    stages::{IndexAccountHistoryStage, IndexStorageHistoryStage, TransactionLookupStage},
    ExecInput, Stage, StageCheckpoint, StageId,
};
use tracing::{debug, info, warn};

/// Per-stage batch sizes for deferred indexing ticks.
///
/// These are tuned to keep lock hold times bounded; storage history is typically the slowest
/// segment, account history is medium, and tx lookup is often fastest (especially when fully
/// pruned).
const TX_LOOKUP_BATCH_SIZE: u64 = 10_000;
const INDEX_ACCOUNT_HISTORY_BATCH_SIZE: u64 = 3_000;
const INDEX_STORAGE_HISTORY_BATCH_SIZE: u64 = 1_000;

/// Construction configuration for the deferred history indexer.
#[derive(Debug, Clone)]
pub struct DeferredHistoryIndexerConfig {
    /// Stage-level execution configuration.
    pub stages: StageConfig,
    /// Pruning modes that affect index stage behavior.
    pub prune_modes: PruneModes,
}

impl DeferredHistoryIndexerConfig {
    /// Creates a new deferred history indexer config.
    pub fn new(stages: StageConfig, prune_modes: PruneModes) -> Self {
        Self { stages, prune_modes }
    }
}

/// Progress information for one deferred indexing batch.
#[derive(Debug, Clone, Copy)]
pub struct DeferredStageProgress {
    /// Deferred stage that advanced in this tick.
    pub stage_id: StageId,
    /// Persisted checkpoint after the stage batch completed.
    pub checkpoint: StageCheckpoint,
}

/// Deferred history indexer that runs pipeline stages inside the persistence service.
///
/// Processes history indexing in small batches, round-robining between the three deferred
/// stages (`TransactionLookup`, `IndexStorageHistory`, `IndexAccountHistory`).
pub struct DeferredHistoryIndexer {
    tx_lookup: TransactionLookupStage,
    index_storage: IndexStorageHistoryStage,
    index_account: IndexAccountHistoryStage,
    tx_lookup_prune_mode: Option<PruneMode>,
    index_storage_prune_mode: Option<PruneMode>,
    index_account_prune_mode: Option<PruneMode>,
    tx_lookup_batch_size: u64,
    index_storage_batch_size: u64,
    index_account_batch_size: u64,
    caught_up: bool,
    /// Round-robin index: 0 = tx_lookup, 1 = index_storage, 2 = index_account
    next_stage: usize,
}

impl std::fmt::Debug for DeferredHistoryIndexer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeferredHistoryIndexer")
            .field("tx_lookup_batch_size", &self.tx_lookup_batch_size)
            .field("index_storage_batch_size", &self.index_storage_batch_size)
            .field("index_account_batch_size", &self.index_account_batch_size)
            .field("caught_up", &self.caught_up)
            .field("next_stage", &self.next_stage)
            .finish()
    }
}

impl DeferredHistoryIndexer {
    /// Creates a new deferred history indexer.
    pub fn new(stages_config: &StageConfig, prune_modes: &PruneModes) -> Self {
        let etl = stages_config.etl.clone();

        Self {
            tx_lookup: TransactionLookupStage::new(
                stages_config.transaction_lookup,
                etl.clone(),
                prune_modes.transaction_lookup,
            )
            .with_progress_logging(false),
            index_storage: IndexStorageHistoryStage::new(
                stages_config.index_storage_history,
                etl.clone(),
                prune_modes.storage_history,
            )
            .with_progress_logging(false),
            index_account: IndexAccountHistoryStage::new(
                stages_config.index_account_history,
                etl,
                prune_modes.account_history,
            )
            .with_progress_logging(false),
            tx_lookup_prune_mode: prune_modes.transaction_lookup,
            index_storage_prune_mode: prune_modes.storage_history,
            index_account_prune_mode: prune_modes.account_history,
            tx_lookup_batch_size: TX_LOOKUP_BATCH_SIZE,
            index_storage_batch_size: INDEX_STORAGE_HISTORY_BATCH_SIZE,
            index_account_batch_size: INDEX_ACCOUNT_HISTORY_BATCH_SIZE,
            caught_up: false,
            next_stage: 0,
        }
    }

    /// Returns the per-stage batch size.
    fn batch_size_for_stage(&self, stage_idx: usize) -> u64 {
        match stage_idx {
            0 => self.tx_lookup_batch_size,
            1 => self.index_storage_batch_size,
            2 => self.index_account_batch_size,
            _ => unreachable!(),
        }
    }

    /// Returns the prune mode/segment pair for the deferred stage index.
    fn prune_mode_and_segment_for_stage(
        &self,
        stage_idx: usize,
    ) -> (Option<PruneMode>, PruneSegment) {
        match stage_idx {
            0 => (self.tx_lookup_prune_mode, PruneSegment::TransactionLookup),
            1 => (self.index_storage_prune_mode, PruneSegment::StorageHistory),
            2 => (self.index_account_prune_mode, PruneSegment::AccountHistory),
            _ => unreachable!(),
        }
    }

    /// Computes the prune floor at the provided tip for the deferred stage index.
    fn prune_floor_for_stage(
        &self,
        stage_idx: usize,
        tip: u64,
    ) -> Result<Option<u64>, Box<dyn std::error::Error + Send + Sync>> {
        let (prune_mode, segment) = self.prune_mode_and_segment_for_stage(stage_idx);
        let Some(prune_mode) = prune_mode else {
            return Ok(None);
        };

        let prune_floor = prune_mode
            .prune_target_block(tip, segment, PrunePurpose::User)?
            .map(|(target_prunable_block, _)| target_prunable_block);

        Ok(prune_floor)
    }

    /// Runs one stage batch and returns whether work was done.
    fn run_stage_batch<Provider>(
        &mut self,
        provider_rw: &Provider,
        stage_idx: usize,
        tip: u64,
    ) -> Result<Option<DeferredStageProgress>, Box<dyn std::error::Error + Send + Sync>>
    where
        Provider: StageCheckpointReader + StageCheckpointWriter,
        TransactionLookupStage: Stage<Provider>,
        IndexStorageHistoryStage: Stage<Provider>,
        IndexAccountHistoryStage: Stage<Provider>,
    {
        let stage_id = match stage_idx {
            0 => StageId::TransactionLookup,
            1 => StageId::IndexStorageHistory,
            2 => StageId::IndexAccountHistory,
            _ => unreachable!(),
        };

        let checkpoint = provider_rw.get_stage_checkpoint(stage_id)?.unwrap_or_default();

        if checkpoint.block_number >= tip {
            return Ok(None);
        }

        let batch_size = self.batch_size_for_stage(stage_idx);
        let batch_target = std::cmp::min(tip, checkpoint.block_number.saturating_add(batch_size));
        let prune_floor = self.prune_floor_for_stage(stage_idx, tip)?;
        let effective_target = prune_floor.map_or(batch_target, |floor| batch_target.max(floor));

        if effective_target > batch_target && checkpoint.block_number < effective_target {
            info!(
                target: "engine::persistence::deferred_indexer",
                %stage_id,
                checkpoint = checkpoint.block_number,
                batch_target,
                effective_target,
                prune_floor = prune_floor.unwrap_or_default(),
                %tip,
                "Deferred indexing bootstrap: raising target to prune floor"
            );
        }

        let input = ExecInput { target: Some(effective_target), checkpoint: Some(checkpoint) };

        debug!(
            target: "engine::persistence::deferred_indexer",
            %stage_id,
            from = checkpoint.block_number,
            to = effective_target,
            %tip,
            "Running deferred indexing batch"
        );

        let result = match stage_idx {
            0 => self.tx_lookup.execute(provider_rw, input),
            1 => self.index_storage.execute(provider_rw, input),
            2 => self.index_account.execute(provider_rw, input),
            _ => unreachable!(),
        };

        match result {
            Ok(output) => {
                provider_rw.save_stage_checkpoint(stage_id, output.checkpoint)?;
                info!(
                    target: "engine::persistence::deferred_indexer",
                    %stage_id,
                    checkpoint = output.checkpoint.block_number,
                    %tip,
                    "Deferred indexing batch complete"
                );
                Ok(Some(DeferredStageProgress { stage_id, checkpoint: output.checkpoint }))
            }
            Err(err) => {
                warn!(
                    target: "engine::persistence::deferred_indexer",
                    %stage_id,
                    %err,
                    "Deferred indexing batch failed, will retry"
                );
                Ok(None)
            }
        }
    }

    /// Check if all three stages have caught up to the provided pipeline tip.
    fn check_caught_up<Provider>(
        &self,
        provider: &Provider,
        tip: u64,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>>
    where
        Provider: StageCheckpointReader,
    {
        if tip == 0 {
            return Ok(false);
        }

        let tx_lookup = provider
            .get_stage_checkpoint(StageId::TransactionLookup)?
            .map(|c| c.block_number)
            .unwrap_or(0);
        let storage = provider
            .get_stage_checkpoint(StageId::IndexStorageHistory)?
            .map(|c| c.block_number)
            .unwrap_or(0);
        let account = provider
            .get_stage_checkpoint(StageId::IndexAccountHistory)?
            .map(|c| c.block_number)
            .unwrap_or(0);

        Ok(tx_lookup >= tip && storage >= tip && account >= tip)
    }
}

impl DeferredHistoryIndexer {
    /// Runs a single deferred indexing tick using the caller-provided transaction/provider.
    pub fn tick_with_provider<Provider>(
        &mut self,
        provider_rw: &Provider,
        tip: u64,
    ) -> Result<Option<DeferredStageProgress>, Box<dyn std::error::Error + Send + Sync>>
    where
        Provider: StageCheckpointReader + StageCheckpointWriter,
        TransactionLookupStage: Stage<Provider>,
        IndexStorageHistoryStage: Stage<Provider>,
        IndexAccountHistoryStage: Stage<Provider>,
    {
        if tip == 0 {
            return Ok(None);
        }

        // Run one batch for the current stage.
        let current = self.next_stage;
        let progress = self.run_stage_batch(provider_rw, current, tip)?;

        // Advance to next stage (round-robin).
        self.next_stage = (self.next_stage + 1) % 3;

        // Check if all stages are caught up.
        if self.check_caught_up(provider_rw, tip)? && !self.caught_up {
            info!(target: "engine::persistence::deferred_indexer", "Deferred history indexing caught up to tip");
            self.caught_up = true;
        }

        Ok(progress)
    }

    /// Returns whether deferred indexing has caught up to the tip.
    pub const fn is_caught_up(&self) -> bool {
        self.caught_up
    }

    /// Marks the indexer as needing catch-up after reorg-related block removal.
    pub fn on_reorg(&mut self, _new_tip_num: u64) {
        self.caught_up = false;
    }
}
