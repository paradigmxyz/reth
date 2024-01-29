use crate::{ExecInput, ExecOutput, Stage, StageError, UnwindInput, UnwindOutput};
use reth_db::database::Database;
use reth_primitives::stage::StageId;
use reth_provider::{BlockNumReader, DatabaseProviderRW};
use reth_snapshot::Snapshotter;

/// The snapshot stage _copies_ all data from database to static files using [Snapshotter]. The
/// block range for copying is determined by the current highest blocks contained in static files
/// and [BlockNumReader::best_block_number],
/// i.e. the range is `highest_snapshotted_block_number..=best_block_number`.
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
        let targets = self.snapshotter.get_snapshot_targets(provider.best_block_number()?)?;
        self.snapshotter.run(targets)?;
        Ok(ExecOutput::done(input.checkpoint()))
    }

    fn unwind(
        &mut self,
        _provider: &DatabaseProviderRW<DB>,
        _input: UnwindInput,
    ) -> Result<UnwindOutput, StageError> {
        Err(StageError::UnwindNotSupported)
    }
}
