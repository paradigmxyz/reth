use crate::{ExecInput, ExecOutput, Stage, StageError, UnwindInput, UnwindOutput};
use reth_db::database::Database;
use reth_primitives::{
    static_file::HighestSnapshots,
    stage::{StageCheckpoint, StageId},
};
use reth_provider::{DatabaseProviderRW, StageCheckpointReader};
use reth_snapshot::Snapshotter;

/// The snapshot stage _copies_ all data from database to static files using [Snapshotter]. The
/// block range for copying is determined by the current highest blocks contained in static files
/// and stage checkpoints for each segment individually.
#[derive(Debug)]
pub struct SnapshotStage<DB: Database> {
    snapshotter: Snapshotter<DB>,
}

impl<DB: Database> SnapshotStage<DB> {
    /// Creates a new snapshot stage.
    pub fn new(snapshotter: Snapshotter<DB>) -> Self {
        Self { snapshotter }
    }
}

impl<DB: Database> Stage<DB> for SnapshotStage<DB> {
    fn id(&self) -> StageId {
        StageId::Snapshot
    }

    fn execute(
        &mut self,
        provider: &DatabaseProviderRW<DB>,
        input: ExecInput,
    ) -> Result<ExecOutput, StageError> {
        let targets = self.snapshotter.get_snapshot_targets(HighestSnapshots {
            headers: provider
                .get_stage_checkpoint(StageId::Headers)?
                .map(|checkpoint| checkpoint.block_number),
            receipts: provider
                .get_stage_checkpoint(StageId::Execution)?
                .map(|checkpoint| checkpoint.block_number),
            transactions: provider
                .get_stage_checkpoint(StageId::Bodies)?
                .map(|checkpoint| checkpoint.block_number),
        })?;
        self.snapshotter.run(targets)?;
        Ok(ExecOutput::done(input.checkpoint()))
    }

    fn unwind(
        &mut self,
        _provider: &DatabaseProviderRW<DB>,
        input: UnwindInput,
    ) -> Result<UnwindOutput, StageError> {
        Ok(UnwindOutput { checkpoint: StageCheckpoint::new(input.unwind_to) })
    }
}
