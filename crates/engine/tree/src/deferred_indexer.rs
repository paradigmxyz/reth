//! Deferred history indexer implementation using pipeline stages.
//!
//! Runs history indexing stages (`TransactionLookup`, `IndexStorageHistory`,
//! `IndexAccountHistory`) incrementally inside the persistence service's thread. This eliminates
//! MDBX write-lock contention that would occur if these stages ran on a separate thread.

use reth_config::config::StageConfig;
use reth_provider::{ProviderError, StageCheckpointReader, StageCheckpointWriter};
use reth_prune::{PruneMode, PruneModes, PrunePurpose, PruneSegment, PruneSegmentError};
use reth_stages::{
    stages::{IndexAccountHistoryStage, IndexStorageHistoryStage, TransactionLookupStage},
    ExecInput, Stage, StageCheckpoint, StageError, StageId,
};
use tracing::{debug, info};

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
    pub const fn new(stages: StageConfig, prune_modes: PruneModes) -> Self {
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

/// Errors produced by deferred history indexing.
#[derive(Debug, thiserror::Error)]
pub enum DeferredIndexerError {
    /// Provider/storage error.
    #[error(transparent)]
    Provider(#[from] ProviderError),
    /// Invalid prune configuration/target computation.
    #[error(transparent)]
    Prune(#[from] PruneSegmentError),
    /// Stage execution error.
    #[error(transparent)]
    Stage(#[from] StageError),
}

/// Deferred indexer lifecycle state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DeferredIndexerState {
    CaughtUp,
    TransactionLookup,
    StorageHistory,
    AccountHistory,
}

impl DeferredIndexerState {
    /// Initial state for a new deferred indexer.
    const fn new() -> Self {
        Self::TransactionLookup
    }

    /// Returns whether the deferred indexer is caught up to tip.
    const fn is_caught_up(self) -> bool {
        matches!(self, Self::CaughtUp)
    }

    /// Returns the current catch-up stage, if any.
    const fn catching_up_stage(self) -> Option<StageId> {
        match self {
            Self::CaughtUp => None,
            Self::TransactionLookup => Some(StageId::TransactionLookup),
            Self::StorageHistory => Some(StageId::IndexStorageHistory),
            Self::AccountHistory => Some(StageId::IndexAccountHistory),
        }
    }

    /// Advances catch-up round-robin stage.
    const fn advance_catch_up_stage(self) -> Self {
        match self {
            Self::CaughtUp => Self::CaughtUp,
            Self::TransactionLookup => Self::StorageHistory,
            Self::StorageHistory => Self::AccountHistory,
            Self::AccountHistory => Self::TransactionLookup,
        }
    }

    /// Transitions to the caught-up state.
    const fn mark_caught_up(mut self) -> Self {
        self = Self::CaughtUp;
        self
    }

    /// Transitions to catch-up mode from the first deferred stage.
    const fn mark_needs_catch_up(mut self) -> Self {
        self = Self::TransactionLookup;
        self
    }
}

/// Deferred history indexer that runs pipeline stages inside the persistence service.
///
/// Processes history indexing in small batches, round-robining between the three deferred
/// stages (`TransactionLookup`, `IndexStorageHistory`, `IndexAccountHistory`).
pub struct DeferredHistoryIndexer {
    /// Deferred transaction-lookup stage instance, executed incrementally by persistence ticks.
    tx_lookup: TransactionLookupStage,
    /// Deferred storage-history indexing stage instance.
    index_storage: IndexStorageHistoryStage,
    /// Deferred account-history indexing stage instance.
    index_account: IndexAccountHistoryStage,
    /// Max blocks to advance `TransactionLookup` in a single tick.
    tx_lookup_batch_size: u64,
    /// Max blocks to advance `IndexStorageHistory` in a single tick.
    index_storage_batch_size: u64,
    /// Max blocks to advance `IndexAccountHistory` in a single tick.
    index_account_batch_size: u64,
    /// Deferred indexing lifecycle state plus round-robin stage selection.
    state: DeferredIndexerState,
}

impl std::fmt::Debug for DeferredHistoryIndexer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeferredHistoryIndexer")
            .field("tx_lookup_batch_size", &self.tx_lookup_batch_size)
            .field("index_storage_batch_size", &self.index_storage_batch_size)
            .field("index_account_batch_size", &self.index_account_batch_size)
            .field("state", &self.state)
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
            tx_lookup_batch_size: TX_LOOKUP_BATCH_SIZE,
            index_storage_batch_size: INDEX_STORAGE_HISTORY_BATCH_SIZE,
            index_account_batch_size: INDEX_ACCOUNT_HISTORY_BATCH_SIZE,
            state: DeferredIndexerState::new(),
        }
    }

    /// Returns the per-stage batch size.
    fn batch_size_for_stage(&self, stage_id: StageId) -> u64 {
        match stage_id {
            StageId::TransactionLookup => self.tx_lookup_batch_size,
            StageId::IndexStorageHistory => self.index_storage_batch_size,
            StageId::IndexAccountHistory => self.index_account_batch_size,
            _ => unreachable!(),
        }
    }

    /// Returns the prune mode/segment pair for the deferred stage.
    fn prune_mode_and_segment_for_stage(
        &self,
        stage_id: StageId,
    ) -> (Option<PruneMode>, PruneSegment) {
        match stage_id {
            StageId::TransactionLookup => {
                (self.tx_lookup.prune_mode(), PruneSegment::TransactionLookup)
            }
            StageId::IndexStorageHistory => {
                (self.index_storage.prune_mode(), PruneSegment::StorageHistory)
            }
            StageId::IndexAccountHistory => {
                (self.index_account.prune_mode(), PruneSegment::AccountHistory)
            }
            _ => unreachable!(),
        }
    }

    /// Computes the prune floor at the provided tip for the deferred stage.
    fn prune_floor_for_stage(
        &self,
        stage_id: StageId,
        tip: u64,
    ) -> Result<Option<u64>, DeferredIndexerError> {
        let (prune_mode, segment) = self.prune_mode_and_segment_for_stage(stage_id);
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
        stage_id: StageId,
        tip: u64,
    ) -> Result<Option<DeferredStageProgress>, DeferredIndexerError>
    where
        Provider: StageCheckpointReader + StageCheckpointWriter,
        TransactionLookupStage: Stage<Provider>,
        IndexStorageHistoryStage: Stage<Provider>,
        IndexAccountHistoryStage: Stage<Provider>,
    {
        let checkpoint = provider_rw.get_stage_checkpoint(stage_id)?.unwrap_or_default();

        if checkpoint.block_number >= tip {
            return Ok(None);
        }

        let batch_size = self.batch_size_for_stage(stage_id);
        let batch_target = std::cmp::min(tip, checkpoint.block_number.saturating_add(batch_size));
        let prune_floor = self.prune_floor_for_stage(stage_id, tip)?;
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

        let result = match stage_id {
            StageId::TransactionLookup => self.tx_lookup.execute(provider_rw, input),
            StageId::IndexStorageHistory => self.index_storage.execute(provider_rw, input),
            StageId::IndexAccountHistory => self.index_account.execute(provider_rw, input),
            _ => unreachable!(),
        };

        let output = result?;
        provider_rw.save_stage_checkpoint(stage_id, output.checkpoint)?;
        Ok(Some(DeferredStageProgress { stage_id, checkpoint: output.checkpoint }))
    }

    /// Check if all three stages have caught up to the provided pipeline tip.
    fn check_caught_up<Provider>(
        &self,
        provider: &Provider,
        tip: u64,
    ) -> Result<bool, DeferredIndexerError>
    where
        Provider: StageCheckpointReader,
    {
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
    ) -> Result<Option<DeferredStageProgress>, DeferredIndexerError>
    where
        Provider: StageCheckpointReader + StageCheckpointWriter,
        TransactionLookupStage: Stage<Provider>,
        IndexStorageHistoryStage: Stage<Provider>,
        IndexAccountHistoryStage: Stage<Provider>,
    {
        if self.state.is_caught_up() {
            if self.check_caught_up(provider_rw, tip)? {
                return Ok(None);
            }
            self.state = self.state.mark_needs_catch_up();
        }

        // Run one batch for the current stage.
        let current_stage =
            self.state.catching_up_stage().expect("non-caught-up state must have a catch-up stage");
        let progress = self.run_stage_batch(provider_rw, current_stage, tip)?;

        // Advance to next stage (round-robin).
        self.state = self.state.advance_catch_up_stage();

        // Check if all stages are caught up.
        if self.check_caught_up(provider_rw, tip)? && !self.state.is_caught_up() {
            info!(target: "engine::persistence::deferred_indexer", "Deferred history indexing caught up to tip");
            self.state = self.state.mark_caught_up();
        }

        Ok(progress)
    }

    /// Returns whether deferred indexing has caught up to the tip.
    pub fn is_caught_up(&self) -> bool {
        self.state.is_caught_up()
    }

    /// Marks the indexer as needing catch-up after reorg-related block removal.
    pub fn on_reorg(&mut self, _new_tip_num: u64) {
        self.state = self.state.mark_needs_catch_up();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reth_provider::ProviderResult;
    use std::string::String;

    #[test]
    fn reorg_marks_indexer_as_not_caught_up() {
        let mut indexer =
            DeferredHistoryIndexer::new(&StageConfig::default(), &PruneModes::default());
        indexer.state = DeferredIndexerState::CaughtUp;

        indexer.on_reorg(123);

        assert!(!indexer.is_caught_up());
        assert_eq!(indexer.state, DeferredIndexerState::TransactionLookup);
    }

    #[derive(Default)]
    struct MockStageCheckpointReader {
        tx_lookup: Option<StageCheckpoint>,
        storage_history: Option<StageCheckpoint>,
        account_history: Option<StageCheckpoint>,
    }

    impl StageCheckpointReader for MockStageCheckpointReader {
        fn get_stage_checkpoint(&self, id: StageId) -> ProviderResult<Option<StageCheckpoint>> {
            Ok(match id {
                StageId::TransactionLookup => self.tx_lookup,
                StageId::IndexStorageHistory => self.storage_history,
                StageId::IndexAccountHistory => self.account_history,
                _ => None,
            })
        }

        fn get_stage_checkpoint_progress(&self, _id: StageId) -> ProviderResult<Option<Vec<u8>>> {
            Ok(None)
        }

        fn get_all_checkpoints(&self) -> ProviderResult<Vec<(String, StageCheckpoint)>> {
            Ok(Vec::new())
        }
    }

    #[test]
    fn genesis_tip_is_trivially_caught_up() {
        let indexer = DeferredHistoryIndexer::new(&StageConfig::default(), &PruneModes::default());
        let provider = MockStageCheckpointReader::default();

        assert!(indexer.check_caught_up(&provider, 0).unwrap());
    }
}
