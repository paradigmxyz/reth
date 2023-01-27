use crate::{pipeline::QueuedStage, Pipeline, Stage, StageSet};
use reth_db::database::Database;
use reth_interfaces::sync::{NoopSyncStateUpdate, SyncStateUpdater};
use reth_primitives::BlockNumber;

/// Builds a [`Pipeline`].
#[derive(Debug)]
#[must_use = "call `build` to construct the pipeline"]
pub struct PipelineBuilder<DB, U = NoopSyncStateUpdate>
where
    DB: Database,
    U: SyncStateUpdater,
{
    pipeline: Pipeline<DB, U>,
}

impl<DB: Database, U: SyncStateUpdater> Default for PipelineBuilder<DB, U> {
    fn default() -> Self {
        Self { pipeline: Pipeline::default() }
    }
}

impl<DB, U> PipelineBuilder<DB, U>
where
    DB: Database,
    U: SyncStateUpdater,
{
    /// Add a stage to the pipeline.
    pub fn add_stage<S>(mut self, stage: S) -> Self
    where
        S: Stage<DB> + 'static,
    {
        self.pipeline.stages.push(QueuedStage { stage: Box::new(stage) });
        self
    }

    /// Add a set of stages to the pipeline.
    ///
    /// Stages can be grouped into a set by using a [`StageSet`].
    ///
    /// To customize the stages in the set (reorder, disable, insert a stage) call
    /// [`build`][StageSet::build] on the set which will convert it to a
    /// [`StageSetBuilder`][crate::StageSetBuilder].
    pub fn add_stages<Set: StageSet<DB>>(mut self, set: Set) -> Self {
        for stage in set.builder().build() {
            self.pipeline.stages.push(QueuedStage { stage });
        }
        self
    }

    /// Set the target block.
    ///
    /// Once this block is reached, the pipeline will stop.
    pub fn with_max_block(mut self, block: BlockNumber) -> Self {
        self.pipeline.max_block = Some(block);
        self
    }

    /// Set a [SyncStateUpdater].
    pub fn with_sync_state_updater(mut self, updater: U) -> Self {
        self.pipeline.sync_state_updater = Some(updater);
        self
    }

    /// Builds the final [`Pipeline`].
    pub fn build(self) -> Pipeline<DB, U> {
        self.pipeline
    }
}
