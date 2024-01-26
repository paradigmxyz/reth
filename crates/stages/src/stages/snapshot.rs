use crate::{ExecInput, ExecOutput, Stage, StageError, UnwindInput, UnwindOutput};
use reth_db::database::Database;
use reth_primitives::stage::{StageCheckpoint, StageId};
use reth_provider::DatabaseProviderRW;
use reth_snapshot::Snapshotter;

/// The snapshot stage.
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
        _provider: &DatabaseProviderRW<DB>,
        input: ExecInput,
    ) -> Result<ExecOutput, StageError> {
        let targets = self.snapshotter.get_snapshot_targets(input.target())?;
        self.snapshotter.run(targets)?;
        Ok(ExecOutput { checkpoint: StageCheckpoint::new(input.target()), done: true })
    }

    fn unwind(
        &mut self,
        _provider: &DatabaseProviderRW<DB>,
        _input: UnwindInput,
    ) -> Result<UnwindOutput, StageError> {
        // TODO(alexey): return proper error
        Err(StageError::ChannelClosed)
    }
}
